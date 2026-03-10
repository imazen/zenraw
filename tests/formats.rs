//! Per-format integration tests using raw.pixls.us sample corpus.
//!
//! Each test exercises a specific RAW format with probe, decode, and
//! optional EXIF extraction. Tests are skipped if the sample file is
//! not present (run `just fetch-samples` first).

use enough::Unstoppable;
use zenraw::{RawDecodeConfig, RawDecodeOutput};

const SAMPLES_DIR: &str = "/mnt/v/input/raw-samples";

/// Try to read a sample file, returning None if it doesn't exist.
fn load_sample(name: &str) -> Option<Vec<u8>> {
    let path = format!("{SAMPLES_DIR}/{name}");
    std::fs::read(&path).ok()
}

/// Decode with default config (scene-referred linear f32).
fn decode_linear(data: &[u8]) -> RawDecodeOutput {
    let config = RawDecodeConfig::default();
    zenraw::decode(data, &config, &Unstoppable).expect("decode should succeed")
}

/// Decode with sRGB gamma (display-referred u8).
fn decode_srgb(data: &[u8]) -> RawDecodeOutput {
    let config = RawDecodeConfig::new().with_gamma(true);
    zenraw::decode(data, &config, &Unstoppable).expect("sRGB decode should succeed")
}

/// Verify basic output sanity: non-zero dimensions, reasonable pixel values.
fn verify_output(output: &RawDecodeOutput, format_name: &str) {
    assert!(output.info.width > 0, "{format_name}: zero width");
    assert!(output.info.height > 0, "{format_name}: zero height");
    assert!(
        !output.info.make.is_empty(),
        "{format_name}: empty camera make"
    );
    assert!(
        !output.info.model.is_empty(),
        "{format_name}: empty camera model"
    );

    let bytes = output.pixels.copy_to_contiguous_bytes();
    assert!(!bytes.is_empty(), "{format_name}: empty pixel data");

    eprintln!(
        "  {format_name}: {}x{} {} {} (CFA={}, orientation={})",
        output.info.width,
        output.info.height,
        output.info.make,
        output.info.model,
        output.info.cfa_pattern,
        output.info.orientation
    );
}

/// Verify linear f32 output has reasonable pixel statistics.
fn verify_linear_stats(output: &RawDecodeOutput, format_name: &str) {
    assert_eq!(
        output.pixels.descriptor(),
        zenpixels::PixelDescriptor::RGBF32_LINEAR,
        "{format_name}: expected RGBF32_LINEAR"
    );

    let bytes = output.pixels.copy_to_contiguous_bytes();
    let floats: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let mean: f32 = floats.iter().sum::<f32>() / floats.len() as f32;
    let max: f32 = floats.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let min: f32 = floats.iter().cloned().fold(f32::INFINITY, f32::min);

    eprintln!("  {format_name} stats: mean={mean:.4} min={min:.4} max={max:.4}");

    assert!(
        mean > 0.001,
        "{format_name}: output is all black (mean={mean})"
    );
    assert!(max <= 1.0, "{format_name}: values exceed 1.0 (max={max})");
    assert!(min >= 0.0, "{format_name}: negative values (min={min})");
}

/// Verify sRGB u8 output.
fn verify_srgb_output(output: &RawDecodeOutput, format_name: &str) {
    assert_eq!(
        output.pixels.descriptor(),
        zenpixels::PixelDescriptor::RGB8_SRGB,
        "{format_name}: expected RGB8_SRGB"
    );

    let bytes = output.pixels.copy_to_contiguous_bytes();
    let pixel_count = output.info.width as usize * output.info.height as usize;
    assert_eq!(
        bytes.len(),
        pixel_count * 3,
        "{format_name}: wrong byte count for sRGB"
    );

    // Check not all black or all white
    let sum: u64 = bytes.iter().map(|&b| b as u64).sum();
    let mean = sum as f64 / bytes.len() as f64;
    eprintln!("  {format_name} sRGB: mean={mean:.1}/255");
    assert!(
        mean > 1.0,
        "{format_name}: sRGB output is all black (mean={mean})"
    );
    assert!(
        mean < 254.0,
        "{format_name}: sRGB output is all white (mean={mean})"
    );
}

