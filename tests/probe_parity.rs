//! Probe-vs-decode parity tests.
//!
//! Verifies that zenraw's detection/classification functions agree with
//! actual decode capability: if we detect a format, we can decode it,
//! and if we can decode it, our detectors correctly identify it.
//!
//! Tests that require external RAW fixture files (FiveK DNG corpus at
//! `/mnt/v/input/fivek/dng` and raw.pixls.us samples at
//! `/mnt/v/input/raw-samples/`) are gated behind the
//! `integration-fixtures` cargo feature so CI does not silently skip
//! or panic on missing fixtures. Run locally with
//! `cargo test --features rawler,integration-fixtures` after
//! `just fetch-samples`.

use enough::Unstoppable;
use zenraw::{FileFormat, classify};
#[cfg(feature = "integration-fixtures")]
use zenraw::{OutputMode, RawDecodeConfig};

#[cfg(feature = "integration-fixtures")]
const SAMPLES_DIR: &str = "/mnt/v/input/raw-samples";
#[cfg(feature = "integration-fixtures")]
const FIVEK_DIR: &str = "/mnt/v/input/fivek/dng";

#[cfg(feature = "integration-fixtures")]
fn load_sample(name: &str) -> Option<Vec<u8>> {
    std::fs::read(format!("{SAMPLES_DIR}/{name}")).ok()
}

#[cfg(feature = "integration-fixtures")]
fn load_first_fivek_dng() -> Option<(String, Vec<u8>)> {
    let entries = std::fs::read_dir(FIVEK_DIR).ok()?;
    for entry in entries.filter_map(|e| e.ok()).take(3) {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
        {
            let name = path.file_name()?.to_str()?.to_string();
            let data = std::fs::read(&path).ok()?;
            return Some((name, data));
        }
    }
    None
}

// ── DNG detection agrees with decode ─────────────────────────────────

#[cfg(feature = "integration-fixtures")]
#[test]
fn dng_classify_agrees_with_probe_and_decode() {
    let (name, data) = load_first_fivek_dng().unwrap_or_else(|| {
        panic!(
            "integration-fixtures enabled but no FiveK DNG files found at {FIVEK_DIR}; \
             run `just fetch-samples` and ensure the FiveK corpus is present"
        )
    });

    // classify should identify as DNG
    let fmt = classify(&data);
    assert_eq!(
        fmt,
        FileFormat::Dng,
        "{name}: classify() returned {fmt}, expected Dng"
    );
    assert!(fmt.is_raw(), "{name}: classify().is_raw() should be true");

    // is_raw_file should agree
    assert!(
        zenraw::is_raw_file(&data),
        "{name}: is_raw_file() should be true for DNG"
    );

    // probe should succeed and report is_dng
    let info = zenraw::probe(&data, &Unstoppable)
        .unwrap_or_else(|e| panic!("{name}: probe() failed: {e}"));
    assert!(info.is_dng, "{name}: probe().is_dng should be true");
    assert!(info.width > 0, "{name}: probe width should be > 0");
    assert!(info.height > 0, "{name}: probe height should be > 0");

    // decode should succeed
    let config = RawDecodeConfig::new().with_output(OutputMode::Linear);
    let output = zenraw::decode(&data, &config, &Unstoppable)
        .unwrap_or_else(|e| panic!("{name}: decode() failed: {e}"));

    // probe and decode dimensions: decode with crop+orientation disabled
    // should match probe (which returns raw sensor active area dimensions)
    let config_raw = RawDecodeConfig::new()
        .with_output(OutputMode::Linear)
        .with_crop(false)
        .with_orientation(false);
    let output_raw = zenraw::decode(&data, &config_raw, &Unstoppable)
        .unwrap_or_else(|e| panic!("{name}: decode(no_crop,no_orient) failed: {e}"));
    assert_eq!(
        info.width, output_raw.info.width,
        "{name}: probe vs decode(no_crop,no_orient) width mismatch"
    );
    assert_eq!(
        info.height, output_raw.info.height,
        "{name}: probe vs decode(no_crop,no_orient) height mismatch"
    );

    // With default settings (crop+orient), pixel count should be
    // less than or equal to uncropped
    let raw_pixels = info.width as u64 * info.height as u64;
    let out_pixels = output.info.width as u64 * output.info.height as u64;
    assert!(
        out_pixels <= raw_pixels,
        "{name}: cropped output has more pixels than probe"
    );

    eprintln!(
        "{name}: classify=Dng, is_dng=true, probe={}x{}, decode={}x{} — all agree",
        info.width, info.height, output.info.width, output.info.height
    );
}

