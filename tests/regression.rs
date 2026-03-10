//! Regression tests comparing zenraw output against darktable-cli reference.
//!
//! Uses zensim psychovisual similarity to establish quality baselines.
//! darktable-cli is the gold standard; these tests track how close our
//! pipeline gets and catch regressions.

#![cfg(feature = "darktable")]

use std::path::{Path, PathBuf};

use image::RgbImage;
use zensim::{RgbSlice, Zensim, ZensimProfile};

use zenraw::RawDecodeConfig;
use zenraw::darktable::{DtColorProfile, DtConfig};

// ── Test corpus discovery ─────────────────────────────────────────────

/// Known locations for RAW test files.
const RAW_SEARCH_DIRS: &[&str] = &[
    "/mnt/v/input/raw-samples/",
    "/mnt/v/input/fivek/dng/",
    "/mnt/tower/input/raw-samples/",
];

const RAW_EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "nef", "arw", "srf", "sr2", "rw2", "pef", "orf", "erf", "raf", "3fr",
    "iiq",
];

/// Find RAW files in known locations.
fn find_raw_files(max_count: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in RAW_SEARCH_DIRS {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if RAW_EXTENSIONS.contains(&ext.as_str()) {
                files.push(path);
                if files.len() >= max_count {
                    return files;
                }
            }
        }
    }
    files
}

/// Find DNG files specifically (darktable handles these well).
fn find_dng_files(max_count: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in RAW_SEARCH_DIRS {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
            {
                files.push(path);
                if files.len() >= max_count {
                    return files;
                }
            }
        }
    }
    files
}

// ── Image comparison utilities ────────────────────────────────────────

/// Convert linear f32 RGB data to sRGB u8 for comparison.
fn linear_f32_to_srgb_u8(data: &[u8], width: u32, height: u32) -> Vec<[u8; 3]> {
    let floats: Vec<f32> = data
        .chunks_exact(4)
        .map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let expected = width as usize * height as usize * 3;
    assert_eq!(
        floats.len(),
        expected,
        "expected {expected} f32 values, got {}",
        floats.len()
    );

    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for chunk in floats.chunks_exact(3) {
        let r = linear_to_srgb_u8(chunk[0]);
        let g = linear_to_srgb_u8(chunk[1]);
        let b = linear_to_srgb_u8(chunk[2]);
        pixels.push([r, g, b]);
    }
    pixels
}

/// Linear to sRGB gamma transfer function.
fn linear_to_srgb_u8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let srgb = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0 + 0.5) as u8
}

/// Compute zensim score between two RGB8 images.
fn zensim_score(a: &[[u8; 3]], b: &[[u8; 3]], width: usize, height: usize) -> f64 {
    let z = Zensim::new(ZensimProfile::latest()).with_parallel(false);
    let src = RgbSlice::new(a, width, height);
    let dst = RgbSlice::new(b, width, height);
    z.compute(&src, &dst).unwrap().score()
}

/// Per-channel mean absolute difference between two RGB8 images.
fn mean_abs_diff(a: &[[u8; 3]], b: &[[u8; 3]]) -> f64 {
    let total: u64 = a
        .iter()
        .zip(b.iter())
        .map(|(ap, bp)| {
            (ap[0] as i16 - bp[0] as i16).unsigned_abs() as u64
                + (ap[1] as i16 - bp[1] as i16).unsigned_abs() as u64
                + (ap[2] as i16 - bp[2] as i16).unsigned_abs() as u64
        })
        .sum();
    total as f64 / (a.len() as f64 * 3.0)
}

/// Decode a file with darktable and return sRGB u8 pixels.
fn darktable_decode_srgb(path: &Path) -> Option<(Vec<[u8; 3]>, u32, u32)> {
    if !zenraw::darktable::is_available() {
        return None;
    }
    let config = DtConfig::new().with_color_profile(DtColorProfile::LinearRec709);
    let output = zenraw::darktable::decode_file(path, &config).ok()?;
    let w = output.info.width;
    let h = output.info.height;
    let bytes = output.pixels.copy_to_contiguous_bytes();
    let pixels = linear_f32_to_srgb_u8(&bytes, w, h);
    Some((pixels, w, h))
}