// ── Per-format tests ─────────────────────────────────────────────────

#[test]
fn format_nikon_nef() {
    let Some(data) = load_sample("nikon_d40.nef") else {
        eprintln!("Skipping: nikon_d40.nef not found (run `just fetch-samples`)");
        return;
    };

    // Probe
    let info = zenraw::probe(&data, &Unstoppable).expect("probe NEF");
    assert!(info.width > 0);
    assert!(info.height > 0);
    assert!(!info.is_dng);
    eprintln!(
        "NEF probe: {}x{} {} {}",
        info.width, info.height, info.make, info.model
    );

    // Linear decode
    let output = decode_linear(&data);
    verify_output(&output, "NEF");
    verify_linear_stats(&output, "NEF");

    // sRGB decode
    let srgb = decode_srgb(&data);
    verify_srgb_output(&srgb, "NEF");
}

#[test]
fn format_canon_cr2() {
    let Some(data) = load_sample("canon_350d.cr2") else {
        eprintln!("Skipping: canon_350d.cr2 not found");
        return;
    };

    let info = zenraw::probe(&data, &Unstoppable).expect("probe CR2");
    assert!(!info.is_dng);
    eprintln!(
        "CR2 probe: {}x{} {} {}",
        info.width, info.height, info.make, info.model
    );

    let output = decode_linear(&data);
    verify_output(&output, "CR2");
    verify_linear_stats(&output, "CR2");

    let srgb = decode_srgb(&data);
    verify_srgb_output(&srgb, "CR2");
}

#[test]
fn format_sony_arw() {
    let Some(data) = load_sample("sony_nex3.arw") else {
        eprintln!("Skipping: sony_nex3.arw not found");
        return;
    };

    let info = zenraw::probe(&data, &Unstoppable).expect("probe ARW");
    assert!(!info.is_dng);
    eprintln!(
        "ARW probe: {}x{} {} {}",
        info.width, info.height, info.make, info.model
    );

    let output = decode_linear(&data);
    verify_output(&output, "ARW");
    verify_linear_stats(&output, "ARW");

    let srgb = decode_srgb(&data);
    verify_srgb_output(&srgb, "ARW");
}

#[test]
fn format_olympus_orf() {
    let Some(data) = load_sample("olympus_c5050z.orf") else {
        eprintln!("Skipping: olympus_c5050z.orf not found");
        return;
    };

    let info = zenraw::probe(&data, &Unstoppable).expect("probe ORF");
    assert!(!info.is_dng);
    eprintln!(
        "ORF probe: {}x{} {} {}",
        info.width, info.height, info.make, info.model
    );

    let output = decode_linear(&data);
    verify_output(&output, "ORF");
    verify_linear_stats(&output, "ORF");

    let srgb = decode_srgb(&data);
    verify_srgb_output(&srgb, "ORF");
}

#[test]
fn format_panasonic_rw2() {
    let Some(data) = load_sample("panasonic_gf1.rw2") else {
        eprintln!("Skipping: panasonic_gf1.rw2 not found");
        return;
    };

    let info = zenraw::probe(&data, &Unstoppable).expect("probe RW2");
    assert!(!info.is_dng);
    eprintln!(
        "RW2 probe: {}x{} {} {}",
        info.width, info.height, info.make, info.model
    );

    let output = decode_linear(&data);
    verify_output(&output, "RW2");
    verify_linear_stats(&output, "RW2");

    let srgb = decode_srgb(&data);
    verify_srgb_output(&srgb, "RW2");
}

#[test]
fn format_dng_iphone() {
    let Some(data) = load_sample("iphone12pro.dng") else {
        eprintln!("Skipping: iphone12pro.dng not found");
        return;
    };

    let info = zenraw::probe(&data, &Unstoppable).expect("probe iPhone DNG");
    assert!(info.is_dng);
    eprintln!(
        "iPhone DNG probe: {}x{} {} {}",
        info.width, info.height, info.make, info.model
    );

    let output = decode_linear(&data);
    verify_output(&output, "iPhone DNG");
    verify_linear_stats(&output, "iPhone DNG");

    let srgb = decode_srgb(&data);
    verify_srgb_output(&srgb, "iPhone DNG");
}

