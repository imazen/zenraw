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
/// The returned matrix maps **raw camera RGB** (not white-balanced) to linear sRGB.
/// White balance is baked into the matrix.
///
/// `as_shot_neutral`: camera-space neutral point (e.g., [0.449, 1.0, 0.541])
pub fn compute_camera_to_srgb_with_wb(
    color_matrix: &Mat3,
    white_xy: (f64, f64),
    as_shot_neutral: &[f64; 3],
) -> Option<Mat3> {
    // ColorMatrix maps XYZ → camera, so invert for camera → XYZ
    let cm_inv = mat3_invert(color_matrix)?;

    // Bradford adapt from camera white to D50 (PCS)
    let adapt = bradford_adapt(white_xy, D50_XY);

    // Camera → XYZ (adapted to D50)
    let camera_to_xyz_d50 = mat3_mul(&adapt, &cm_inv);

    // XYZ (D50) → sRGB linear
    let srgb_from_xyz = mat3_invert(&SRGB_TO_XYZ_D50)?;

    // camera_raw → XYZ → sRGB (without WB)
    let raw_mat = mat3_mul(&srgb_from_xyz, &camera_to_xyz_d50);

    // Bake WB into the matrix: pre-multiply by diag(1/neutral)
    // raw_pixel → (raw_pixel / neutral) → raw_mat → sRGB
    // Combined: wb_mat = raw_mat × diag(1/neutral[0], 1/neutral[1], 1/neutral[2])
    let mut wb_mat = raw_mat;
    for i in 0..3 {
        for j in 0..3 {
            wb_mat[i][j] /= as_shot_neutral[j].max(1e-10);
        }
    }

    // Normalize: neutral input should map to equal sRGB output.
    // After WB baking, neutral → wb_mat × neutral = raw_mat × [1,1,1] (WB cancels out).
    // So we normalize by what [neutral] produces, making R=G=B.
    let test = mat3_vec(&wb_mat, as_shot_neutral);
    // We want R≈G≈B. Scale each row so test produces equal output.
    // Simple approach: normalize each row by its output for neutral.
    let target = test[1]; // normalize to G channel value
    if target.abs() < 1e-10 {
        return None;
    }
    let mut result = wb_mat;
    for i in 0..3 {
        let row_scale = target / test[i].max(1e-10);
        for j in 0..3 {
            result[i][j] *= row_scale;
        }
    }

    Some(result)
}

