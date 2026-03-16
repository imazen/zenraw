//! DNG rendering pipeline following Adobe DNG SDK specification.
//!
//! Implements the full color pipeline for DNG files:
//! 1. ProfileGainTableMap (spatially-varying Smart HDR gain)
//! 2. White balance + color matrix → ProPhoto RGB
//! 3. ProfileHueSatMap (3D HSV color grading LUT)
//! 4. Exposure compensation
//! 5. ProfileToneCurve (global tone mapping)
//! 6. ProPhoto RGB → sRGB
//! 7. sRGB gamma encoding
//!
//! This module provides the math primitives. The full pipeline requires
//! the `apple` feature for ProfileGainTableMap extraction.
//!
//! Reference: Adobe DNG SDK `dng_render.cpp`, `dng_color_spec.cpp`

extern crate alloc;

use alloc::vec::Vec;

// ── D50 / D65 constants ─────────────────────────────────────────────

/// D50 illuminant xy coordinates (PCS white point).
pub const D50_XY: (f64, f64) = (0.3457, 0.3585);

/// D65 illuminant xy coordinates.
pub const D65_XY: (f64, f64) = (0.3127, 0.3290);

/// Standard illuminant A xy coordinates (incandescent).
pub const STD_A_XY: (f64, f64) = (0.4476, 0.4074);

// ── Color space matrices ────────────────────────────────────────────

/// ProPhoto RGB to XYZ (D50) matrix.
pub const PROPHOTO_TO_XYZ: [[f64; 3]; 3] = [
    [0.7977, 0.1352, 0.0313],
    [0.2880, 0.7119, 0.0001],
    [0.0000, 0.0000, 0.8249],
];

/// sRGB to XYZ (D50) matrix (adapted from D65).
pub const SRGB_TO_XYZ_D50: [[f64; 3]; 3] = [
    [0.4361, 0.3851, 0.1431],
    [0.2225, 0.7169, 0.0606],
    [0.0139, 0.0971, 0.7141],
];

/// Bradford chromatic adaptation matrix.
const BRADFORD: [[f64; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

// ── 3x3 matrix operations ───────────────────────────────────────────

pub type Mat3 = [[f64; 3]; 3];
pub type Vec3 = [f64; 3];

pub fn mat3_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut c = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            c[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    c
}

pub fn mat3_vec(m: &Mat3, v: &Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

pub fn mat3_invert(m: &Mat3) -> Option<Mat3> {
    let a = m;
    let cof00 = a[1][1] * a[2][2] - a[2][1] * a[1][2];
    let cof01 = a[2][1] * a[0][2] - a[0][1] * a[2][2];
    let cof02 = a[0][1] * a[1][2] - a[1][1] * a[0][2];
    let cof10 = a[2][0] * a[1][2] - a[1][0] * a[2][2];
    let cof11 = a[0][0] * a[2][2] - a[2][0] * a[0][2];
    let cof12 = a[1][0] * a[0][2] - a[0][0] * a[1][2];
    let cof20 = a[1][0] * a[2][1] - a[2][0] * a[1][1];
    let cof21 = a[2][0] * a[0][1] - a[0][0] * a[2][1];
    let cof22 = a[0][0] * a[1][1] - a[1][0] * a[0][1];

    let det = a[0][0] * cof00 + a[0][1] * cof10 + a[0][2] * cof20;
    if det.abs() < 1e-15 {
        return None;
    }
    let inv_det = 1.0 / det;

    Some([
        [cof00 * inv_det, cof01 * inv_det, cof02 * inv_det],
        [cof10 * inv_det, cof11 * inv_det, cof12 * inv_det],
        [cof20 * inv_det, cof21 * inv_det, cof22 * inv_det],
    ])
}

pub fn mat3_identity() -> Mat3 {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

pub fn mat3_diagonal(d: &Vec3) -> Mat3 {
    [[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]]
}

pub fn mat3_scale(m: &Mat3, s: f64) -> Mat3 {
    let mut r = *m;
    for row in &mut r {
        for v in row.iter_mut() {
            *v *= s;
        }
    }
    r
}

pub fn mat3_lerp(a: &Mat3, b: &Mat3, t: f64) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][j] * (1.0 - t) + b[i][j] * t;
        }
    }
    r
}

// ── xy ↔ XYZ conversions ────────────────────────────────────────────

/// Convert CIE xy chromaticity to XYZ (Y=1).
pub fn xy_to_xyz(x: f64, y: f64) -> Vec3 {
    let y_clamped = y.max(1e-6);
    [x / y_clamped, 1.0, (1.0 - x - y) / y_clamped]
}