#[test]
fn format_fivek_dng() {
    // Test with a FiveK corpus DNG
    let dirs = ["/mnt/v/input/fivek/dng/"];
    let mut data = None;
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()).take(1) {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
            {
                data = std::fs::read(&path).ok().map(|d| (path, d));
                break;
            }
        }
    }

    let Some((path, data)) = data else {
        eprintln!("Skipping: no FiveK DNG files found");
        return;
    };

    let info = zenraw::probe(&data, &Unstoppable).expect("probe FiveK DNG");
    assert!(info.is_dng);
    eprintln!(
        "FiveK DNG probe: {} {}x{} {} {}",
        path.file_name().unwrap().to_str().unwrap(),
        info.width,
        info.height,
        info.make,
        info.model
    );

    let output = decode_linear(&data);
    verify_output(&output, "FiveK DNG");
    verify_linear_stats(&output, "FiveK DNG");
}

// ── Formats that need special handling ───────────────────────────────

#[test]
fn format_fuji_raf_xtrans() {
    let Some(data) = load_sample("fuji_xt1.raf") else {
        eprintln!("Skipping: fuji_xt1.raf not found");
        return;
    };

    // X-Trans sensor — probe should work but decode will fail
    // (our demosaic only handles 2x2 Bayer)
    let info = zenraw::probe(&data, &Unstoppable).expect("probe RAF");
    assert!(!info.is_dng);
    eprintln!(
        "RAF probe: {}x{} {} {} CFA={}",
        info.width, info.height, info.make, info.model, info.cfa_pattern
    );

    // Decode should fail with Unsupported for X-Trans
    let config = RawDecodeConfig::default();
    let result = zenraw::decode(&data, &config, &Unstoppable);
    match result {
        Err(e) => {
            eprintln!("RAF decode correctly rejected: {e}");
            // Should be Unsupported, not a panic
        }
        Ok(output) => {
            // If rawler managed to decode it (non-Bayer path), that's fine too
            eprintln!(
                "RAF decoded (unexpected success): {}x{}",
                output.info.width, output.info.height
            );
        }
    }
}

#[test]
fn format_canon_cr3() {
    let Some(data) = load_sample("canon_eosr_craw.cr3") else {
        eprintln!("Skipping: canon_eosr_craw.cr3 not found");
        return;
    };

    // CR3 requires rawler (rawloader doesn't support it)
    // Probe may or may not work depending on backend
    match zenraw::probe(&data, &Unstoppable) {
        Ok(info) => {
            eprintln!(
                "CR3 probe: {}x{} {} {}",
                info.width, info.height, info.make, info.model
            );

            let output = decode_linear(&data);
            verify_output(&output, "CR3");
            verify_linear_stats(&output, "CR3");

            let srgb = decode_srgb(&data);
            verify_srgb_output(&srgb, "CR3");
        }
        Err(e) => {
            eprintln!("CR3 probe failed (expected without rawler): {e}");
        }
    }
}

// ── Cross-format consistency tests ───────────────────────────────────

#[test]
fn all_formats_probe_consistency() {
    let samples = [
        "nikon_d40.nef",
        "canon_350d.cr2",
        "sony_nex3.arw",
        "olympus_c5050z.orf",
        "panasonic_gf1.rw2",
        "iphone12pro.dng",
    ];

    let mut probed = 0;
    for name in &samples {
        let Some(data) = load_sample(name) else {
            continue;
        };

        let info = zenraw::probe(&data, &Unstoppable).expect(&format!("probe {name}"));

        // All should have valid dimensions
        assert!(
            info.width >= 100,
            "{name}: width too small ({})",
            info.width
        );
        assert!(
            info.height >= 100,
            "{name}: height too small ({})",
            info.height
        );

        // Sensor dimensions should be >= output dimensions
        assert!(
            info.sensor_width >= info.width,
            "{name}: sensor_width < width"
        );
        assert!(
            info.sensor_height >= info.height,
            "{name}: sensor_height < height"
        );

        // Orientation should be valid EXIF (1-8)
        assert!(
            info.orientation >= 1 && info.orientation <= 8,
            "{name}: invalid orientation {}",
            info.orientation
        );

        probed += 1;
    }

    if probed == 0 {
        eprintln!("Skipping: no sample files available");
    } else {
        eprintln!("Probed {probed}/{} formats successfully", samples.len());
    }
}