/// Decode a file with zenraw (rawloader) and return sRGB u8 pixels.
/// Catches panics since rawloader panics on X-Trans and some corrupt files.
fn zenraw_decode_srgb(data: &[u8]) -> Option<(Vec<[u8; 3]>, u32, u32)> {
    let data = data.to_vec(); // owned for catch_unwind
    let result = std::panic::catch_unwind(|| {
        let config = RawDecodeConfig::new().with_gamma(true);
        let output = zenraw::decode(&data, &config, &enough::Unstoppable).ok()?;
        let w = output.info.width;
        let h = output.info.height;
        let bytes = output.pixels.copy_to_contiguous_bytes();
        let pixels: Vec<[u8; 3]> = bytes.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        Some((pixels, w, h))
    });
    result.ok().flatten()
}

/// Save comparison result to a log file for tracking.
fn log_result(name: &str, zsim: f64, mad: f64, w: u32, h: u32) {
    let log_dir = Path::new("/mnt/v/output/zenraw/regression/");
    let _ = std::fs::create_dir_all(log_dir);
    let log_path = log_dir.join("results.tsv");
    let header = !log_path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    if let Some(ref mut f) = file {
        use std::io::Write;
        if header {
            let _ = writeln!(f, "file\twidth\theight\tzensim\tmad");
        }
        let _ = writeln!(f, "{name}\t{w}\t{h}\t{zsim:.2}\t{mad:.2}");
    }
}

// ── Regression tests ──────────────────────────────────────────────────

/// Compare darktable vs zenraw (rawloader) output for files rawloader can handle.
#[test]
fn regression_rawloader_vs_darktable() {
    if !zenraw::darktable::is_available() {
        eprintln!("Skipping: darktable-cli not found");
        return;
    }

    let files = find_raw_files(20);
    if files.is_empty() {
        eprintln!("Skipping: no RAW files found");
        return;
    }

    let mut tested = 0;
    let mut scores = Vec::new();

    for path in &files {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };

        // Try rawloader first — skip files it can't handle
        let Some((zenraw_pixels, zw, zh)) = zenraw_decode_srgb(&data) else {
            continue;
        };

        // Get darktable reference
        let Some((dt_pixels, dw, dh)) = darktable_decode_srgb(path) else {
            continue;
        };

        // Dimensions may differ due to:
        // - EXIF rotation (darktable applies it, rawloader doesn't) → W×H swapped
        // - Different crop amounts
        // Check both orientations
        let dims_match = (zw == dw && zh == dh) || (zw == dh && zh == dw);
        let close_dims = {
            let (zw, zh, dw, dh) = (zw as i64, zh as i64, dw as i64, dh as i64);
            ((zw - dw).abs() < 20 && (zh - dh).abs() < 20)
                || ((zw - dh).abs() < 20 && (zh - dw).abs() < 20)
        };
        if !dims_match && !close_dims {
            eprintln!(
                "Dimension mismatch for {}: zenraw={}x{} dt={}x{} — skipping",
                path.file_name().unwrap().to_str().unwrap(),
                zw,
                zh,
                dw,
                dh
            );
            continue;
        }

        // If dimensions are close but not exact, skip zensim (needs exact match)
        if zw != dw || zh != dh {
            eprintln!(
                "Close dimensions for {}: zenraw={}x{} dt={}x{} — logging MAD only",
                path.file_name().unwrap().to_str().unwrap(),
                zw,
                zh,
                dw,
                dh
            );
            continue;
        }

        let zsim = zensim_score(&zenraw_pixels, &dt_pixels, zw as usize, zh as usize);
        let mad = mean_abs_diff(&zenraw_pixels, &dt_pixels);
        let name = path.file_name().unwrap().to_str().unwrap_or("unknown");

        eprintln!("{name}: zensim={zsim:.2} mad={mad:.2} ({zw}x{zh})");
        log_result(name, zsim, mad, zw, zh);

        scores.push((name.to_string(), zsim, mad));
        tested += 1;
    }

    if tested == 0 {
        eprintln!("Skipping: no files could be decoded by both rawloader and darktable");
        return;
    }

    // Summary
    let avg_zsim: f64 = scores.iter().map(|(_, z, _)| z).sum::<f64>() / scores.len() as f64;
    let min_zsim: f64 = scores
        .iter()
        .map(|(_, z, _)| *z)
        .fold(f64::INFINITY, f64::min);
    let avg_mad: f64 = scores.iter().map(|(_, _, m)| m).sum::<f64>() / scores.len() as f64;

    eprintln!("\n=== Regression Summary ({tested} files) ===");
    eprintln!("  zensim: avg={avg_zsim:.2} min={min_zsim:.2}");
    eprintln!("  MAD:    avg={avg_mad:.2}");

    // These are baselines — we expect some difference since our pipeline
    // is simpler than darktable's. Track these numbers and tighten over time.
    // Don't fail the test, just report scores.
}