/// Bradford chromatic adaptation matrix from white1 → white2.
pub fn bradford_adapt(white1_xy: (f64, f64), white2_xy: (f64, f64)) -> Mat3 {
    let w1_xyz = xy_to_xyz(white1_xy.0, white1_xy.1);
    let w2_xyz = xy_to_xyz(white2_xy.0, white2_xy.1);

    let w1_lms = mat3_vec(&BRADFORD, &w1_xyz);
    let w2_lms = mat3_vec(&BRADFORD, &w2_xyz);

    let scale = [
        (w2_lms[0] / w1_lms[0].max(1e-10)).clamp(0.1, 10.0),
        (w2_lms[1] / w1_lms[1].max(1e-10)).clamp(0.1, 10.0),
        (w2_lms[2] / w1_lms[2].max(1e-10)).clamp(0.1, 10.0),
    ];

    let scale_mat = mat3_diagonal(&scale);
    let brad_inv = mat3_invert(&BRADFORD).unwrap();
    mat3_mul(&brad_inv, &mat3_mul(&scale_mat, &BRADFORD))
}

// ── Dual-illuminant interpolation ───────────────────────────────────

/// DNG illuminant IDs to approximate color temperature.
pub fn illuminant_to_temp(illuminant: u16) -> f64 {
    match illuminant {
        1 => 5500.0,  // Daylight
        2 => 4150.0,  // Fluorescent
        3 => 3400.0,  // Tungsten
        4 => 5500.0,  // Flash
        9 => 7000.0,  // Fine weather
        10 => 6500.0, // Cloudy weather
        11 => 5500.0, // Shade
        12 => 6430.0, // Daylight fluorescent (D 5700-7100K)
        13 => 4230.0, // Day white fluorescent (N 4600-5500K)
        14 => 3450.0, // Cool white fluorescent (W 3800-4500K)
        15 => 2940.0, // White fluorescent (WW 3250-3800K)
        17 => 2856.0, // Standard light A
        18 => 4874.0, // Standard light B
        19 => 6774.0, // Standard light C
        20 => 5503.0, // D55
        21 => 6504.0, // D65
        22 => 7504.0, // D75
        23 => 5003.0, // D50
        _ => 5500.0,  // Unknown → daylight
    }
}

/// Compute interpolation weight between two illuminants.
/// Uses reciprocal temperature interpolation per DNG spec.
/// Returns weight for illuminant 1 (0.0 = use illum2, 1.0 = use illum1).
pub fn dual_illuminant_weight(temp: f64, illum1: u16, illum2: u16) -> f64 {
    let t1 = illuminant_to_temp(illum1);
    let t2 = illuminant_to_temp(illum2);

    if (t1 - t2).abs() < 1.0 {
        return 0.5; // Same temperature, blend equally
    }

    // Interpolate in reciprocal temperature space
    let inv_t = 1.0 / temp.max(1.0);
    let inv_t1 = 1.0 / t1;
    let inv_t2 = 1.0 / t2;

    let g = (inv_t - inv_t2) / (inv_t1 - inv_t2);
    g.clamp(0.0, 1.0)
}

// ── Camera neutral → white point ────────────────────────────────────

/// Compute camera white point from AsShotNeutral and ColorMatrix.
///
/// AsShotNeutral gives the camera-space white point as ratios.
/// ColorMatrix maps XYZ → camera space, so we invert to find the
/// corresponding xy chromaticity.
pub fn neutral_to_xy(as_shot_neutral: &[f64], color_matrix: &Mat3) -> Option<(f64, f64)> {
    // Camera neutral → XYZ: invert ColorMatrix
    let cm_inv = mat3_invert(color_matrix)?;
    let neutral = if as_shot_neutral.len() >= 3 {
        [as_shot_neutral[0], as_shot_neutral[1], as_shot_neutral[2]]
    } else {
        return None;
    };
    let xyz = mat3_vec(&cm_inv, &neutral);

    // XYZ → xy
    let sum = xyz[0] + xyz[1] + xyz[2];
    if sum <= 0.0 {
        return None;
    }
    Some((xyz[0] / sum, xyz[1] / sum))
}

// ── Full camera-to-sRGB matrix computation ──────────────────────────

