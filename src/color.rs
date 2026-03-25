//! Color processing pipeline for camera RAW data.
//!
//! After demosaicing, camera RGB values need:
//! 1. White balance application
//! 2. Camera-to-XYZ color matrix transform
//! 3. XYZ-to-linear-sRGB conversion
//! 4. Clamp to [0, 1]
//!
//! This module performs steps 1-4 in a single pass over the pixel data.

use archmage::prelude::*;

/// Apply the full color pipeline to demosaiced camera RGB data in-place.
///
/// Transforms camera RGB → white-balanced camera RGB → XYZ → linear sRGB.
///
/// `rgb`: interleaved f32 RGB data (3 components per pixel)
/// `wb_coeffs`: white balance multipliers [R, G, B, E] from rawloader
/// `xyz_to_cam`: 4×3 matrix (XYZ→camera) from rawloader — we invert it
pub fn apply_color_pipeline(rgb: &mut [f32], wb_coeffs: [f32; 4], xyz_to_cam: [[f32; 3]; 4]) {
    let cam_to_srgb = compute_cam_to_srgb_matrix(wb_coeffs, xyz_to_cam);
    apply_color_matrix(rgb, cam_to_srgb);
}

/// Apply a 3×3 color matrix to interleaved RGB data with clamping.
///
/// Autoversioned: compiles for AVX2/NEON/scalar and dispatches at runtime.
#[autoversion]
fn apply_color_matrix(rgb: &mut [f32], mat: [[f32; 3]; 3]) {
    let pixel_count = rgb.len() / 3;
    for i in 0..pixel_count {
        let idx = i * 3;
        let r = rgb[idx];
        let g = rgb[idx + 1];
        let b = rgb[idx + 2];

        let sr = mat[0][0] * r + mat[0][1] * g + mat[0][2] * b;
        let sg = mat[1][0] * r + mat[1][1] * g + mat[1][2] * b;
        let sb = mat[2][0] * r + mat[2][1] * g + mat[2][2] * b;

        rgb[idx] = sr.clamp(0.0, 1.0);
        rgb[idx + 1] = sg.clamp(0.0, 1.0);
        rgb[idx + 2] = sb.clamp(0.0, 1.0);
    }
}

/// Compute the combined white-balance + camera-to-sRGB matrix.
///
/// The pipeline is:
///   camera_rgb → WB(camera_rgb) → XYZ → sRGB_linear
///
/// cam_to_xyz = inverse(xyz_to_cam)  (3×3, first 3 rows of the 4×3)
/// srgb_from_xyz is the standard Bradford-adapted D65 matrix
/// WB is a diagonal matrix with wb_coeffs normalized by the green channel
///
/// Combined: srgb_from_xyz × cam_to_xyz × WB_diag
fn compute_cam_to_srgb_matrix(wb_coeffs: [f32; 4], xyz_to_cam: [[f32; 3]; 4]) -> [[f32; 3]; 3] {
    // Normalize WB coefficients relative to green
    let wb_g = wb_coeffs[1];
    let wb = if wb_g > 0.0 {
        [wb_coeffs[0] / wb_g, 1.0, wb_coeffs[2] / wb_g]
    } else {
        [1.0, 1.0, 1.0]
    };

    // Extract 3×3 from xyz_to_cam (drop 4th row — E channel)
    let xtc = [
        [xyz_to_cam[0][0], xyz_to_cam[0][1], xyz_to_cam[0][2]],
        [xyz_to_cam[1][0], xyz_to_cam[1][1], xyz_to_cam[1][2]],
        [xyz_to_cam[2][0], xyz_to_cam[2][1], xyz_to_cam[2][2]],
    ];

    // Normalize rows of xyz_to_cam so each sums to 1
    let xtc_norm = normalize_rows(xtc);

    // cam_to_xyz = inverse(xtc_norm)
    let cam_to_xyz = invert_3x3(xtc_norm);

    // Standard XYZ→linear sRGB matrix (D65, IEC 61966-2-1)
    // Standard IEC 61966-2-1 XYZ→linear sRGB matrix (D65 adapted).
    // Values rounded to f32 representable precision.
    #[allow(clippy::excessive_precision)]
    let xyz_to_srgb = [
        [3.2404542, -1.5371385, -0.4985314],
        [-0.9692660, 1.8760108, 0.0415560],
        [0.0556434, -0.2040259, 1.0572252],
    ];

    // cam_to_srgb = xyz_to_srgb × cam_to_xyz
    let cam_to_srgb = mat_mul(xyz_to_srgb, cam_to_xyz);

    // Normalize cam_to_srgb rows to sum to 1.
    // This ensures that equal-channel input (a neutral) maps to equal
    // sRGB output, so WB column-multiply produces correct neutrals.
    let cam_to_srgb = normalize_rows(cam_to_srgb);

    // Apply white balance: multiply each column by the WB factor
    [
        [
            cam_to_srgb[0][0] * wb[0],
            cam_to_srgb[0][1] * wb[1],
            cam_to_srgb[0][2] * wb[2],
        ],
        [
            cam_to_srgb[1][0] * wb[0],
            cam_to_srgb[1][1] * wb[1],
            cam_to_srgb[1][2] * wb[2],
        ],
        [
            cam_to_srgb[2][0] * wb[0],
            cam_to_srgb[2][1] * wb[1],
            cam_to_srgb[2][2] * wb[2],
        ],
    ]
}

