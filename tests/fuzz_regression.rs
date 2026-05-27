//! Replay seed inputs from `fuzz/regression/` through every fuzz target
//! entry point. Shared scaffolding lives in `zen-fuzz-regress`.

use enough::Unstoppable;
use zenraw::{FileFormat, RawDecodeConfig, classify};
use zenutils_fuzz::RegressionSuite;

#[test]
fn fuzz_regression() {
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