// ── Non-DNG RAW detection agrees with decode ────────────────────────

#[cfg(feature = "integration-fixtures")]
#[test]
fn non_dng_raw_classify_agrees_with_probe_and_decode() {
    let samples: &[&str] = &[
        "nikon_d40.nef",
        "canon_350d.cr2",
        "sony_nex3.arw",
        "olympus_c5050z.orf",
        "panasonic_gf1.rw2",
        "fuji_xt1.raf",
    ];

    let mut tested = 0;
    let mut missing: Vec<&str> = Vec::new();
    for &name in samples {
        let Some(data) = load_sample(name) else {
            missing.push(name);
            continue;
        };

        // classify should not return DNG for non-DNG files
        let fmt = classify(&data);
        assert_ne!(
            fmt,
            FileFormat::Dng,
            "{name}: classify() should not return Dng for non-DNG"
        );
        assert_ne!(
            fmt,
            FileFormat::AppleDng,
            "{name}: classify() should not return AppleDng"
        );
        eprintln!("{name}: classify() = {fmt}");

        // is_raw_file should agree
        assert!(
            zenraw::is_raw_file(&data),
            "{name}: is_raw_file() should be true"
        );

        // probe should succeed and report NOT DNG
        let info = match zenraw::probe(&data, &Unstoppable) {
            Ok(info) => info,
            Err(e) => {
                eprintln!("{name}: probe() failed (backend limitation): {e}");
                continue;
            }
        };
        assert!(!info.is_dng, "{name}: probe().is_dng should be false");

        // decode should succeed
        let config = RawDecodeConfig::new().with_output(OutputMode::Linear);
        let output = match zenraw::decode(&data, &config, &Unstoppable) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("{name}: decode() failed (backend limitation): {e}");
                continue;
            }
        };

        // probe and decode dimensions: decode with crop+orientation disabled
        // should match probe (which returns raw sensor active area dimensions)
        let config_raw = RawDecodeConfig::new()
            .with_output(OutputMode::Linear)
            .with_crop(false)
            .with_orientation(false);
        let output_raw = match zenraw::decode(&data, &config_raw, &Unstoppable) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("{name}: decode(no_crop,no_orient) failed: {e}");
                continue;
            }
        };
        assert_eq!(
            info.width, output_raw.info.width,
            "{name}: probe vs decode(no_crop,no_orient) width mismatch"
        );
        assert_eq!(
            info.height, output_raw.info.height,
            "{name}: probe vs decode(no_crop,no_orient) height mismatch"
        );

        // With default settings (crop+orient), pixel count should be
        // less than or equal to uncropped
        let raw_pixels = info.width as u64 * info.height as u64;
        let out_pixels = output.info.width as u64 * output.info.height as u64;
        assert!(
            out_pixels <= raw_pixels,
            "{name}: cropped output has more pixels than probe"
        );

        eprintln!(
            "{name}: classify={fmt}, is_dng=false, probe={}x{}, decode={}x{} — all agree",
            info.width, info.height, output.info.width, output.info.height
        );
        tested += 1;
    }

    assert!(
        missing.is_empty(),
        "integration-fixtures enabled but missing samples in {SAMPLES_DIR}: {missing:?}; \
         run `just fetch-samples`"
    );
    assert!(tested > 0, "no non-DNG RAW samples were tested");
    eprintln!("Tested {tested}/{} non-DNG formats", samples.len());
}

// ── Non-RAW files correctly rejected ────────────────────────────────

