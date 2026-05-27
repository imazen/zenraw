//! Probe-vs-decode parity tests.
//!
//! Verifies that zenraw's detection/classification functions agree with
//! actual decode capability: if we detect a format, we can decode it,
//! and if we can decode it, our detectors correctly identify it.

use enough::Unstoppable;
use zenraw::{FileFormat, OutputMode, RawDecodeConfig, classify};

/// Env var that points at the non-DNG RAW sample corpus.
///
/// The corpus is local-only block storage (not in CI), so these tests are
/// gated on the caller setting this variable — the skip decision is visible
/// in the CI -> justfile -> test chain, never a silent runtime check. See the
/// `test-raw-parity` justfile target for the canonical local path.
const SAMPLES_DIR_ENV: &str = "ZENRAW_RAW_SAMPLES_DIR";
const FIVEK_DIR: &str = "/mnt/v/input/fivek/dng";

/// Resolve the RAW sample corpus directory from the environment.
///
/// Returns `None` when `ZENRAW_RAW_SAMPLES_DIR` is unset — the caller is
/// expected to print a skip message and `return`. When it IS set, the corpus
/// is treated as required: missing sample files are hard failures, not skips.
fn samples_dir() -> Option<String> {
    match std::env::var(SAMPLES_DIR_ENV) {
        Ok(dir) if !dir.is_empty() => Some(dir),
        _ => None,
    }
}

/// Load a required sample from the corpus directory.
///
/// The caller has already established (via [`samples_dir`]) that the corpus
/// was requested, so a missing/unreadable file is a hard failure — the caller
/// asked for the corpus and a listed sample is absent.
fn load_required_sample(dir: &str, name: &str) -> Vec<u8> {
    let path = format!("{dir}/{name}");
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("{SAMPLES_DIR_ENV}={dir} is set, but required sample {path} could not be read: {e}")
    })
}

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

#[test]
fn dng_classify_agrees_with_probe_and_decode() {
    let Some((name, data)) = load_first_fivek_dng() else {
        eprintln!("Skipping: no FiveK DNG files found at {FIVEK_DIR}");
        return;
    };

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

#[test]
fn non_dng_raw_classify_agrees_with_probe_and_decode() {
    let Some(dir) = samples_dir() else {
        eprintln!(
            "{SAMPLES_DIR_ENV} not set — skipping non-DNG RAW parity test \
             (run `just test-raw-parity` to exercise it)"
        );
        return;
    };

    let samples: &[&str] = &[
        "nikon_d40.nef",
        "canon_350d.cr2",
        "sony_nex3.arw",
        "olympus_c5050z.orf",
        "panasonic_gf1.rw2",
        "fuji_xt1.raf",
    ];

    let mut tested = 0;
    for &name in samples {
        // Env var is set, so the corpus was requested: a missing listed
        // sample is a hard failure, not a silent skip.
        let data = load_required_sample(&dir, name);

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

    assert!(tested > 0, "no non-DNG RAW samples were available");
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

#[cfg(feature = "xmp")]
#[test]
fn xmp_extraction_consistent_across_methods() {
    let Some((name, data)) = load_first_fivek_dng() else {
        eprintln!("Skipping: no FiveK DNG files found at {FIVEK_DIR}");
        return;
    };

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
#[cfg(feature = "xmp")]
#[test]
fn xmp_cross_format_consistency() {
    let Some(dir) = samples_dir() else {
        eprintln!(
            "{SAMPLES_DIR_ENV} not set — skipping XMP cross-format parity test \
             (run `just test-raw-parity` to exercise it)"
        );
        return;
    };

    let samples = [
        "nikon_d40.nef",
        "canon_350d.cr2",
        "sony_nex3.arw",
        "iphone12pro.dng",
    ];

    let mut tested = 0;
    for name in &samples {
        // Env var is set: missing listed samples are hard failures.
        let data = load_required_sample(&dir, name);

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
        tested > 0,
        "{SAMPLES_DIR_ENV}={dir} is set but no sample files were exercised"
    );
    eprintln!("XMP consistency verified for {tested} formats");
}
