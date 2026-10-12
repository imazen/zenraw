//! Structural inventory fuzzer: `DecodeJob::inventory` must never panic, and
//! every inventory it returns must cover the input exactly and validate.
//!
//! The inventory runs the backend's own container parse (rawloader
//! `decode_dummy`) under the decode path's `catch_unwind`, and rawloader
//! panics on malformed input. libfuzzer-sys aborts on every panic from its
//! hook, before any `catch_unwind` runs, so the target lets panics located
//! in the backend crates unwind to that guard, as they do in production.
//! Any other panic, or a backend panic nothing catches, still aborts.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zencodec::decode::{DecodeJob, DecoderConfig};
use zenraw::RawDecoderConfig;

static HOOK: std::sync::Once = std::sync::Once::new();

fuzz_target!(|data: &[u8]| {
    HOOK.call_once(|| {
        let abort = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let backend = info.location().is_some_and(|l| {
                let f = l.file();
                f.contains("/rawloader-") || f.contains("/rawler-")
            });
            if !backend {
                abort(info);
            }
        }));
    });
    let job = RawDecoderConfig::new().job();
    match job.inventory(data) {
        Ok(inv) => {
            assert_eq!(inv.input_len(), data.len() as u64);
            if let Err(e) = inv.validate() {
                panic!("invalid inventory: {e}\n{inv}");
            }
        }
        // Only the part cap may fail, and it needs far more than a fuzz input.
        Err(e) => panic!("inventory failed: {e}"),
    }
});
