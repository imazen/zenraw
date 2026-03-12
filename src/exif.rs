//! EXIF and DNG metadata extraction.
//!
//! Uses [`kamadak-exif`](https://crates.io/crates/kamadak-exif) for TIFF-based
//! RAW/DNG metadata reading. Provides structured access to camera info, color
//! matrices, and DNG-specific tags.
//!
//! Feature-gated behind `exif`.

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

use exif::{Context, In, Tag, Value};

/// EXIF metadata extracted from a RAW/DNG file.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ExifMetadata {
    // ── Camera info ──
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub date_time: Option<String>,

    // ── Exposure ──
    pub exposure_time: Option<(u32, u32)>,
    pub f_number: Option<(u32, u32)>,
    pub iso: Option<u32>,
    pub focal_length: Option<(u32, u32)>,
    pub lens_model: Option<String>,

    // ── Image ──
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u16>,
    pub bits_per_sample: Option<u16>,

    // ── GPS ──
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub gps_altitude: Option<f64>,

    // ── DNG-specific ──
    pub dng_version: Option<[u8; 4]>,
    pub unique_camera_model: Option<String>,
    pub color_matrix_1: Option<Vec<f64>>,
    pub color_matrix_2: Option<Vec<f64>>,
    pub forward_matrix_1: Option<Vec<f64>>,
    pub forward_matrix_2: Option<Vec<f64>>,
    pub as_shot_neutral: Option<Vec<f64>>,
    pub as_shot_white_xy: Option<(f64, f64)>,
    pub baseline_exposure: Option<f64>,
    pub calibration_illuminant_1: Option<u16>,
    pub calibration_illuminant_2: Option<u16>,
}

// ── DNG Tag constants ─────────────────────────────────────────────────

const DNG_VERSION: Tag = Tag(Context::Tiff, 0xC612);
const UNIQUE_CAMERA_MODEL: Tag = Tag(Context::Tiff, 0xC614);
const COLOR_MATRIX_1: Tag = Tag(Context::Tiff, 0xC621);
const COLOR_MATRIX_2: Tag = Tag(Context::Tiff, 0xC622);
const FORWARD_MATRIX_1: Tag = Tag(Context::Tiff, 0xC714);
const FORWARD_MATRIX_2: Tag = Tag(Context::Tiff, 0xC715);
const AS_SHOT_NEUTRAL: Tag = Tag(Context::Tiff, 0xC628);
const AS_SHOT_WHITE_XY: Tag = Tag(Context::Tiff, 0xC629);
const BASELINE_EXPOSURE: Tag = Tag(Context::Tiff, 0xC62A);
const CALIBRATION_ILLUMINANT_1: Tag = Tag(Context::Tiff, 0xC65A);
const CALIBRATION_ILLUMINANT_2: Tag = Tag(Context::Tiff, 0xC65B);

/// Extract EXIF and DNG metadata from file bytes.
///
/// Works with TIFF-based RAW formats (DNG, CR2, NEF, ARW, PEF, ORF, etc.).
pub fn read_metadata(data: &[u8]) -> Option<ExifMetadata> {
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(data))
        .ok()?;

    Some(ExifMetadata {
        // Camera info
        make: get_string(&exif, Tag::Make),
        model: get_string(&exif, Tag::Model),
        software: get_string(&exif, Tag::Software),
        date_time: get_string(&exif, Tag::DateTime),

        // Exposure
        exposure_time: get_rational(&exif, Tag::ExposureTime),
        f_number: get_rational(&exif, Tag::FNumber),
        iso: get_u32(&exif, Tag::PhotographicSensitivity),
        focal_length: get_rational(&exif, Tag::FocalLength),
        lens_model: get_string(&exif, Tag::LensModel),

        // Image dimensions
        width: get_u32(&exif, Tag::ImageWidth).or_else(|| get_u32(&exif, Tag::PixelXDimension)),
        height: get_u32(&exif, Tag::ImageLength).or_else(|| get_u32(&exif, Tag::PixelYDimension)),
        orientation: get_u16(&exif, Tag::Orientation),
        bits_per_sample: get_u16(&exif, Tag::BitsPerSample),

        // GPS
        gps_latitude: get_gps_coord(&exif, Tag::GPSLatitude, Tag::GPSLatitudeRef),
        gps_longitude: get_gps_coord(&exif, Tag::GPSLongitude, Tag::GPSLongitudeRef),
        gps_altitude: get_rational(&exif, Tag::GPSAltitude).map(|(n, d)| n as f64 / d as f64),

        // DNG
        dng_version: get_dng_version(&exif),
        unique_camera_model: get_string(&exif, UNIQUE_CAMERA_MODEL),
        color_matrix_1: get_srational_vec(&exif, COLOR_MATRIX_1),
        color_matrix_2: get_srational_vec(&exif, COLOR_MATRIX_2),
        forward_matrix_1: get_srational_vec(&exif, FORWARD_MATRIX_1),
        forward_matrix_2: get_srational_vec(&exif, FORWARD_MATRIX_2),
        as_shot_neutral: get_rational_vec(&exif, AS_SHOT_NEUTRAL),
        as_shot_white_xy: get_rational_xy(&exif, AS_SHOT_WHITE_XY),
        baseline_exposure: get_srational_f64(&exif, BASELINE_EXPOSURE),
        calibration_illuminant_1: get_u16(&exif, CALIBRATION_ILLUMINANT_1),
        calibration_illuminant_2: get_u16(&exif, CALIBRATION_ILLUMINANT_2),
    })
}

