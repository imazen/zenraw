//! Integration tests for zenraw.
//!
//! Tests the full decode pipeline using synthetic sensor data and,
//! when available, real DNG/RAW files.

use enough::Unstoppable;
use zenraw::{DemosaicMethod, RawDecodeConfig, RawError};

// ── Format detection tests ─────────────────────────────────────────────

#[test]
fn is_raw_file_detection() {
    // TIFF little-endian header
    let tiff_le = [b'I', b'I', 42, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(zenraw::is_raw_file(&tiff_le));

    // TIFF big-endian header
    let tiff_be = [b'M', b'M', 0, 42, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(zenraw::is_raw_file(&tiff_be));

    // Fuji RAF
    let fuji = b"FUJIFILM1234";
    assert!(zenraw::is_raw_file(fuji));

    // Panasonic RW2
    let rw2 = [b'I', b'I', 0x55, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(zenraw::is_raw_file(&rw2));

    // PNG (not RAW)
    let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
    assert!(!zenraw::is_raw_file(&png));

    // JPEG (not RAW)
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(!zenraw::is_raw_file(&jpeg));

    // Too short
    assert!(!zenraw::is_raw_file(&[0, 1, 2]));
    assert!(!zenraw::is_raw_file(&[]));
}

// ── Invalid input tests ────────────────────────────────────────────────

#[test]
fn decode_empty_input_fails() {
    let config = RawDecodeConfig::default();
    let result = zenraw::decode(&[], &config, &Unstoppable);
    assert!(result.is_err());
}

#[test]
fn decode_garbage_input_fails() {
    let config = RawDecodeConfig::default();
    let garbage = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00];
    let result = zenraw::decode(&garbage, &config, &Unstoppable);
    assert!(result.is_err());
}

#[test]
fn decode_jpeg_input_fails() {
    let config = RawDecodeConfig::default();
    let jpeg_header = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];
    let result = zenraw::decode(&jpeg_header, &config, &Unstoppable);
    assert!(result.is_err());
}

#[test]
fn probe_empty_input_fails() {
    let result = zenraw::probe(&[], &Unstoppable);
    assert!(result.is_err());
}

// ── Config tests ───────────────────────────────────────────────────────

#[test]
fn config_builder() {
    let config = RawDecodeConfig::new()
        .with_demosaic(DemosaicMethod::Bilinear)
        .with_max_pixels(1_000_000)
        .with_gamma(false)
        .with_crop(false);

    assert_eq!(config.demosaic, DemosaicMethod::Bilinear);
    assert_eq!(config.max_pixels, 1_000_000);
    assert!(!config.apply_gamma);
    assert!(!config.apply_crop);
}

#[test]
fn config_defaults() {
    let config = RawDecodeConfig::default();
    assert_eq!(config.demosaic, DemosaicMethod::MalvarHeCutler);
    assert_eq!(config.max_pixels, 200_000_000);
    assert!(config.apply_gamma);
    assert!(config.apply_crop);
}

// ── Demosaic module tests (exposed via public API) ─────────────────────

#[test]
fn demosaic_public_api() {
    let cfa = rawloader::CFA::new("RGGB");
    let data = vec![0.5f32; 16 * 16];

    let rgb_bilinear =
        zenraw::demosaic::demosaic_to_rgb_f32(&data, 16, 16, &cfa, DemosaicMethod::Bilinear);
    assert_eq!(rgb_bilinear.len(), 16 * 16 * 3);

    let rgb_malvar =
        zenraw::demosaic::demosaic_to_rgb_f32(&data, 16, 16, &cfa, DemosaicMethod::MalvarHeCutler);
    assert_eq!(rgb_malvar.len(), 16 * 16 * 3);
}

// ── Color pipeline tests (exposed via public API) ──────────────────────

#[test]
fn color_pipeline_public_api() {
    let mut rgb = vec![0.5f32; 30]; // 10 pixels × 3 channels
    let wb = [2.0f32, 1.0, 1.5, 0.0];
    let xtc = [
        [0.6, 0.3, 0.1],
        [0.2, 0.7, 0.1],
        [0.1, 0.2, 0.7],
        [0.0, 0.0, 0.0],
    ];
    zenraw::color::apply_color_pipeline(&mut rgb, wb, xtc);
    // All values should be in [0, 1]
    for &v in &rgb {
        assert!(v >= 0.0 && v <= 1.0, "value out of range: {v}");
    }
}

#[test]
fn srgb_gamma_public_api() {
    let mut rgb = vec![0.0f32, 0.5, 1.0];
    zenraw::color::apply_srgb_gamma(&mut rgb);
    assert!((rgb[0] - 0.0).abs() < 1e-6);
    assert!(rgb[1] > 0.5); // sRGB gamma should brighten midtones
    assert!((rgb[2] - 1.0).abs() < 1e-4);
}

// ── Full pipeline synthetic test ───────────────────────────────────────

#[test]
fn full_pipeline_synthetic_data() {
    // Test the color pipeline with a gradient
    let width = 32;
    let height = 32;
    let cfa = rawloader::CFA::new("RGGB");

    // Create a gradient Bayer pattern
    let mut data = Vec::with_capacity(width * height);
    for row in 0..height {
        for col in 0..width {
            let val = (row * width + col) as f32 / (width * height) as f32;
            data.push(val);
        }
    }

    // Demosaic
    let mut rgb = zenraw::demosaic::demosaic_to_rgb_f32(
        &data,
        width,
        height,
        &cfa,
        DemosaicMethod::MalvarHeCutler,
    );
    assert_eq!(rgb.len(), width * height * 3);

    // Apply a reasonable color pipeline
    let wb = [1.5f32, 1.0, 1.2, 0.0];
    let xtc = [
        [0.5, 0.3, 0.2],
        [0.2, 0.6, 0.2],
        [0.1, 0.2, 0.7],
        [0.0, 0.0, 0.0],
    ];
    zenraw::color::apply_color_pipeline(&mut rgb, wb, xtc);

    // All values should be clamped
    for &v in &rgb {
        assert!(v >= 0.0 && v <= 1.0, "color pipeline out of range: {v}");
    }

    // Apply gamma
    zenraw::color::apply_srgb_gamma(&mut rgb);

    // Convert to u8
    let u8_data = zenraw::color::f32_to_u8_srgb(&rgb);
    assert_eq!(u8_data.len(), width * height * 3);

    // Build pixel buffer
    let buf = zenpixels::PixelBuffer::from_vec(
        u8_data,
        width as u32,
        height as u32,
        zenpixels::PixelDescriptor::RGB8_SRGB,
    )
    .expect("buffer creation");

    assert_eq!(buf.width(), width as u32);
    assert_eq!(buf.height(), height as u32);
}

// ── Real file tests (skipped when test data unavailable) ───────────────

/// Try to find a RAW file that rawloader can handle.
fn find_working_raw_file() -> Option<(std::path::PathBuf, Vec<u8>)> {
    // Try common locations
    let search_dirs = [
        "/mnt/v/input/raw-samples/",
        "/mnt/v/input/fivek/dng/",
        "/mnt/tower/input/raw-samples/",
    ];

    for dir in &search_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(
                ext.to_lowercase().as_str(),
                "dng" | "cr2" | "nef" | "arw" | "rw2" | "pef" | "orf"
            ) {
                continue;
            }

            if let Ok(data) = std::fs::read(&path) {
                // Quick check if rawloader can handle it (without full decode)
                if rawloader::decode(&mut std::io::Cursor::new(&data)).is_ok() {
                    return Some((path, data));
                }
            }
        }
    }
    None
}

