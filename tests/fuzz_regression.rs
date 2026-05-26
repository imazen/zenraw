//! Fuzz crash regression suite (DEDUP-J template, ported from zenwebp).
//!
//! Runs every file in `fuzz/regression/` through every decoder entry point that
//! has a fuzz target. Each seed file is a previously-found crash that has been
//! fixed; this test ensures none of them re-introduce a panic.
//!
//! Reproduces what the `fuzz_classify` and `fuzz_decode` fuzz targets do, but
//! as a regular `cargo test` — no nightly toolchain needed. Failures here mean
//! a regression of a previously-fixed bug.
//!
//! To add a new seed: drop the (preferably minimized) crash file into
//! `fuzz/regression/` (or a per-target subdir under it), no other action
//! required.

use std::fs;
use std::path::PathBuf;

use enough::Unstoppable;
use zenraw::{classify, FileFormat, RawDecodeConfig};

fn regression_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/regression")
}

/// Recursively collect every regular file under `dir`. Skips dotfiles and
/// silently tolerates a missing directory.
fn collect_seeds(dir: &PathBuf, out: &mut Vec<PathBuf>) {
    let read = match fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip dotfiles, README, and other meta files — they're documentation
        // for this directory, not crash seeds.
        if name.starts_with('.') || name.eq_ignore_ascii_case("README.md") {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_file() => out.push(path),
            Ok(t) if t.is_dir() => collect_seeds(&path, out),
            _ => {}
        }
    }
}

fn run_classify(input: &[u8]) {
    // Mirrors fuzz_targets/fuzz_classify.rs.
    let _ = classify(input);
}

fn run_decode(input: &[u8]) {
    // Mirrors fuzz_targets/fuzz_decode.rs: aggressive input filtering before
    // touching rawloader (which has known panic paths upstream that
    // catch_unwind cannot intercept under ASAN/abort).
    if input.len() < 4096 {
        return;
    }
    let format = classify(input);
    if format == FileFormat::Unknown || format == FileFormat::Jpeg {
        return;
    }
    let config = RawDecodeConfig::default();
    let _ = zenraw::decode(input, &config, &Unstoppable);
}

#[test]
fn fuzz_regression_seeds_do_not_panic() {
    let dir = regression_dir();
    let mut seeds = Vec::new();
    collect_seeds(&dir, &mut seeds);

    if seeds.is_empty() {
        eprintln!(
            "note: no regression seeds found under {} — nothing to check",
            dir.display()
        );
        return;
    }

    for path in seeds {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unnamed>")
            .to_owned();
        let input = fs::read(&path).unwrap_or_else(|e| panic!("read {name}: {e}"));

        // Each entry point may return Err but must not panic. If any panics,
        // the test fails with the seed name in the unwind message.
        run_classify(&input);
        run_decode(&input);

        eprintln!("ok: {name} ({} bytes)", input.len());
    }
}