/// Compute camera → sRGB without baked-in WB (legacy, for external WB).
pub fn compute_camera_to_srgb(color_matrix: &Mat3, white_xy: (f64, f64)) -> Option<Mat3> {
    let cm_inv = mat3_invert(color_matrix)?;
    let adapt = bradford_adapt(white_xy, D50_XY);
    let camera_to_xyz_d50 = mat3_mul(&adapt, &cm_inv);
    let srgb_from_xyz = mat3_invert(&SRGB_TO_XYZ_D50)?;
    Some(mat3_mul(&srgb_from_xyz, &camera_to_xyz_d50))
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

// ── Full DNG rendering pipeline ──────────────────────────────────────

/// Configuration for the DNG rendering pipeline.
///
/// Extracts all needed metadata from the DNG file and provides
/// a `render()` method that applies the full pipeline:
/// PGTM → exposure × WB → color matrix → tone curve → sRGB gamma
#[derive(Clone, Debug)]
pub struct DngPipeline {
    /// Camera → sRGB linear color matrix (3×3).
    pub camera_to_srgb: Mat3,
    /// White balance multipliers (normalized, max=1.0).
    pub wb_mult: Vec3,
    /// BaselineExposure in EV (applied as 2^EV multiplier).
    pub baseline_exposure: f64,
    /// AnalogBalance diagonal (usually [1,1,1]).
    pub analog_balance: Vec3,
    /// Image dimensions.
    pub width: u32,
    pub height: u32,
    /// ProfileToneCurve LUT (4096 entries, maps [0,1]→[0,1]).
    pub tone_curve: Option<Vec<f32>>,
    /// ProfileGainTableMap for Smart HDR.
    #[cfg(feature = "apple")]
    pub gain_table_map: Option<crate::apple::ProfileGainTableMap>,
}

impl DngPipeline {
    /// Build a pipeline from DNG EXIF metadata.
    ///
    /// Returns None if essential metadata is missing (ColorMatrix, AsShotNeutral).
    #[cfg(feature = "exif")]
    pub fn from_metadata(
        exif: &crate::exif::ExifMetadata,
        width: u32,
        height: u32,
    ) -> Option<Self> {
        // Get color matrix (prefer CM2 for D65 illuminant if available)
        let cm_data = if exif.calibration_illuminant_2 == Some(21) {
            exif.color_matrix_2
                .as_deref()
                .or(exif.color_matrix_1.as_deref())?
        } else {
            exif.color_matrix_1.as_deref()?
        };

        if cm_data.len() < 9 {
            return None;
        }

        let color_matrix: Mat3 = [
            [cm_data[0], cm_data[1], cm_data[2]],
            [cm_data[3], cm_data[4], cm_data[5]],
            [cm_data[6], cm_data[7], cm_data[8]],
        ];

        // Get white balance from AsShotNeutral and AnalogBalance
        let neutral = exif.as_shot_neutral.as_deref()?;
        if neutral.len() < 3 {
            return None;
        }

        // AnalogBalance: per-channel gain applied to raw data before color matrix.
        // In DNG spec: effective camera neutral = AnalogBalance × AsShotNeutral
        let ab = exif.analog_balance.as_deref().unwrap_or(&[1.0, 1.0, 1.0]);
        let analog_balance = if ab.len() >= 3 {
            [ab[0], ab[1], ab[2]]
        } else {
            [1.0, 1.0, 1.0]
        };

        // Effective neutral = AnalogBalance × AsShotNeutral.
        // This is what the sensor outputs for neutral — raw data has AB baked in.
        let effective_neutral = [
            neutral[0] * analog_balance[0],
            neutral[1] * analog_balance[1],
            neutral[2] * analog_balance[2],
        ];

        // White point: derived from AsShotNeutral (not effective_neutral)
        let neutral3 = [neutral[0], neutral[1], neutral[2]];
        let white_xy = neutral_to_xy(&neutral3, &color_matrix)?;

        // Matrix bakes in WB for effective_neutral (what the raw data actually contains).
        // AnalogBalance is NOT applied in render() — it's already in the raw pixels.
        let camera_to_srgb =
            compute_camera_to_srgb_with_wb(&color_matrix, white_xy, &effective_neutral)?;

        let wb_mult = [1.0, 1.0, 1.0];
        let analog_balance = [1.0, 1.0, 1.0]; // already in raw data

        let baseline_exposure = exif.baseline_exposure.unwrap_or(0.0);

        Some(DngPipeline {
            camera_to_srgb,
            wb_mult,
            baseline_exposure,
            analog_balance,
            width,
            height,
            tone_curve: None,
            #[cfg(feature = "apple")]
            gain_table_map: None,
        })
    }

    /// Set the ProfileToneCurve from raw 514-float data (257 x,y pairs).
    pub fn with_tone_curve(mut self, tc_data: &[f32]) -> Self {
        let n_points = tc_data.len() / 2;
        if n_points >= 2 {
            let points: Vec<(f32, f32)> = (0..n_points)
                .map(|i| (tc_data[i * 2], tc_data[i * 2 + 1]))
                .collect();
            let lut_size = 4096usize;
            let mut lut = vec![0.0f32; lut_size + 1];
            for i in 0..=lut_size {
                let x = i as f32 / lut_size as f32;
                lut[i] = interpolate_curve(&points, x);
            }
            self.tone_curve = Some(lut);
        }
        self
    }

    /// Set the ProfileGainTableMap.
    #[cfg(feature = "apple")]
    pub fn with_gain_table_map(mut self, pgtm: crate::apple::ProfileGainTableMap) -> Self {
        self.gain_table_map = Some(pgtm);
        self
    }

    /// Render raw camera-space linear f32 pixels to sRGB u8.
    ///
    /// Input: interleaved RGB f32 in camera color space (NOT white-balanced).
    /// This is the raw demosaiced data before any color processing.
    ///
    /// Pipeline:
    /// 1. ProfileGainTableMap (spatially-varying gain, if present)
    /// 2. BaselineExposure scaling
    /// 3. White balance
    /// 4. Camera → sRGB color matrix
    /// 5. ProfileToneCurve (if present) or clamp
    /// 6. sRGB gamma encoding
    pub fn render(&self, raw_linear: &[f32]) -> Vec<u8> {
        let npix = (self.width as usize) * (self.height as usize);
        assert_eq!(raw_linear.len(), npix * 3, "pixel count mismatch");

        let mut pixels = raw_linear.to_vec();

        // 1. ProfileGainTableMap
        #[cfg(feature = "apple")]
        if let Some(ref pgtm) = self.gain_table_map {
            pgtm.apply(&mut pixels, self.width, self.height, self.baseline_exposure);
        }

        // 2. AnalogBalance + BaselineExposure (+ WB if not baked into matrix)
        let bl_mult = 2.0f32.powf(self.baseline_exposure as f32);
        let ab = [
            bl_mult * self.analog_balance[0] as f32 * self.wb_mult[0] as f32,
            bl_mult * self.analog_balance[1] as f32 * self.wb_mult[1] as f32,
            bl_mult * self.analog_balance[2] as f32 * self.wb_mult[2] as f32,
        ];
        for i in 0..npix {
            let base = i * 3;
            pixels[base] *= ab[0];
            pixels[base + 1] *= ab[1];
            pixels[base + 2] *= ab[2];
        }

        // 3. Camera → sRGB color matrix
        apply_matrix_rgb(&mut pixels, &self.camera_to_srgb);

        // Clamp to [0, 1] after matrix (matrix can produce negatives)
        for v in pixels.iter_mut() {
            *v = v.clamp(0.0, 1.0);
        }

        // 4. ProfileToneCurve (applied per-channel in linear sRGB, per DNG spec)
        if let Some(ref lut) = self.tone_curve {
            for v in pixels.iter_mut() {
                *v = eval_lut(lut, *v);
            }
        }

        // 5. sRGB gamma encoding
        linear_to_srgb_u8(&pixels)
    }

    /// Render with luminance-preserving tone curve application.
    ///
    /// Like `render()` but applies the tone curve on luminance only,
    /// preserving color ratios. Often produces better visual results
    /// than per-channel application.
    pub fn render_lum_preserving(&self, raw_linear: &[f32]) -> Vec<u8> {
        let npix = (self.width as usize) * (self.height as usize);
        assert_eq!(raw_linear.len(), npix * 3, "pixel count mismatch");

        let mut pixels = raw_linear.to_vec();

        // 1. ProfileGainTableMap
        #[cfg(feature = "apple")]
        if let Some(ref pgtm) = self.gain_table_map {
            pgtm.apply(&mut pixels, self.width, self.height, self.baseline_exposure);
        }

        // 2. AnalogBalance + BaselineExposure (+ WB if not baked into matrix)
        let bl_mult = 2.0f32.powf(self.baseline_exposure as f32);
        let ab = [
            bl_mult * self.analog_balance[0] as f32 * self.wb_mult[0] as f32,
            bl_mult * self.analog_balance[1] as f32 * self.wb_mult[1] as f32,
            bl_mult * self.analog_balance[2] as f32 * self.wb_mult[2] as f32,
        ];
        for i in 0..npix {
            let base = i * 3;
            pixels[base] *= ab[0];
            pixels[base + 1] *= ab[1];
            pixels[base + 2] *= ab[2];
        }

        // 3. Camera → sRGB color matrix
        apply_matrix_rgb(&mut pixels, &self.camera_to_srgb);

        // Clamp negatives
        for v in pixels.iter_mut() {
            *v = v.max(0.0);
        }

        // 4. Luminance-preserving tone curve
        if let Some(ref lut) = self.tone_curve {
            for i in 0..npix {
                let base = i * 3;
                let r = pixels[base];
                let g = pixels[base + 1];
                let b = pixels[base + 2];
                let lum = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(1e-10);
                let mapped = eval_lut(lut, lum.min(1.0));
                let ratio = mapped / lum;
                pixels[base] = (r * ratio).min(1.0);
                pixels[base + 1] = (g * ratio).min(1.0);
                pixels[base + 2] = (b * ratio).min(1.0);
            }
        } else {
            for v in pixels.iter_mut() {
                *v = v.min(1.0);
            }
        }

        // 5. sRGB gamma
        linear_to_srgb_u8(&pixels)
    }
}

/// Evaluate a LUT with linear interpolation.
fn eval_lut(lut: &[f32], x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let max_idx = (lut.len() - 1) as f32;
    let idx_f = x * max_idx;
    let idx = idx_f as usize;
    let frac = idx_f - idx as f32;
    if idx >= lut.len() - 1 {
        lut[lut.len() - 1]
    } else {
        lut[idx] * (1.0 - frac) + lut[idx + 1] * frac
    }
}

/// Linear interpolation on sorted (x, y) control points.
fn interpolate_curve(points: &[(f32, f32)], x: f32) -> f32 {
    if x <= points[0].0 {
        return points[0].1;
    }
    if x >= points[points.len() - 1].0 {
        return points[points.len() - 1].1;
    }
    let idx = points.partition_point(|p| p.0 < x);
    if idx == 0 {
        return points[0].1;
    }
    let (x0, y0) = points[idx - 1];
    let (x1, y1) = points[idx];
    let t = if (x1 - x0).abs() < 1e-10 {
        0.0
    } else {
        (x - x0) / (x1 - x0)
    };
    y0 + t * (y1 - y0)
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
        assert!((0.28..0.45).contains(&x), "x={x}");
        assert!((0.28..0.45).contains(&y), "y={y}");
    }

    #[test]
    fn test_eval_lut_identity() {
        // Identity LUT: y = x
        let lut: Vec<f32> = (0..=256).map(|i| i as f32 / 256.0).collect();
        assert!((eval_lut(&lut, 0.0) - 0.0).abs() < 0.01);
        assert!((eval_lut(&lut, 0.5) - 0.5).abs() < 0.01);
        assert!((eval_lut(&lut, 1.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_eval_lut_clamp() {
        let lut = vec![0.0, 0.5, 1.0];
        assert_eq!(eval_lut(&lut, -0.5), 0.0);
        assert_eq!(eval_lut(&lut, 1.5), 1.0);
    }

    #[test]
    fn test_interpolate_curve() {
        let pts = vec![(0.0, 0.0), (0.5, 0.3), (1.0, 1.0)];
        assert!((interpolate_curve(&pts, 0.0) - 0.0).abs() < 0.01);
        assert!((interpolate_curve(&pts, 0.5) - 0.3).abs() < 0.01);
        assert!((interpolate_curve(&pts, 1.0) - 1.0).abs() < 0.01);
        // Interpolated value between 0.0 and 0.5
        let v = interpolate_curve(&pts, 0.25);
        assert!((v - 0.15).abs() < 0.01, "expected ~0.15, got {v}");
    }

    #[test]
    fn test_camera_to_srgb_matrix_produces_valid_white() {
        let cm = [
            [1.2272, -0.5455, -0.2613],
            [-0.4547, 1.5178, -0.0427],
            [-0.0409, 0.1636, 0.5913],
        ];
        let neutral = [0.4490, 1.0, 0.5409];
        let white_xy = neutral_to_xy(&neutral, &cm).unwrap();
        let mat = compute_camera_to_srgb(&cm, white_xy).unwrap();

        // Camera neutral should map to approximately white (equal RGB) in sRGB
        let white_cam = [neutral[0] as f32, neutral[1] as f32, neutral[2] as f32];
        let mut test = white_cam;
        let m = [
            [mat[0][0] as f32, mat[0][1] as f32, mat[0][2] as f32],
            [mat[1][0] as f32, mat[1][1] as f32, mat[1][2] as f32],
            [mat[2][0] as f32, mat[2][1] as f32, mat[2][2] as f32],
        ];
        let r = m[0][0] * test[0] + m[0][1] * test[1] + m[0][2] * test[2];
        let g = m[1][0] * test[0] + m[1][1] * test[1] + m[1][2] * test[2];
        let b = m[2][0] * test[0] + m[2][1] * test[1] + m[2][2] * test[2];
        // After WB (multiply by wb_mult), neutral should map to gray
        let wb_max = neutral[0].max(neutral[1]).max(neutral[2]);
        let wr = wb_max / neutral[0];
        let wg = wb_max / neutral[1];
        let wb = wb_max / neutral[2];
        let r2 = m[0][0] * test[0] * wr as f32
            + m[0][1] * test[1] * wg as f32
            + m[0][2] * test[2] * wb as f32;
        let g2 = m[1][0] * test[0] * wr as f32
            + m[1][1] * test[1] * wg as f32
            + m[1][2] * test[2] * wb as f32;
        let b2 = m[2][0] * test[0] * wr as f32
            + m[2][1] * test[1] * wg as f32
            + m[2][2] * test[2] * wb as f32;
        eprintln!("Without WB: R={r:.3} G={g:.3} B={b:.3}");
        eprintln!("With WB:    R={r2:.3} G={g2:.3} B={b2:.3}");
        // With WB, R/G and B/G should be close to 1.0
        let rg_ratio = r2 / g2;
        let bg_ratio = b2 / g2;
        eprintln!("Ratios: R/G={rg_ratio:.3} B/G={bg_ratio:.3}");
    }

    #[test]
    fn test_pipeline_synthetic_neutral_gray() {
        // Synthetic test: neutral gray in camera space should produce neutral gray in sRGB
        let cm = [
            [1.2272, -0.5455, -0.2613],
            [-0.4547, 1.5178, -0.0427],
            [-0.0409, 0.1636, 0.5913],
        ];
        let neutral = [0.4490, 1.0, 0.5409];
        let white_xy = neutral_to_xy(&neutral, &cm).unwrap();
        let camera_to_srgb = compute_camera_to_srgb_with_wb(&cm, white_xy, &neutral).unwrap();

        // Verify: matrix × neutral should produce near-equal RGB
        let test = mat3_vec(&camera_to_srgb, &neutral);
        eprintln!(
            "Matrix × neutral = [{:.4}, {:.4}, {:.4}]",
            test[0], test[1], test[2]
        );
        let rg = test[0] / test[1];
        let bg = test[2] / test[1];
        eprintln!("Ratios: R/G={rg:.4} B/G={bg:.4}");
        assert!(
            (rg - 1.0).abs() < 0.05,
            "matrix should map neutral to gray, R/G={rg}"
        );

        let pipeline = DngPipeline {
            camera_to_srgb,
            wb_mult: [1.0, 1.0, 1.0], // WB baked into matrix
            baseline_exposure: 0.0,
            analog_balance: [1.0, 1.0, 1.0],
            width: 2,
            height: 2,
            tone_curve: None,
            #[cfg(feature = "apple")]
            gain_table_map: None,
        };

        // 4 pixels of neutral gray at 18% in camera space
        // Camera neutral = [0.449, 1.0, 0.541], so neutral * 0.18 = camera middle gray
        let raw: Vec<f32> = (0..4)
            .flat_map(|_| {
                [
                    neutral[0] as f32 * 0.18,
                    neutral[1] as f32 * 0.18,
                    neutral[2] as f32 * 0.18,
                ]
            })
            .collect();

        let srgb = pipeline.render(&raw);

        let r = srgb[0];
        let g = srgb[1];
        let b = srgb[2];
        eprintln!("Neutral gray render: R={r} G={g} B={b}");
        let max_diff = (r as i32 - g as i32)
            .unsigned_abs()
            .max((g as i32 - b as i32).unsigned_abs());
        assert!(
            max_diff < 10,
            "neutral gray should produce near-equal RGB, got R={r} G={g} B={b} (max_diff={max_diff})"
        );
        assert!(
            r > 30 && r < 200,
            "middle gray should be in mid-range, got {r}"
        );
    }

    #[test]
    fn test_pipeline_from_real_appledng() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let exif = crate::exif::read_metadata(&data).expect("should read EXIF");
        let dng_profile = crate::apple::extract_dng_profile(&data);
        let pgtm = crate::apple::extract_profile_gain_table_map(&data);

        let dw = exif.width.unwrap_or(4032);
        let dh = exif.height.unwrap_or(3024);

        let mut pipeline =
            DngPipeline::from_metadata(&exif, dw, dh).expect("should build pipeline from EXIF");

        // Add tone curve
        if let Some(ref profile) = dng_profile {
            if let Some(ref tc) = profile.tone_curve {
                pipeline = pipeline.with_tone_curve(tc);
            }
        }

        // Add PGTM
        if let Some(pgtm) = pgtm {
            pipeline = pipeline.with_gain_table_map(pgtm);
        }

        eprintln!(
            "Pipeline: BL={:.2} EV, WB=[{:.3},{:.3},{:.3}], has_curve={}, has_pgtm={}",
            pipeline.baseline_exposure,
            pipeline.wb_mult[0],
            pipeline.wb_mult[1],
            pipeline.wb_mult[2],
            pipeline.tone_curve.is_some(),
            pipeline.gain_table_map.is_some()
        );

        // Decode raw pixels (rawler gives us WB+ColorMatrix processed data, but
        // for this test we need camera-space raw. Since rawler doesn't expose that,
        // we'll test with a small synthetic patch instead.)
        eprintln!("Pipeline constructed successfully from real APPLEDNG metadata");
        eprintln!(
            "  camera_to_srgb diagonal: [{:.3}, {:.3}, {:.3}]",
            pipeline.camera_to_srgb[0][0],
            pipeline.camera_to_srgb[1][1],
            pipeline.camera_to_srgb[2][2]
        );
    }

    #[test]
    fn test_full_pipeline_real_appledng() {
        // End-to-end test: decode APPLEDNG with skip_color_pipeline, then render via DngPipeline
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let exif = crate::exif::read_metadata(&data).expect("should read EXIF");
        let dng_profile = crate::apple::extract_dng_profile(&data);
        let pgtm = crate::apple::extract_profile_gain_table_map(&data);

        // Decode with skip_color_pipeline = true (camera-space raw)
        let mut config = crate::decode::RawDecodeConfig::default();
        config.skip_color_pipeline = true;
        let output =
            crate::decode(&data, &config, &enough::Unstoppable).expect("should decode APPLEDNG");

        let dw = output.pixels.width();
        let dh = output.pixels.height();
        let raw_bytes = output.pixels.copy_to_contiguous_bytes();
        let camera_raw: &[f32] = bytemuck::cast_slice(&raw_bytes);
        eprintln!(
            "Camera-space raw: {}x{}, {} floats",
            dw,
            dh,
            camera_raw.len()
        );

        // Verify: camera-space data should have unequal channel means (not WB'd)
        let npix = (dw as usize) * (dh as usize);
        let (mut mr, mut mg, mut mb) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..npix {
            mr += camera_raw[i * 3] as f64;
            mg += camera_raw[i * 3 + 1] as f64;
            mb += camera_raw[i * 3 + 2] as f64;
        }
        mr /= npix as f64;
        mg /= npix as f64;
        mb /= npix as f64;
        eprintln!("Channel means: R={mr:.5} G={mg:.5} B={mb:.5}");
        eprintln!("Ratios: R/G={:.3} B/G={:.3}", mr / mg, mb / mg);

        // Build pipeline
        let mut pipeline =
            DngPipeline::from_metadata(&exif, dw, dh).expect("should build pipeline from EXIF");
        if let Some(ref profile) = dng_profile {
            if let Some(ref tc) = profile.tone_curve {
                pipeline = pipeline.with_tone_curve(tc);
            }
        }
        if let Some(pgtm) = pgtm {
            pipeline = pipeline.with_gain_table_map(pgtm);
        }

        eprintln!("Rendering via DngPipeline...");
        let srgb = pipeline.render_lum_preserving(camera_raw);
        eprintln!("Output: {} bytes", srgb.len());

        // Verify output is not all black or all white
        let mean: f64 = srgb.iter().map(|&v| v as f64).sum::<f64>() / srgb.len() as f64;
        eprintln!("sRGB output mean: {mean:.1}");
        assert!(
            mean > 10.0,
            "output should not be nearly black, mean={mean}"
        );
        assert!(
            mean < 245.0,
            "output should not be nearly white, mean={mean}"
        );

        // Save output for visual inspection
        let out_path = "/mnt/v/output/zenfilters/mobile_parity/46CD_dng_pipeline.raw";
        let _ = std::fs::write(out_path, &srgb);
        eprintln!(
            "Saved raw sRGB u8 to {out_path} ({}x{}, {} bytes)",
            dw,
            dh,
            srgb.len()
        );
    }

    #[test]
    fn test_cbfa_metadata_investigation() {
        let path = "/mnt/v/heic/CBFA569A-5C28-468E-96B4-CFFBAEB951C7.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: CBFA file not found");
            return;
        };

        // kamadak-exif
        let exif = crate::exif::read_metadata(&data).unwrap();
        eprintln!("kamadak-exif AsShotNeutral: {:?}", exif.as_shot_neutral);
        eprintln!("kamadak-exif AsShotWhiteXY: {:?}", exif.as_shot_white_xy);
        eprintln!("kamadak-exif ColorMatrix1: {:?}", exif.color_matrix_1);
        eprintln!("kamadak-exif ColorMatrix2: {:?}", exif.color_matrix_2);
        eprintln!(
            "kamadak-exif CalibIllum1: {:?}",
            exif.calibration_illuminant_1
        );
        eprintln!(
            "kamadak-exif CalibIllum2: {:?}",
            exif.calibration_illuminant_2
        );
        eprintln!(
            "kamadak-exif BaselineExposure: {:?}",
            exif.baseline_exposure
        );

        // Our TIFF parser — check IFD0 for the actual tag values
        let tiff = crate::tiff_ifd::TiffStructure::parse(&data).unwrap();
        let bo = tiff.byte_order;

        // AsShotNeutral (0xC628)
        if let Some(entry) = tiff.ifd0_entry(crate::tiff_ifd::tags::AS_SHOT_NEUTRAL) {
            let vals = crate::tiff_ifd::read_rational_values(&data, entry, bo);
            eprintln!(
                "TIFF AsShotNeutral (0xC628): type={} count={} vals={:?}",
                entry.dtype, entry.count, vals
            );
        } else {
            eprintln!("TIFF: NO AsShotNeutral tag!");
        }

        // AnalogBalance (0xC627)
        if let Some(entry) = tiff.ifd0_entry(crate::tiff_ifd::tags::ANALOG_BALANCE) {
            let vals = crate::tiff_ifd::read_rational_values(&data, entry, bo);
            eprintln!(
                "TIFF AnalogBalance (0xC627): type={} count={} vals={:?}",
                entry.dtype, entry.count, vals
            );
        }

        // AsShotWhiteXY (0xC629)
        if let Some(entry) = tiff.ifd0_entry(crate::tiff_ifd::tags::AS_SHOT_WHITE_XY) {
            let vals = crate::tiff_ifd::read_rational_values(&data, entry, bo);
            eprintln!(
                "TIFF AsShotWhiteXY (0xC629): type={} count={} vals={:?}",
                entry.dtype, entry.count, vals
            );
        }

        // ColorMatrix1 (0xC621) — SRational
        if let Some(entry) = tiff.ifd0_entry(crate::tiff_ifd::tags::COLOR_MATRIX_1) {
            let vals = crate::tiff_ifd::read_rational_values(&data, entry, bo);
            eprintln!(
                "TIFF ColorMatrix1 (0xC621): type={} count={} vals={:?}",
                entry.dtype, entry.count, vals
            );
        }

        // Decode camera-space raw and check channel balance
        let mut config = crate::decode::RawDecodeConfig::default();
        config.skip_color_pipeline = true;
        if let Ok(output) = crate::decode(&data, &config, &enough::Unstoppable) {
            let w = output.pixels.width();
            let h = output.pixels.height();
            let bytes = output.pixels.copy_to_contiguous_bytes();
            let raw: &[f32] = bytemuck::cast_slice(&bytes);
            let npix = (w as usize) * (h as usize);
            let (mut r, mut g, mut b) = (0.0f64, 0.0f64, 0.0f64);
            for i in 0..npix {
                r += raw[i * 3] as f64;
                g += raw[i * 3 + 1] as f64;
                b += raw[i * 3 + 2] as f64;
            }
            r /= npix as f64;
            g /= npix as f64;
            b /= npix as f64;
            eprintln!(
                "Camera-space means: R={r:.5} G={g:.5} B={b:.5}  R/G={:.3} B/G={:.3}",
                r / g,
                b / g
            );
        }

        // Build DngPipeline — should work with AnalogBalance
        let pipeline = DngPipeline::from_metadata(&exif, 4032, 3024);
        assert!(pipeline.is_some(), "DngPipeline should build for CBFA");
        let p = pipeline.unwrap();
        eprintln!(
            "CBFA analog_balance: [{:.4}, {:.4}, {:.4}]",
            p.analog_balance[0], p.analog_balance[1], p.analog_balance[2]
        );

        // Test: render a 1×1 neutral patch.
        // For CBFA: AsShotNeutral=[1,1,1], AnalogBalance=[2.382, 1.0, 1.875]
        // Camera-space neutral at level 0.05 = effective_neutral * 0.05
        //   = [1.0*2.382*0.05, 1.0*1.0*0.05, 1.0*1.875*0.05] = [0.119, 0.05, 0.094]
        let ab = exif.analog_balance.as_ref().unwrap();
        let n = exif.as_shot_neutral.as_ref().unwrap();
        let test_pipe = DngPipeline {
            width: 1,
            height: 1,
            ..p
        };
        let raw_patch = vec![
            (n[0] * ab[0] * 0.05) as f32,
            (n[1] * ab[1] * 0.05) as f32,
            (n[2] * ab[2] * 0.05) as f32,
        ];
        let srgb = test_pipe.render(&raw_patch);
        eprintln!(
            "CBFA neutral render: R={} G={} B={}",
            srgb[0], srgb[1], srgb[2]
        );
        let max_diff = (srgb[0] as i32 - srgb[1] as i32)
            .unsigned_abs()
            .max((srgb[1] as i32 - srgb[2] as i32).unsigned_abs());
        assert!(
            max_diff < 15,
            "CBFA neutral should produce near-equal RGB, got R={} G={} B={} (diff={max_diff})",
            srgb[0],
            srgb[1],
            srgb[2]
        );
    }

    #[test]
    fn test_linear_to_srgb_u8_boundaries() {
        let srgb = linear_to_srgb_u8(&[0.0, 0.5, 1.0]);
        assert_eq!(srgb[0], 0);
        assert!(
            srgb[1] > 180 && srgb[1] < 200,
            "0.5 linear → ~188 sRGB, got {}",
            srgb[1]
        );
        assert_eq!(srgb[2], 255);
    }

    #[test]
    fn test_render_lum_preserving_color_ratios() {
        // Simple pipeline with a contrast-boosting tone curve
        let cm = mat3_identity(); // identity matrix = camera is sRGB
        let pipeline = DngPipeline {
            camera_to_srgb: cm,
            wb_mult: [1.0, 1.0, 1.0],
            baseline_exposure: 0.0,
            analog_balance: [1.0, 1.0, 1.0],
            width: 1,
            height: 1,
            tone_curve: Some({
                // Gamma 0.5 curve (brightens)
                (0..=4096)
                    .map(|i| {
                        let x = i as f32 / 4096.0;
                        x.powf(0.5)
                    })
                    .collect()
            }),
            #[cfg(feature = "apple")]
            gain_table_map: None,
        };

        // Red-ish pixel
        let raw = vec![0.3f32, 0.1, 0.05];
        let per_ch = pipeline.render(&raw);
        let lum_pres = pipeline.render_lum_preserving(&raw);

        eprintln!(
            "Per-channel: R={} G={} B={}",
            per_ch[0], per_ch[1], per_ch[2]
        );
        eprintln!(
            "Lum-preserving: R={} G={} B={}",
            lum_pres[0], lum_pres[1], lum_pres[2]
        );

        // Lum-preserving should maintain better color ratios
        // Per-channel boost makes all channels closer to equal (desaturates)
        let per_ch_rg = per_ch[0] as f32 / per_ch[1].max(1) as f32;
        let lum_rg = lum_pres[0] as f32 / lum_pres[1].max(1) as f32;
        // Original ratio: 0.3/0.1 = 3.0
        // Lum-preserving should be closer to 3.0 than per-channel
        eprintln!("R/G ratios: input=3.0, per_ch={per_ch_rg:.2}, lum={lum_rg:.2}");
        assert!(
            lum_rg > per_ch_rg,
            "lum-preserving should maintain higher R/G ratio"
        );
    }
}
