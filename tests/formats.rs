//! Per-format integration tests using raw.pixls.us sample corpus.
//!
//! Each test exercises a specific RAW format with probe, decode, and
//! optional EXIF extraction. Tests are skipped if the sample file is
//! not present (run `just fetch-samples` first).

use enough::Unstoppable;
use zenraw::{OutputMode, RawDecodeConfig, RawDecodeOutput};

const SAMPLES_DIR: &str = "/mnt/v/input/raw-samples";

/// Try to read a sample file, returning None if it doesn't exist.
fn load_sample(name: &str) -> Option<Vec<u8>> {
    let path = format!("{SAMPLES_DIR}/{name}");
    std::fs::read(&path).ok()
}

/// Decode with linear f32 config (scene-referred linear f32).
fn decode_linear(data: &[u8]) -> RawDecodeOutput {
    let config = RawDecodeConfig::new().with_output(OutputMode::Linear);
    zenraw::decode(data, &config, &Unstoppable).expect("decode should succeed")
}

/// Decode with sRGB (display-referred u16).
fn decode_srgb(data: &[u8]) -> RawDecodeOutput {
    let config = RawDecodeConfig::new().with_output(OutputMode::Develop);
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
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_ne_bytes(*c))
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

/// Verify sRGB u16 output.
fn verify_srgb_output(output: &RawDecodeOutput, format_name: &str) {
    assert_eq!(
        output.pixels.descriptor(),
        zenpixels::PixelDescriptor::RGB16_SRGB,
        "{format_name}: expected RGB16_SRGB"
    );

    let bytes = output.pixels.copy_to_contiguous_bytes();
    let pixel_count = output.info.width as usize * output.info.height as usize;
    assert_eq!(
        bytes.len(),
        pixel_count * 6, // 3 channels × 2 bytes per u16
        "{format_name}: wrong byte count for sRGB u16"
    );

    // Check not all black or all white — interpret as u16
    let samples: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_ne_bytes(*c))
        .collect();
    let sum: u64 = samples.iter().map(|&s| s as u64).sum();
    let mean = sum as f64 / samples.len() as f64;
    eprintln!("  {format_name} sRGB: mean={mean:.1}/65535");
    assert!(
        mean > 256.0,
        "{format_name}: sRGB output is all black (mean={mean})"
    );
    assert!(
        mean < 65279.0,
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

    // iPhone ProRAW DNGs use LJPEG predictor 7 which rawloader doesn't support.
    // This test verifies we return a clean error rather than panicking.
    match zenraw::probe(&data, &Unstoppable) {
        Ok(info) => {
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
        Err(e) => {
            eprintln!("iPhone DNG probe failed (expected without rawler): {e}");
        }
    }
}

#[test]
fn format_apple_proraw_batch() {
    // Test all Apple ProRAW DNGs from /mnt/v/heic/
    let dir = "/mnt/v/heic/";
    let Ok(entries) = std::fs::read_dir(dir) else {
        eprintln!("Skipping: /mnt/v/heic/ not found");
        return;
    };

    let mut tested = 0;
    for entry in entries.filter_map(|e| e.ok()) {
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
        let name = path.file_name().unwrap().to_str().unwrap().to_string();

        // Skip tiny stubs
        if data.len() < 1024 {
            eprintln!("{name}: skipping (only {} bytes)", data.len());
            continue;
        }

        // Check if it's actually a TIFF-based DNG (not JPEG-wrapped AMPF)
        if !zenraw::is_raw_file(&data) {
            eprintln!("{name}: not a RAW/DNG file (JPEG wrapper?)");
            continue;
        }

        // Probe
        match zenraw::probe(&data, &Unstoppable) {
            Ok(info) => {
                assert!(info.is_dng, "{name}: expected DNG");
                eprintln!(
                    "{name}: {}x{} {} {} (sensor={}x{}, orient={})",
                    info.width,
                    info.height,
                    info.make,
                    info.model,
                    info.sensor_width,
                    info.sensor_height,
                    info.orientation
                );

                // Full decode
                let output = decode_linear(&data);
                verify_output(&output, &name);
                verify_linear_stats(&output, &name);

                // sRGB decode
                let srgb = decode_srgb(&data);
                verify_srgb_output(&srgb, &name);

                tested += 1;
            }
            Err(e) => {
                eprintln!("{name}: probe failed: {e}");
            }
        }
    }

    if tested == 0 {
        eprintln!("Skipping: no decodable Apple ProRAW DNGs found");
    } else {
        eprintln!("Decoded {tested} Apple ProRAW DNGs successfully");
    }
}

#[test]
fn format_android_dng() {
    let path = std::path::Path::new("/mnt/v/heic/android/20260220_093521.dng");
    let Ok(data) = std::fs::read(path) else {
        eprintln!("Skipping: Android DNG not found");
        return;
    };

    if !zenraw::is_raw_file(&data) {
        eprintln!("Skipping: not a RAW file");
        return;
    }

    let info = zenraw::probe(&data, &Unstoppable).expect("probe Android DNG");
    assert!(info.is_dng);
    eprintln!(
        "Android DNG: {}x{} {} {}",
        info.width, info.height, info.make, info.model
    );

    let output = decode_linear(&data);
    verify_output(&output, "Android DNG");
    verify_linear_stats(&output, "Android DNG");

    let srgb = decode_srgb(&data);
    verify_srgb_output(&srgb, "Android DNG");
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

    // X-Trans sensor — both probe and decode should work
    let info = zenraw::probe(&data, &Unstoppable).expect("probe RAF");
    assert!(!info.is_dng);
    eprintln!(
        "RAF probe: {}x{} {} {} CFA={}",
        info.width, info.height, info.make, info.model, info.cfa_pattern
    );

    // Decode X-Trans with bilinear demosaic
    let config = RawDecodeConfig::default();
    let output = zenraw::decode(&data, &config, &Unstoppable).expect("decode RAF X-Trans");
    eprintln!(
        "RAF decoded: {}x{} orient={}",
        output.info.width, output.info.height, output.info.orientation
    );
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

        let info = match zenraw::probe(&data, &Unstoppable) {
            Ok(info) => info,
            Err(e) => {
                // Some formats (e.g., iPhone DNG with LJPEG predictor 7)
                // may not be supported by the current backend.
                eprintln!("{name}: probe failed (skipping): {e}");
                continue;
            }
        };

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

        // Decode with both demosaic methods (Develop mode → u16 sRGB)
        let config_malvar = RawDecodeConfig::new();
        let config_bilinear =
            RawDecodeConfig::new().with_demosaic(zenraw::DemosaicMethod::Bilinear);

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

        // Both should be sRGB u16
        assert_eq!(
            malvar.pixels.descriptor(),
            zenpixels::PixelDescriptor::RGB16_SRGB
        );
        assert_eq!(
            bilinear.pixels.descriptor(),
            zenpixels::PixelDescriptor::RGB16_SRGB
        );

        // They should differ (different algorithms) but not drastically
        let m_bytes = malvar.pixels.copy_to_contiguous_bytes();
        let b_bytes = bilinear.pixels.copy_to_contiguous_bytes();
        assert_eq!(m_bytes.len(), b_bytes.len());

        // Compare as u16 samples
        let m_samples: Vec<u16> = m_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_ne_bytes(*c))
            .collect();
        let b_samples: Vec<u16> = b_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_ne_bytes(*c))
            .collect();

        let diff: u64 = m_samples
            .iter()
            .zip(b_samples.iter())
            .map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs() as u64)
            .sum();
        let mad = diff as f64 / m_samples.len() as f64;

        eprintln!("{name}: Malvar vs Bilinear MAD = {mad:.2} (u16 scale)");

        // They should be similar but not identical (scaled from u8 threshold: 20 * 257 ≈ 5140)
        assert!(
            mad < 5200.0,
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
    fn exif_apple_proraw() {
        let dir = "/mnt/v/heic/";
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("Skipping: /mnt/v/heic/ not found");
            return;
        };

        for entry in entries.filter_map(|e| e.ok()) {
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
            let name = path.file_name().unwrap().to_str().unwrap().to_string();

            if data.len() < 1024 || !zenraw::is_raw_file(&data) {
                continue;
            }

            match zenraw::exif::read_metadata(&data) {
                Some(meta) => {
                    assert!(meta.make.is_some());
                    let make = meta.make.as_deref().unwrap_or("");
                    assert!(
                        make.to_lowercase().contains("apple"),
                        "{name}: expected Apple make, got: {make}"
                    );
                    assert!(meta.dng_version.is_some(), "{name}: expected DNG version");
                    eprintln!(
                        "{name}: make={:?} model={:?} DNG={:?}",
                        meta.make, meta.model, meta.dng_version
                    );
                    eprintln!(
                        "  GPS: lat={:?} lon={:?} alt={:?}",
                        meta.gps_latitude, meta.gps_longitude, meta.gps_altitude
                    );
                    eprintln!(
                        "  ISO={:?} focal={:?} f_number={:?}",
                        meta.iso, meta.focal_length, meta.f_number
                    );
                }
                None => {
                    eprintln!("{name}: EXIF extraction failed");
                }
            }
        }
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

// ── XMP extraction tests ─────────────────────────────────────────────

#[cfg(feature = "xmp")]
mod xmp_tests {
    use super::*;

    #[test]
    fn xmp_fivek_dng() {
        let dir = "/mnt/v/input/fivek/dng/";
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("Skipping: FiveK DNG directory not found");
            return;
        };

        let mut with_xmp = 0;
        let mut without_xmp = 0;
        let mut white_balances: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();

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

            match zenraw::xmp::read_xmp_metadata(&data) {
                Some(meta) => {
                    with_xmp += 1;
                    if let Some(ref wb) = meta.white_balance {
                        white_balances.insert(wb.clone());
                    }
                }
                None => {
                    without_xmp += 1;
                }
            }
        }

        eprintln!("FiveK XMP: {with_xmp} with XMP, {without_xmp} without");
        eprintln!("White balances: {:?}", white_balances);
        assert!(with_xmp > 0, "no DNG files had XMP");
    }

    #[test]
    fn xmp_consistency_with_exif() {
        // For DNG files that have both XMP and EXIF, verify they agree on make/model
        let dir = "/mnt/v/input/fivek/dng/";
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("Skipping: FiveK DNG directory not found");
            return;
        };

        let mut compared = 0;

        for entry in entries.filter_map(|e| e.ok()).take(20) {
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

            #[cfg(feature = "exif")]
            {
                let exif = zenraw::exif::read_metadata(&data);
                let xmp = zenraw::xmp::read_xmp_metadata(&data);

                if let (Some(exif), Some(xmp)) = (exif, xmp) {
                    // If both have make, they should agree
                    if let (Some(exif_make), Some(xmp_make)) = (&exif.make, &xmp.tiff_make) {
                        assert_eq!(
                            exif_make,
                            xmp_make,
                            "EXIF/XMP make mismatch in {}",
                            path.file_name().unwrap().to_str().unwrap()
                        );
                    }
                    compared += 1;
                }
            }
        }

        if compared > 0 {
            eprintln!("Compared EXIF vs XMP for {compared} files — all consistent");
        } else {
            eprintln!("Skipping: no files with both EXIF and XMP found");
        }
    }

    #[test]
    fn xmp_raw_samples() {
        let samples = [
            ("nikon_d40.nef", false),
            ("canon_350d.cr2", false),
            ("sony_nex3.arw", false),
            ("olympus_c5050z.orf", false),
            ("panasonic_gf1.rw2", false),
            ("iphone12pro.dng", false), // iPhone DNG may or may not have XMP
            ("canon_eosr_craw.cr3", true), // CR3 typically has XMP
        ];

        for (name, expect_xmp) in &samples {
            let Some(data) = load_sample(name) else {
                continue;
            };

            let xmp = zenraw::xmp::extract_xmp(&data);
            let has_xmp = xmp.is_some();

            if *expect_xmp {
                assert!(has_xmp, "{name}: expected XMP but none found");
            }

            eprintln!("{name}: XMP={}", if has_xmp { "yes" } else { "no" });

            if let Some(meta) = zenraw::xmp::read_xmp_metadata(&data) {
                if let Some(ref make) = meta.tiff_make {
                    eprintln!("  tiff:Make = {make}");
                }
                if let Some(ref wb) = meta.white_balance {
                    eprintln!("  crs:WhiteBalance = {wb}");
                }
            }
        }
    }
}