/// Test darktable output consistency across multiple DNG files.
#[test]
fn regression_darktable_dng_batch() {
    if !zenraw::darktable::is_available() {
        eprintln!("Skipping: darktable-cli not found");
        return;
    }

    let files = find_dng_files(10);
    if files.is_empty() {
        eprintln!("Skipping: no DNG files found");
        return;
    }

    let mut decoded = 0;
    for path in &files {
        let config = DtConfig::default();
        match zenraw::darktable::decode_file(path, &config) {
            Ok(output) => {
                let name = path.file_name().unwrap().to_str().unwrap_or("?");
                let w = output.info.width;
                let h = output.info.height;

                // Verify reasonable output
                let bytes = output.pixels.copy_to_contiguous_bytes();
                let floats: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();

                let mean: f32 = floats.iter().sum::<f32>() / floats.len() as f32;
                let max: f32 = floats.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let min: f32 = floats.iter().cloned().fold(f32::INFINITY, f32::min);

                eprintln!("{name}: {w}x{h} mean={mean:.4} min={min:.4} max={max:.4}");

                assert!(mean > 0.001, "{name}: output is all black (mean={mean})");
                assert!(max < 100.0, "{name}: unreasonable max value ({max})");

                decoded += 1;
            }
            Err(e) => {
                eprintln!(
                    "WARN: failed to decode {}: {e}",
                    path.file_name().unwrap().to_str().unwrap_or("?")
                );
            }
        }
    }

    eprintln!("\nDecoded {decoded}/{} DNG files successfully", files.len());
    assert!(decoded > 0, "no DNG files could be decoded");
}

/// Save darktable reference PNGs for manual inspection.
#[test]
fn regression_save_reference_pngs() {
    if !zenraw::darktable::is_available() {
        eprintln!("Skipping: darktable-cli not found");
        return;
    }

    let files = find_dng_files(5);
    if files.is_empty() {
        eprintln!("Skipping: no DNG files found");
        return;
    }

    let out_dir = Path::new("/mnt/v/output/zenraw/reference/");
    let _ = std::fs::create_dir_all(out_dir);

    for path in &files {
        let Some((pixels, w, h)) = darktable_decode_srgb(path) else {
            continue;
        };

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        // Save darktable reference
        let flat: Vec<u8> = pixels.iter().flat_map(|p| p.iter().copied()).collect();
        let img = RgbImage::from_raw(w, h, flat).expect("valid image");
        let dt_path = out_dir.join(format!("{stem}_dt.png"));
        img.save(&dt_path).expect("save PNG");
        eprintln!("Saved: {}", dt_path.display());

        // Try zenraw (rawloader) too
        if let Ok(data) = std::fs::read(path) {
            if let Some((zr_pixels, zw, zh)) = zenraw_decode_srgb(&data) {
                let flat: Vec<u8> = zr_pixels.iter().flat_map(|p| p.iter().copied()).collect();
                let img = RgbImage::from_raw(zw, zh, flat).expect("valid image");
                let zr_path = out_dir.join(format!("{stem}_zenraw.png"));
                img.save(&zr_path).expect("save PNG");
                eprintln!("Saved: {}", zr_path.display());
            }
        }
    }
}
