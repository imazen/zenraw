//! Color processing pipeline for camera RAW data.
//!
//! After demosaicing, camera RGB values need:
//! 1. White balance application
//! 2. Camera-to-XYZ color matrix transform
//! 3. XYZ-to-output-primaries conversion
//! 4. Clamp to [0, 1]
//!
//! This module performs steps 1-4 in a single pass over the pixel data.

use archmage::prelude::*;

use crate::dng_render::OutputPrimaries;

/// Apply the full color pipeline to demosaiced camera RGB data in-place.
///
/// Transforms camera RGB → white-balanced camera RGB → XYZ → linear output primaries.
///
/// `rgb`: interleaved f32 RGB data (3 components per pixel)
/// `wb_coeffs`: white balance multipliers [R, G, B, E] from rawloader
/// `xyz_to_cam`: 4×3 matrix (XYZ→camera) from rawloader — we invert it
/// `target`: output color primaries (sRGB, Display P3, or BT.2020)
pub fn apply_color_pipeline(
    rgb: &mut [f32],
    wb_coeffs: [f32; 4],
    xyz_to_cam: [[f32; 3]; 4],
    target: OutputPrimaries,
) {
    let cam_to_output = compute_cam_to_output_matrix(wb_coeffs, xyz_to_cam, target);
    apply_color_matrix(rgb, cam_to_output);
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

/// Compute the combined white-balance + camera-to-output matrix.
///
/// The pipeline is:
///   camera_rgb → WB(camera_rgb) → XYZ → linear output primaries
///
/// cam_to_xyz = inverse(xyz_to_cam)  (3×3, first 3 rows of the 4×3)
/// xyz_to_output is the standard D65 matrix for the target color space
/// WB is a diagonal matrix with wb_coeffs normalized by the green channel
///
/// Combined: xyz_to_output × cam_to_xyz × WB_diag
fn compute_cam_to_output_matrix(
    wb_coeffs: [f32; 4],
    xyz_to_cam: [[f32; 3]; 4],
    target: OutputPrimaries,
) -> [[f32; 3]; 3] {
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

    let xyz_to_output = xyz_to_rgb_d65(target);

    // cam_to_output = xyz_to_output × cam_to_xyz
    let cam_to_output = mat_mul(xyz_to_output, cam_to_xyz);

    // Normalize rows to sum to 1.
    // This ensures that equal-channel input (a neutral) maps to equal
    // output, so WB column-multiply produces correct neutrals.
    let cam_to_output = normalize_rows(cam_to_output);

    // Apply white balance: multiply each column by the WB factor
    [
        [
            cam_to_output[0][0] * wb[0],
            cam_to_output[0][1] * wb[1],
            cam_to_output[0][2] * wb[2],
        ],
        [
            cam_to_output[1][0] * wb[0],
            cam_to_output[1][1] * wb[1],
            cam_to_output[1][2] * wb[2],
        ],
        [
            cam_to_output[2][0] * wb[0],
            cam_to_output[2][1] * wb[1],
            cam_to_output[2][2] * wb[2],
        ],
    ]
}

/// XYZ→linear RGB matrix (D65 illuminant) for the given output primaries.
#[allow(clippy::excessive_precision)]
fn xyz_to_rgb_d65(target: OutputPrimaries) -> [[f32; 3]; 3] {
    match target {
        // IEC 61966-2-1 sRGB / BT.709
        OutputPrimaries::Srgb => [
            [3.2404542, -1.5371385, -0.4985314],
            [-0.9692660, 1.8760108, 0.0415560],
            [0.0556434, -0.2040259, 1.0572252],
        ],
        // Display P3 (D65 white point)
        OutputPrimaries::DisplayP3 => [
            [2.4934969, -0.9313836, -0.4027108],
            [-0.8294890, 1.7626641, 0.0236247],
            [0.0358458, -0.0761724, 0.9568845],
        ],
        // BT.2020 / BT.2100 (D65 white point)
        OutputPrimaries::Bt2020 => [
            [1.7166512, -0.3556708, -0.2533663],
            [-0.6666844, 1.6164812, 0.0157685],
            [0.0176399, -0.0427706, 0.9421031],
        ],
    }
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

/// Apply sRGB gamma curve (linear → sRGB transfer function), in place.
///
/// Operates on interleaved RGB f32 data (values should be in \[0, 1\]). The
/// output is still f32 in \[0, 1\], now sRGB-*encoded*. This is the only
/// public function in this module that applies a transfer function; feed its
/// result to [`f32_to_u8_srgb`] to quantise. Do **not** call it on
/// [`OutputMode::Develop`](crate::OutputMode::Develop) output, which is
/// already sRGB-encoded — that double-encodes.
pub fn apply_srgb_gamma(rgb: &mut [f32]) {
    for val in rgb.iter_mut() {
        *val = crate::simd::linear_to_srgb(*val);
    }
}

/// Quantise f32 \[0,1\] **already sRGB-encoded** samples to u8 \[0,255\].
///
/// A plain clamp-to-\[0, 1\], scale-by-255 and round: it applies **no**
/// transfer function. The `_srgb` in the name says what the input is expected
/// to be, not what this does — run [`apply_srgb_gamma`] first on linear data.
/// Composing the two in that order is exactly one sRGB encode.
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

    /// `f32_to_u8_srgb` is a pure quantiser: it must NOT apply the sRGB curve
    /// (issue #7 footgun — `apply_srgb_gamma` + `f32_to_u8_srgb` composed must
    /// encode exactly once). Linear 0.5 → sRGB is 188, not 128; a quantiser
    /// that secretly encoded would return 188 here.
    #[test]
    fn f32_to_u8_srgb_applies_no_transfer_function() {
        let out = f32_to_u8_srgb(&[0.5, 0.25, 1.0 / 255.0]);
        assert_eq!(out, [128, 64, 1]);
        // And the documented composition encodes exactly once.
        let mut lin = [0.5f32];
        apply_srgb_gamma(&mut lin);
        assert_eq!(f32_to_u8_srgb(&lin), [188]);
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
        apply_color_pipeline(&mut rgb, wb, xtc, OutputPrimaries::Srgb);
        for &v in &rgb {
            assert!((0.0..=1.0).contains(&v), "Out of range: {v}");
        }
    }
}