// ── Helper functions ──────────────────────────────────────────────────

fn get_string(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    Some(
        field
            .display_value()
            .to_string()
            .trim_matches('"')
            .to_string(),
    )
}

fn get_u32(exif: &exif::Exif, tag: Tag) -> Option<u32> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Long(v) => v.first().copied(),
        Value::Short(v) => v.first().map(|&x| x as u32),
        _ => None,
    }
}

fn get_u16(exif: &exif::Exif, tag: Tag) -> Option<u16> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Short(v) => v.first().copied(),
        Value::Long(v) => v.first().map(|&x| x as u16),
        _ => None,
    }
}

fn get_rational(exif: &exif::Exif, tag: Tag) -> Option<(u32, u32)> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Rational(v) => v.first().map(|r| (r.num, r.denom)),
        _ => None,
    }
}

fn get_rational_vec(exif: &exif::Exif, tag: Tag) -> Option<Vec<f64>> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Rational(v) => {
            let vals: Vec<f64> = v
                .iter()
                .map(|r| r.num as f64 / r.denom.max(1) as f64)
                .collect();
            if vals.is_empty() { None } else { Some(vals) }
        }
        _ => None,
    }
}

fn get_rational_xy(exif: &exif::Exif, tag: Tag) -> Option<(f64, f64)> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Rational(v) if v.len() >= 2 => {
            let x = v[0].num as f64 / v[0].denom.max(1) as f64;
            let y = v[1].num as f64 / v[1].denom.max(1) as f64;
            Some((x, y))
        }
        _ => None,
    }
}

fn get_srational_vec(exif: &exif::Exif, tag: Tag) -> Option<Vec<f64>> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::SRational(v) => {
            let vals: Vec<f64> = v
                .iter()
                .map(|r| r.num as f64 / r.denom.max(1) as f64)
                .collect();
            if vals.is_empty() { None } else { Some(vals) }
        }
        _ => None,
    }
}

fn get_srational_f64(exif: &exif::Exif, tag: Tag) -> Option<f64> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::SRational(v) => v.first().map(|r| r.num as f64 / r.denom.max(1) as f64),
        _ => None,
    }
}

fn get_dng_version(exif: &exif::Exif) -> Option<[u8; 4]> {
    let field = exif.get_field(DNG_VERSION, In::PRIMARY)?;
    match &field.value {
        Value::Byte(v) if v.len() >= 4 => Some([v[0], v[1], v[2], v[3]]),
        _ => None,
    }
}

fn get_gps_coord(exif: &exif::Exif, coord_tag: Tag, ref_tag: Tag) -> Option<f64> {
    let field = exif.get_field(coord_tag, In::PRIMARY)?;
    let rationals = match &field.value {
        Value::Rational(v) if v.len() >= 3 => v,
        _ => return None,
    };

    let deg = rationals[0].num as f64 / rationals[0].denom.max(1) as f64;
    let min = rationals[1].num as f64 / rationals[1].denom.max(1) as f64;
    let sec = rationals[2].num as f64 / rationals[2].denom.max(1) as f64;
    let mut coord = deg + min / 60.0 + sec / 3600.0;

    // Check reference direction (S/W are negative)
    if let Some(ref_field) = exif.get_field(ref_tag, In::PRIMARY) {
        let s = ref_field.display_value().to_string();
        if s.contains('S') || s.contains('W') {
            coord = -coord;
        }
    }

    Some(coord)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_dng_metadata() {
        // Try reading a FiveK DNG
        let dirs = ["/mnt/v/input/fivek/dng/"];
        for dir in &dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.filter_map(|e| e.ok()).take(3) {
                let path = entry.path();
                if !path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
                {
                    continue;
                }

                let data = std::fs::read(&path).unwrap();
                let meta = read_metadata(&data);
                assert!(
                    meta.is_some(),
                    "failed to read metadata from {}",
                    path.display()
                );

                let meta = meta.unwrap();
                eprintln!("File: {}", path.file_name().unwrap().to_str().unwrap());
                eprintln!("  Make: {:?}", meta.make);
                eprintln!("  Model: {:?}", meta.model);
                eprintln!("  DNG version: {:?}", meta.dng_version);
                eprintln!("  ColorMatrix1: {:?}", meta.color_matrix_1);
                eprintln!("  AsShotNeutral: {:?}", meta.as_shot_neutral);
                eprintln!("  ISO: {:?}", meta.iso);
                eprintln!("  Orientation: {:?}", meta.orientation);

                // DNG files should have DNG version
                assert!(meta.dng_version.is_some());
                // Should have at least a color matrix
                assert!(
                    meta.color_matrix_1.is_some(),
                    "DNG should have ColorMatrix1"
                );
                return; // One successful test is enough
            }
        }
        eprintln!("Skipping: no DNG files found for EXIF test");
    }
}
