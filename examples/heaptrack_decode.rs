//! Heaptrack harness for camera-RAW decode-from-bytes allocation profiling.
//!
//! Profiles the production-critical path: `zenraw::decode(&bytes, &config, stop)` —
//! decoding a camera RAW / DNG file (untrusted input) all the way to a developed
//! `PixelBuffer` (white balance + color matrix + tone curve + gamma, the default
//! `OutputMode::Develop`). The goal is to surface allocation *pathologies* that
//! don't show up in a wall-clock benchmark: a high allocation *count* relative to
//! image size, per-pixel or per-row mallocs, large transient peaks, or unbounded
//! growth across repeated decodes (a leak). High allocation churn hurts most under
//! contended allocators (Windows, multi-threaded servers) where a single decode of
//! an untrusted upload turns into thousands of lock round-trips.
//!
//! NOTE: the bitstream parse + Bayer unpack is done by the third-party `rawloader`
//! backend (default feature); zenraw owns the demosaic + color-develop pipeline.
//! With the `rawler` feature enabled, `decode` prefers the `rawler` backend
//! instead. The report below notes which backend allocations originate in.
//!
//! Usage:
//!   cargo build -p zenraw --release --example heaptrack_decode               # default (rawloader)
//!   heaptrack ./target/release/examples/heaptrack_decode                     # default fixture
//!   heaptrack ./target/release/examples/heaptrack_decode <file.{nef,dng,..}> [iters]
//!
//! Then inspect:
//!   heaptrack_print heaptrack.heaptrack_decode.*.zst | less
//!
//! There is no committed RAW fixture (camera RAW files are large and licensing-
//! encumbered), so this defaults to `/mnt/v/input/raw-samples/nikon_d40.nef`
//! (a real Bayer-sensor NEF — a full demosaic + color develop, a meaningful per-row
//! work count for judging the allocation count), the same block-storage location
//! the crate's `profile_dng` example and tests read from. Pass any RAW path to
//! profile a different file; a large/high-MP RAW should be decoded fewer times
//! (pass a smaller `iters`).

use std::hint::black_box;
use std::path::PathBuf;

const DEFAULT_FIXTURE: &str = "/mnt/v/input/raw-samples/nikon_d40.nef";

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let path: PathBuf = match args.get(1) {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(DEFAULT_FIXTURE),
    };
    // Default 8 iterations; a leak shows up as monotonic growth across them, and a
    // healthy decoder's steady-state per-decode allocation count is iterations-stable.
    let iters: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);

    let data = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("failed to read {}: {e}", path.display());
        eprintln!(
            "(no RAW fixture is committed; fetch one to {DEFAULT_FIXTURE} or pass a RAW path)"
        );
        std::process::exit(1);
    });

    let config = zenraw::RawDecodeConfig::default();

    // Decode once up front to report the dimensions the alloc count is relative to.
    {
        let probe = zenraw::decode(&data, &config, &enough::Unstoppable).unwrap_or_else(|e| {
            eprintln!("probe decode failed for {}: {e}", path.display());
            std::process::exit(1);
        });
        eprintln!("fixture: {} ({} bytes on disk)", path.display(), data.len());
        eprintln!(
            "  decoded image: {}x{} ({:.2} MP), model {:?}, cfa {}",
            probe.info.width,
            probe.info.height,
            (f64::from(probe.info.width) * f64::from(probe.info.height)) / 1.0e6,
            probe.info.model,
            probe.info.cfa_pattern,
        );
    }

    eprintln!("decoding {iters}x via zenraw::decode(.., Develop) ...");

    let mut total_pixels: u64 = 0;
    for i in 0..iters {
        let out = zenraw::decode(&data, &config, &enough::Unstoppable).unwrap_or_else(|e| {
            eprintln!("decode iteration {i} failed: {e}");
            std::process::exit(1);
        });
        total_pixels += u64::from(out.pixels.width()) * u64::from(out.pixels.height());
        // Consume the decoded buffer so the optimizer can't elide the decode or the
        // allocation of the output PixelBuffer.
        black_box(out.pixels.width());
        black_box(out.pixels.height());
        black_box(&out.pixels);
    }

    eprintln!("done: decoded {total_pixels} total pixels across {iters} iterations");
}