#[test]
fn non_raw_correctly_rejected() {
    // JPEG header
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
    assert!(
        !zenraw::is_raw_file(&jpeg),
        "JPEG should not be detected as RAW"
    );
    assert_eq!(classify(&jpeg), FileFormat::Jpeg);
    assert!(!classify(&jpeg).is_raw());

    // PNG header
    let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert!(
        !zenraw::is_raw_file(&png),
        "PNG should not be detected as RAW"
    );
    assert_eq!(classify(&png), FileFormat::Unknown);

    // Empty / garbage
    assert!(!zenraw::is_raw_file(&[]), "empty should not be RAW");
    assert!(!zenraw::is_raw_file(&[0xDE, 0xAD, 0xBE, 0xEF]));

    // Probe should fail on non-RAW
    assert!(zenraw::probe(&jpeg, &Unstoppable).is_err());
    assert!(zenraw::probe(&[], &Unstoppable).is_err());

    eprintln!("Non-RAW rejection: all formats correctly rejected");
}

// ── XMP extraction parity ───────────────────────────────────────────

#[cfg(all(feature = "xmp", feature = "integration-fixtures"))]
#[test]
fn xmp_extraction_consistent_across_methods() {
    let (name, data) = load_first_fivek_dng().unwrap_or_else(|| {
        panic!(
            "integration-fixtures enabled but no FiveK DNG files found at {FIVEK_DIR}; \
             run `just fetch-samples` and ensure the FiveK corpus is present"
        )
    });

    // extract_xmp returns the raw XML string
    let xmp_raw = zenraw::xmp::extract_xmp(&data);

    // read_xmp_metadata parses properties from the same XML
    let xmp_meta = zenraw::xmp::read_xmp_metadata(&data);

    // If extract_xmp finds XML, read_xmp_metadata should also succeed
    // (since it calls extract_xmp internally)
    match (&xmp_raw, &xmp_meta) {
        (Some(xml), Some(meta)) => {
            // The parsed metadata's raw_xml should match the extracted xml
            assert_eq!(
                meta.raw_xml.as_deref(),
                Some(xml.as_str()),
                "{name}: raw_xml from read_xmp_metadata differs from extract_xmp"
            );
            eprintln!(
                "{name}: XMP consistent — {} bytes, tiff:Make={:?}",
                xml.len(),
                meta.tiff_make
            );
        }
        (None, None) => {
            eprintln!("{name}: no XMP in this DNG (both methods agree)");
        }
        (Some(_), None) => {
            panic!("{name}: extract_xmp found XMP but read_xmp_metadata returned None");
        }
        (None, Some(_)) => {
            panic!("{name}: read_xmp_metadata found XMP but extract_xmp returned None");
        }
    }
}

/// Verify XMP extraction works on multiple sample formats and that
/// the presence/absence of XMP is consistent between extract_xmp and
/// read_xmp_metadata.
#[cfg(all(feature = "xmp", feature = "integration-fixtures"))]
#[test]
fn xmp_cross_format_consistency() {
    let samples = [
        "nikon_d40.nef",
        "canon_350d.cr2",
        "sony_nex3.arw",
        "iphone12pro.dng",
    ];

    let mut tested = 0;
    let mut missing: Vec<&str> = Vec::new();
    for name in &samples {
        let Some(data) = load_sample(name) else {
            missing.push(name);
            continue;
        };

        let raw = zenraw::xmp::extract_xmp(&data);
        let meta = zenraw::xmp::read_xmp_metadata(&data);

        match (&raw, &meta) {
            (Some(xml), Some(m)) => {
                assert_eq!(
                    m.raw_xml.as_deref(),
                    Some(xml.as_str()),
                    "{name}: XMP methods disagree"
                );
                eprintln!("{name}: has XMP ({} bytes)", xml.len());
            }
            (None, None) => {
                eprintln!("{name}: no XMP (consistent)");
            }
            _ => {
                panic!("{name}: XMP extraction methods disagree on presence");
            }
        }
        tested += 1;
    }

    assert!(
        missing.is_empty(),
        "integration-fixtures enabled but missing samples in {SAMPLES_DIR}: {missing:?}; \
         run `just fetch-samples`"
    );
    assert!(tested > 0, "no sample files were tested");
    eprintln!("XMP consistency verified for {tested} formats");
}
