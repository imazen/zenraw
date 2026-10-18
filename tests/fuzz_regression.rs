//! Replay seed inputs from `fuzz/regression/` through every fuzz target
//! entry point. Shared scaffolding lives in `zen-fuzz-regress`.

use enough::Unstoppable;
use std::path::Path;
use zenraw::{FileFormat, RawDecodeConfig, classify};
use zenutils_fuzz::RegressionSuite;

/// Lower bound on the replayable seed corpus committed under `fuzz/regression/`.
///
/// `RegressionSuite` treats a missing or empty seed directory as a clean no-op,
/// so an emptied, renamed, or never-checked-out corpus would let this test pass
/// without replaying a single seed. Pinning the floor makes that a loud failure.
/// Raise this when seeds are added; only lower it when deleting seeds on purpose.
const MIN_SEEDS: usize = 1;

/// Count the files `RegressionSuite::run` will actually replay, using its own
/// filters: recurse into subdirectories, skip dotfiles, `*.md` and `*.txt`.
fn replayable_seeds(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut found = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            found += replayable_seeds(&path);
        } else if path.is_file() {
            let lower = name.to_ascii_lowercase();
            if !lower.ends_with(".md") && !lower.ends_with(".txt") {
                found += 1;
            }
        }
    }
    found
}

/// Fail loudly when the corpus this suite exists to replay is not there.
fn assert_corpus_present() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/regression");
    let found = replayable_seeds(&dir);
    assert!(
        found >= MIN_SEEDS,
        "{} holds {found} replayable seeds, expected at least {MIN_SEEDS} — \
         the committed regression corpus is missing or was renamed, which would \
         otherwise let this test pass without replaying anything",
        dir.display()
    );
}

#[test]
fn fuzz_regression() {
    assert_corpus_present();
    let suite = RegressionSuite::new("fuzz/regression")
        .target("classify", replay_classify)
        .target("decode", replay_decode);
    #[cfg(feature = "inventory")]
    let suite = suite.target("inventory", replay_inventory);
    suite.run();
}

fn replay_classify(input: &[u8]) {
    let _ = classify(input);
}

fn replay_decode(input: &[u8]) {
    // Same caller-side filter as fuzz_targets/fuzz_decode.rs.
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

#[cfg(feature = "inventory")]
fn replay_inventory(input: &[u8]) {
    use zencodec::decode::{DecodeJob, DecoderConfig};
    let inv = zenraw::RawDecoderConfig::new()
        .job()
        .inventory(input)
        .unwrap();
    assert_eq!(inv.input_len(), input.len() as u64);
    inv.validate().unwrap();
}

// Exact minimized bytes from PR 20 (3f8bc711), stored as text. Each saved
// input exercises every fuzz entry point under the target's own filters.
#[cfg(feature = "inventory")]
#[test]
fn saved_inventory_regressions() {
    for (text, len) in [
        (
            include_str!("support/inventory-regressions/rawloader-panic-inside-the-gate.hex"),
            64,
        ),
        (
            include_str!("support/inventory-regressions/table-overlap-wrap.hex"),
            644,
        ),
    ] {
        let hex: String = text.split_whitespace().collect();
        assert_eq!(hex.len(), len * 2);
        let input: Vec<u8> = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        replay_classify(&input);
        replay_decode(&input);
        replay_inventory(&input);
    }
}