/// Normalize each row of a 3×3 matrix to sum to 1.
fn normalize_rows(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = m;
    for row in &mut out {
        let sum: f32 = row.iter().sum();
        if sum.abs() > 1e-10 {
            for v in row.iter_mut() {
                *v /= sum;
            }
        }
    }
    out
}

/// 3×3 matrix multiplication.
fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

/// Invert a 3×3 matrix using Cramer's rule.
fn invert_3x3(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);

    if det.abs() < 1e-10 {
        // Singular matrix — return identity as fallback
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }

    let inv_det = 1.0 / det;

    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ]
}

/// Apply sRGB gamma curve (linear → sRGB transfer function).
///
/// Operates on interleaved RGB f32 data (values should be in \[0, 1\]).
pub fn apply_srgb_gamma(rgb: &mut [f32]) {
    for val in rgb.iter_mut() {
        *val = crate::simd::linear_to_srgb(*val);
    }
}

/// Convert f32 \[0,1\] RGB data to u8 \[0,255\] sRGB data.
pub fn f32_to_u8_srgb(src: &[f32]) -> alloc::vec::Vec<u8> {
    f32_to_u8_inner(src)
}

#[autoversion]
fn f32_to_u8_inner(src: &[f32]) -> alloc::vec::Vec<u8> {
    src.iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect()
}

/// Convert f32 \[0,1\] RGB data to u16 \[0,65535\] data.
pub(crate) fn f32_to_u16(src: &[f32]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::with_capacity(src.len() * 2);
    for &v in src {
        let val = (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
        out.extend_from_slice(&val.to_ne_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn identity_matrix_inversion() {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let inv = invert_3x3(id);
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (inv[i][j] - expected).abs() < 1e-5,
                    "inv[{i}][{j}] = {} != {expected}",
                    inv[i][j]
                );
            }
        }
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn matrix_mul_identity() {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let m = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        let result = mat_mul(id, m);
        for i in 0..3 {
            for j in 0..3 {
                assert!((result[i][j] - m[i][j]).abs() < 1e-5);
            }
        }
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn invert_and_multiply_is_identity() {
        let m = [[2.0, 1.0, 0.0], [0.0, 3.0, 1.0], [1.0, 0.0, 2.0]];
        let inv = invert_3x3(m);
        let product = mat_mul(m, inv);
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (product[i][j] - expected).abs() < 1e-4,
                    "product[{i}][{j}] = {} != {expected}",
                    product[i][j]
                );
            }
        }
    }

    #[test]
    fn srgb_gamma_boundaries() {
        assert!((crate::simd::linear_to_srgb(0.0) - 0.0).abs() < 1e-6);
        assert!((crate::simd::linear_to_srgb(1.0) - 1.0).abs() < 1e-4);
        // Linear segment
        assert!((crate::simd::linear_to_srgb(0.001) - 0.001 * 12.92).abs() < 1e-6);
        // Transition point
        let at_transition = crate::simd::linear_to_srgb(0.0031308);
        assert!(at_transition > 0.03 && at_transition < 0.05);
    }

    #[test]
    fn srgb_gamma_monotonic() {
        let mut prev = 0.0f32;
        for i in 0..=1000 {
            let x = i as f32 / 1000.0;
            let y = crate::simd::linear_to_srgb(x);
            assert!(y >= prev, "sRGB gamma not monotonic at x={x}: {y} < {prev}");
            prev = y;
        }
    }

    #[test]
    fn normalize_rows_sums_to_one() {
        let m = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        let n = normalize_rows(m);
        for row in &n {
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "Row sum = {sum}");
        }
    }

    #[test]
    fn f32_to_u8_clamps() {
        let data = [-0.1f32, 0.0, 0.5, 1.0, 1.5];
        let out = f32_to_u8_srgb(&data);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 0);
        assert_eq!(out[2], 128);
        assert_eq!(out[3], 255);
        assert_eq!(out[4], 255);
    }

    #[test]
    fn color_pipeline_does_not_crash() {
        let mut rgb = vec![0.5f32; 12]; // 4 pixels
        let wb = [2.0, 1.0, 1.5, 0.0];
        // Simple identity-ish xyz_to_cam
        let xtc = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
        ];
        apply_color_pipeline(&mut rgb, wb, xtc);
        for &v in &rgb {
            assert!((0.0..=1.0).contains(&v), "Out of range: {v}");
        }
    }
}