/// Compute the combined camera RGB → linear sRGB matrix.
///
/// This implements the ColorMatrix path (no ForwardMatrix):
/// 1. Apply Bradford adaptation from white point to D50
/// 2. Multiply by inverted ColorMatrix to get XYZ
/// 3. Convert XYZ to sRGB
///
/// Returns the 3×3 matrix that maps camera-space RGB to linear sRGB.
pub fn compute_camera_to_srgb(color_matrix: &Mat3, white_xy: (f64, f64)) -> Option<Mat3> {
    // Bradford adapt from camera white to D50 (PCS)
    let adapt = bradford_adapt(white_xy, D50_XY);

    // ColorMatrix maps XYZ → camera, so invert for camera → XYZ
    let cm_inv = mat3_invert(color_matrix)?;

    // Camera → XYZ (adapted to D50)
    let camera_to_xyz = mat3_mul(&adapt, &cm_inv);

    // XYZ (D50) → sRGB linear
    let srgb_from_xyz = mat3_invert(&SRGB_TO_XYZ_D50)?;

    Some(mat3_mul(&srgb_from_xyz, &camera_to_xyz))
}

/// Apply a 3×3 color matrix to interleaved RGB f32 pixel data.
pub fn apply_matrix_rgb(pixels: &mut [f32], matrix: &Mat3) {
    let m = [
        [
            matrix[0][0] as f32,
            matrix[0][1] as f32,
            matrix[0][2] as f32,
        ],
        [
            matrix[1][0] as f32,
            matrix[1][1] as f32,
            matrix[1][2] as f32,
        ],
        [
            matrix[2][0] as f32,
            matrix[2][1] as f32,
            matrix[2][2] as f32,
        ],
    ];

    let npix = pixels.len() / 3;
    for i in 0..npix {
        let base = i * 3;
        let r = pixels[base];
        let g = pixels[base + 1];
        let b = pixels[base + 2];
        pixels[base] = m[0][0] * r + m[0][1] * g + m[0][2] * b;
        pixels[base + 1] = m[1][0] * r + m[1][1] * g + m[1][2] * b;
        pixels[base + 2] = m[2][0] * r + m[2][1] * g + m[2][2] * b;
    }
}

/// Apply sRGB gamma encoding to linear f32 data, producing u8 output.
pub fn linear_to_srgb_u8(linear: &[f32]) -> Vec<u8> {
    let mut output = Vec::with_capacity(linear.len());
    for &v in linear {
        let v = v.clamp(0.0, 1.0);
        let srgb = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        output.push((srgb * 255.0 + 0.5) as u8);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mat3_invert_identity() {
        let id = mat3_identity();
        let inv = mat3_invert(&id).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((inv[i][j] - expected).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn test_bradford_d65_to_d50() {
        let adapt = bradford_adapt(D65_XY, D50_XY);
        // Verify it maps D65 white to D50 white
        let d65_xyz = xy_to_xyz(D65_XY.0, D65_XY.1);
        let mapped = mat3_vec(&adapt, &d65_xyz);
        let d50_xyz = xy_to_xyz(D50_XY.0, D50_XY.1);
        // Y should be preserved (both are Y=1)
        assert!((mapped[1] - d50_xyz[1]).abs() < 0.01);
    }

    #[test]
    fn test_dual_illuminant_weight() {
        // At illuminant 1 temperature → weight = 1.0
        let w = dual_illuminant_weight(2856.0, 17, 21); // StdA=2856, D65=6504
        assert!(w > 0.95, "at illum1 temp, weight should be ~1.0, got {w}");

        // At illuminant 2 temperature → weight = 0.0
        let w = dual_illuminant_weight(6504.0, 17, 21);
        assert!(w < 0.05, "at illum2 temp, weight should be ~0.0, got {w}");

        // Mid temperature → ~0.5
        let w = dual_illuminant_weight(4000.0, 17, 21);
        assert!(
            (0.2..0.8).contains(&w),
            "mid temp weight should be ~0.3-0.7, got {w}"
        );
    }

    #[test]
    fn test_neutral_to_xy_apple() {
        // Apple iPhone 16 Pro ColorMatrix1 and AsShotNeutral from our test file
        let cm = [
            [1.2272, -0.5455, -0.2613],
            [-0.4547, 1.5178, -0.0427],
            [-0.0409, 0.1636, 0.5913],
        ];
        let neutral = [0.4490, 1.0, 0.5409];
        let xy = neutral_to_xy(&neutral, &cm);
        assert!(xy.is_some());
        let (x, y) = xy.unwrap();
        eprintln!("Apple white point: ({x:.4}, {y:.4})");
        // Should be in reasonable range for a daylight-ish scene
        assert!((0.28..0.45).contains(&x), "x={x}");
        assert!((0.28..0.45).contains(&y), "y={y}");
    }
}
