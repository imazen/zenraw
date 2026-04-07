#![no_main]
use libfuzzer_sys::fuzz_target;
use enough::Unstoppable;
use zenraw::{RawDecodeConfig, classify, FileFormat};

fuzz_target!(|data: &[u8]| {
    // Note: rawloader has many internal panics on malformed inputs
    // (slice index out of bounds in TIFF/RAF/X3F/etc parsers).
    // These are upstream bugs in rawloader-0.37.1, not zenraw bugs.
    // Under ASAN, catch_unwind doesn't work (panic=abort), so we
    // filter inputs aggressively. In production, zenraw wraps
    // rawloader calls with catch_unwind for non-ASAN builds.

    // RAW files need substantial headers + sensor data
    if data.len() < 4096 {
        return;
    }

    // Only fuzz formats we can classify
    let format = classify(data);
    if format == FileFormat::Unknown || format == FileFormat::Jpeg {
        return;
    }

    let config = RawDecodeConfig::default();
    let _ = zenraw::decode(data, &config, &Unstoppable);
});
