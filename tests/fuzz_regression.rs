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
    RegressionSuite::new("fuzz/regression")
        .target("classify", |input| {
            let _ = classify(input);
        })
        .target("decode", |input| {
            // Mirrors fuzz_targets/fuzz_decode.rs: aggressive input filtering
            // before touching rawloader (known panic paths upstream).
            if input.len() < 4096 {
                return;
            }
            let format = classify(input);
            if format == FileFormat::Unknown || format == FileFormat::Jpeg {
                return;
            }
            let config = RawDecodeConfig::default();
            let _ = zenraw::decode(input, &config, &Unstoppable);
        })
        .run();
}