#[test]
fn all_formats_bilinear_vs_malvar() {
    let samples = [
        "nikon_d40.nef",
        "canon_350d.cr2",
        "sony_nex3.arw",
        "olympus_c5050z.orf",
        "panasonic_gf1.rw2",
    ];

    for name in &samples {
        let Some(data) = load_sample(name) else {
            continue;
        };

        // Decode with both demosaic methods
        let config_malvar = RawDecodeConfig::new().with_gamma(true);
        let config_bilinear = RawDecodeConfig::new()
            .with_gamma(true)
            .with_demosaic(zenraw::DemosaicMethod::Bilinear);

        let Ok(malvar) = zenraw::decode(&data, &config_malvar, &Unstoppable) else {
            continue;
        };
        let Ok(bilinear) = zenraw::decode(&data, &config_bilinear, &Unstoppable) else {
            continue;
        };

        // Same dimensions
        assert_eq!(
            malvar.info.width, bilinear.info.width,
            "{name}: width mismatch"
        );
        assert_eq!(
            malvar.info.height, bilinear.info.height,
            "{name}: height mismatch"
        );

        // Both should be sRGB
        assert_eq!(
            malvar.pixels.descriptor(),
            zenpixels::PixelDescriptor::RGB8_SRGB
        );
        assert_eq!(
            bilinear.pixels.descriptor(),
            zenpixels::PixelDescriptor::RGB8_SRGB
        );

        // They should differ (different algorithms) but not drastically
        let m_bytes = malvar.pixels.copy_to_contiguous_bytes();
        let b_bytes = bilinear.pixels.copy_to_contiguous_bytes();
        assert_eq!(m_bytes.len(), b_bytes.len());

        let diff: u64 = m_bytes
            .iter()
            .zip(b_bytes.iter())
            .map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs() as u64)
            .sum();
        let mad = diff as f64 / m_bytes.len() as f64;

        eprintln!("{name}: Malvar vs Bilinear MAD = {mad:.2}");

        // They should be similar but not identical
        assert!(
            mad < 20.0,
            "{name}: demosaic methods differ too much (MAD={mad})"
        );
    }
}

// ── EXIF extraction tests ────────────────────────────────────────────

#[cfg(feature = "exif")]
mod exif_tests {
    use super::*;

    #[test]
    fn exif_nikon_nef() {
        let Some(data) = load_sample("nikon_d40.nef") else {
            eprintln!("Skipping: nikon_d40.nef not found");
            return;
        };

        let meta = zenraw::exif::read_metadata(&data).expect("EXIF from NEF");
        assert!(meta.make.is_some());
        assert!(meta.model.is_some());
        eprintln!("NEF EXIF: make={:?} model={:?}", meta.make, meta.model);
        eprintln!(
            "  ISO={:?} exposure={:?} f_number={:?}",
            meta.iso, meta.exposure_time, meta.f_number
        );
        eprintln!(
            "  orientation={:?} bits_per_sample={:?}",
            meta.orientation, meta.bits_per_sample
        );

        // NEF should NOT have DNG version
        assert!(meta.dng_version.is_none(), "NEF should not be DNG");
    }

    #[test]
    fn exif_canon_cr2() {
        let Some(data) = load_sample("canon_350d.cr2") else {
            eprintln!("Skipping: canon_350d.cr2 not found");
            return;
        };

        let meta = zenraw::exif::read_metadata(&data).expect("EXIF from CR2");
        assert!(meta.make.is_some());
        let make = meta.make.as_deref().unwrap_or("");
        assert!(
            make.to_lowercase().contains("canon"),
            "expected Canon make, got: {make}"
        );
        eprintln!("CR2 EXIF: make={:?} model={:?}", meta.make, meta.model);
        assert!(meta.dng_version.is_none());
    }

    #[test]
    fn exif_sony_arw() {
        let Some(data) = load_sample("sony_nex3.arw") else {
            eprintln!("Skipping: sony_nex3.arw not found");
            return;
        };

        let meta = zenraw::exif::read_metadata(&data).expect("EXIF from ARW");
        assert!(meta.make.is_some());
        let make = meta.make.as_deref().unwrap_or("");
        assert!(
            make.to_lowercase().contains("sony"),
            "expected Sony make, got: {make}"
        );
        eprintln!("ARW EXIF: make={:?} model={:?}", meta.make, meta.model);
    }