#[test]
fn decode_real_raw_file() {
    let Some((path, data)) = find_working_raw_file() else {
        eprintln!("Skipping: no compatible RAW files found for testing");
        return;
    };

    eprintln!("Testing with: {}", path.display());

    // Test probe
    let info = zenraw::probe(&data, &Unstoppable).expect("probe should succeed");
    assert!(info.width > 0);
    assert!(info.height > 0);
    eprintln!(
        "Probed: {}x{} {} {} CFA={}",
        info.width, info.height, info.make, info.model, info.cfa_pattern
    );

    // Test decode with default settings (sRGB RGB8)
    let config = RawDecodeConfig::default();
    let output = zenraw::decode(&data, &config, &Unstoppable).expect("decode should succeed");
    assert_eq!(
        output.pixels.descriptor(),
        zenpixels::PixelDescriptor::RGB8_SRGB
    );
    assert!(output.info.width > 0);
    assert!(output.info.height > 0);

    // Test decode with linear f32
    let config_linear = RawDecodeConfig::new().with_gamma(false);
    let output_linear = zenraw::decode(&data, &config_linear, &Unstoppable).expect("linear decode");
    assert_eq!(
        output_linear.pixels.descriptor(),
        zenpixels::PixelDescriptor::RGBF32_LINEAR
    );

    // Test decode with bilinear demosaic
    let config_bilinear = RawDecodeConfig::new().with_demosaic(DemosaicMethod::Bilinear);
    let output_bilinear =
        zenraw::decode(&data, &config_bilinear, &Unstoppable).expect("bilinear decode");
    assert!(output_bilinear.info.width > 0);

    eprintln!("All decode modes successful for {}", path.display());
}

#[test]
fn pixel_limit_with_real_file() {
    let Some((_path, data)) = find_working_raw_file() else {
        eprintln!("Skipping: no compatible RAW files found");
        return;
    };

    let config = RawDecodeConfig::new().with_max_pixels(100);
    let result = zenraw::decode(&data, &config, &Unstoppable);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        matches!(err.as_ref(), RawError::LimitExceeded(_)),
        "expected LimitExceeded, got: {err:?}"
    );
}