    #[test]
    fn exif_olympus_orf() {
        let Some(data) = load_sample("olympus_c5050z.orf") else {
            eprintln!("Skipping: olympus_c5050z.orf not found");
            return;
        };

        // kamadak-exif can't parse some ORF files (non-standard TIFF variant)
        match zenraw::exif::read_metadata(&data) {
            Some(meta) => {
                assert!(meta.make.is_some());
                eprintln!("ORF EXIF: make={:?} model={:?}", meta.make, meta.model);
            }
            None => {
                eprintln!("ORF EXIF: kamadak-exif cannot parse this ORF (expected limitation)");
            }
        }
    }

    #[test]
    fn exif_panasonic_rw2() {
        let Some(data) = load_sample("panasonic_gf1.rw2") else {
            eprintln!("Skipping: panasonic_gf1.rw2 not found");
            return;
        };

        // kamadak-exif can't parse some RW2 files (non-standard TIFF variant)
        match zenraw::exif::read_metadata(&data) {
            Some(meta) => {
                assert!(meta.make.is_some());
                let make = meta.make.as_deref().unwrap_or("");
                assert!(
                    make.to_lowercase().contains("panasonic"),
                    "expected Panasonic make, got: {make}"
                );
                eprintln!("RW2 EXIF: make={:?} model={:?}", meta.make, meta.model);
            }
            None => {
                eprintln!("RW2 EXIF: kamadak-exif cannot parse this RW2 (expected limitation)");
            }
        }
    }

    #[test]
    fn exif_iphone_dng() {
        let Some(data) = load_sample("iphone12pro.dng") else {
            eprintln!("Skipping: iphone12pro.dng not found");
            return;
        };

        let meta = zenraw::exif::read_metadata(&data).expect("EXIF from iPhone DNG");
        assert!(meta.make.is_some());
        let make = meta.make.as_deref().unwrap_or("");
        assert!(
            make.to_lowercase().contains("apple"),
            "expected Apple make, got: {make}"
        );

        // iPhone DNG should have DNG-specific fields
        assert!(
            meta.dng_version.is_some(),
            "iPhone DNG should have DNG version"
        );
        eprintln!(
            "iPhone DNG EXIF: make={:?} model={:?}",
            meta.make, meta.model
        );
        eprintln!("  DNG version: {:?}", meta.dng_version);
        eprintln!("  ColorMatrix1: {:?}", meta.color_matrix_1);
        eprintln!("  AsShotNeutral: {:?}", meta.as_shot_neutral);
        eprintln!(
            "  GPS: lat={:?} lon={:?}",
            meta.gps_latitude, meta.gps_longitude
        );
    }

    #[test]
    fn exif_fivek_batch() {
        let dir = "/mnt/v/input/fivek/dng/";
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("Skipping: FiveK DNG directory not found");
            return;
        };

        let mut success = 0;
        let mut fail = 0;
        let mut cameras: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        for entry in entries.filter_map(|e| e.ok()).take(50) {
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
            {
                continue;
            }

            let Ok(data) = std::fs::read(&path) else {
                continue;
            };

            match zenraw::exif::read_metadata(&data) {
                Some(meta) => {
                    success += 1;
                    assert!(meta.dng_version.is_some());
                    assert!(meta.color_matrix_1.is_some());
                    if let (Some(make), Some(model)) = (&meta.make, &meta.model) {
                        cameras.insert(format!("{make} {model}"));
                    }
                }
                None => {
                    fail += 1;
                    eprintln!(
                        "WARN: failed EXIF for {}",
                        path.file_name().unwrap().to_str().unwrap()
                    );
                }
            }
        }

        eprintln!("FiveK EXIF batch: {success} ok, {fail} failed");
        eprintln!("Cameras found: {:?}", cameras);
        assert!(success > 0, "no DNG files had valid EXIF");
        assert_eq!(fail, 0, "some DNG files failed EXIF extraction");
    }
}
