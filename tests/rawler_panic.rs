//! Regression test for issue #12: the rawler decode backend must be
//! panic-isolated so a malformed/crafted RAW cannot crash the host process.
//!
//! Background: the rawloader backend (`src/decode.rs`) has always wrapped its
//! decode call in `std::panic::catch_unwind`. The rawler backend
//! (`src/rawler_backend.rs`) historically called `rawler::decode` directly,
//! so a panic inside rawler unwound straight through the host. These tests
//! exercise the rawler path (via `zenraw::decode`, which routes to rawler when
//! the `rawler` feature is enabled) and assert that malformed input returns
//! `Err` rather than panicking.
//!
//! The cases below ALWAYS run under the `rawler` feature and need no external
//! corpus — reaching the `is_err()` assertion at all is the proof that the
//! panic was caught (an un-isolated panic would unwind through the test thread
//! and fail it as a panic instead). The positive "good decode still works
//! through the new wrapper" case is caller-gated via the `ZENRAW_RAW_CORPUS`
//! env var (a path to a real RAW/DNG file) so the skip decision is visible to
//! the caller, not buried as a runtime file-existence check.
//!
//! True panic-reproducer: `x3f-oob-panic-issue12.bin` is a 257-byte
//! fuzzer-discovered input (header `FOVb`, the Sigma X3F/Foveon magic) that
//! drives rawler 0.7.2 into an out-of-bounds slice panic inside
//! `rawler/src/decoders/x3f.rs` ("range start index 570425343 out of range for
//! slice of length 257"). rawler 0.7.2 happens to have its own inner
//! `catch_unwind` that currently converts this particular panic into an `Err`,
//! but that is not guaranteed on every rawler code path or future version — the
//! zenraw-side `catch_unwind` this test guards is the defense-in-depth that
//! keeps the host alive regardless. The seed is checked in under
//! `fuzz/regression/` and exercised both here and by `tests/fuzz_regression.rs`.

#![cfg(feature = "rawler")]

use enough::Unstoppable;
use zenraw::RawDecodeConfig;

/// Build an input that is long enough to clear the backend's `len() < 64`
/// short-input gate, so execution actually reaches the `rawler::decode` call
/// where panics could originate.
fn padded(prefix: &[u8], total_len: usize) -> Vec<u8> {
    let mut v = prefix.to_vec();
    v.resize(total_len.max(prefix.len()).max(128), 0u8);
    v
}

#[test]
fn rawler_truncated_garbage_returns_err_not_panic() {
    let config = RawDecodeConfig::default();

    // A grab-bag of inputs that pass the short-input length gate (>= 64 bytes)
    // and therefore reach `rawler::decode`, but are malformed in different
    // ways. None should decode; all must return Err (not panic / not abort).
    let cases: Vec<(&str, Vec<u8>)> = vec![
        // Little-endian TIFF/DNG magic followed by zeroed-out IFD bytes —
        // looks enough like a RAW container to get past format sniffing but
        // has no valid IFD structure.
        ("tiff-le-zeroed", padded(&[b'I', b'I', 42, 0, 8, 0, 0, 0], 512)),
        // Big-endian TIFF magic, likewise truncated/garbage afterwards.
        ("tiff-be-zeroed", padded(&[b'M', b'M', 0, 42, 0, 0, 0, 8], 512)),
        // Fujifilm RAF signature with garbage body.
        ("raf-garbage", padded(b"FUJIFILMCCD-RAW ", 512)),
        // All-0xFF block (no recognizable structure).
        ("all-ff", vec![0xFFu8; 512]),
        // Pseudo-random-ish noise.
        (
            "noise",
            (0..512u32).map(|i| (i.wrapping_mul(2654435761) >> 16) as u8).collect(),
        ),
    ];

    for (name, input) in cases {
        // The key assertion is simply that this call RETURNS (any Result) — if
        // the rawler path panicked without the catch_unwind wrapper, the test
        // thread would unwind here and the case would be reported as a panic.
        let result = zenraw::decode(&input, &config, &Unstoppable);
        assert!(
            result.is_err(),
            "case {name:?}: expected Err from malformed rawler input, got Ok"
        );
    }
}

/// True panic-reproducer: feed the checked-in 257-byte X3F/Foveon fuzz seed
/// that drives rawler into an out-of-bounds slice panic. With the zenraw-side
/// `catch_unwind` (and rawler's own inner guard) this must surface as `Err`,
/// never a process abort. Reaching the assertion proves the panic was contained.
#[test]
fn rawler_x3f_oob_panic_seed_returns_err_not_panic() {
    const X3F_OOB: &[u8] = include_bytes!("../fuzz/regression/fuzz_decode/x3f-oob-panic-issue12.bin");
    let config = RawDecodeConfig::default();
    let result = zenraw::decode(X3F_OOB, &config, &Unstoppable);
    assert!(
        result.is_err(),
        "expected Err from the X3F OOB-panic seed, got Ok"
    );
}

/// A truncated copy of an otherwise-valid DNG header is a realistic
/// "crafted/truncated RAW routed to rawler" input — exactly the shape that can
/// trip parser panics. We synthesize one without shipping a real RAW by taking
/// a minimal DNG-like TIFF prefix and truncating it mid-structure.
#[test]
fn rawler_truncated_dng_prefix_returns_err_not_panic() {
    let config = RawDecodeConfig::default();

    // Minimal little-endian TIFF header that points its first IFD beyond the
    // (truncated) buffer end — a classic way to provoke out-of-bounds reads in
    // RAW parsers. Must be >= 64 bytes to clear the length gate.
    let mut input = Vec::new();
    input.extend_from_slice(&[b'I', b'I', 42, 0]); // TIFF LE magic
    input.extend_from_slice(&0x10000u32.to_le_bytes()); // IFD offset way past EOF
    input.resize(256, 0u8); // body never reaches the claimed IFD offset

    let result = zenraw::decode(&input, &config, &Unstoppable);
    assert!(
        result.is_err(),
        "expected Err from truncated DNG-prefix input, got Ok"
    );
}

/// Positive case: a normal RAW still decodes successfully *through* the new
/// catch_unwind wrapper. Caller-gated via `ZENRAW_RAW_CORPUS` (path to a real
/// RAW/DNG file) so the dependency on external test data is explicit to the
/// caller rather than a silent runtime skip. When the env var is set, a missing
/// or undecodable file is a hard failure.
#[test]
fn rawler_normal_decode_succeeds_through_wrapper() {
    let Some(path) = std::env::var_os("ZENRAW_RAW_CORPUS") else {
        // Not configured by the caller; the panic-isolation regression is
        // covered by the defensive tests above, which always run.
        eprintln!(
            "rawler_normal_decode_succeeds_through_wrapper: set ZENRAW_RAW_CORPUS=<path-to-raw> \
             to exercise the good-decode path through the catch_unwind wrapper"
        );
        return;
    };

    let data = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("ZENRAW_RAW_CORPUS={path:?} could not be read: {e}");
    });

    let config = RawDecodeConfig::default();
    let result = zenraw::decode(&data, &config, &Unstoppable);
    let out = result.unwrap_or_else(|e| {
        panic!("ZENRAW_RAW_CORPUS={path:?} failed to decode through rawler wrapper: {e}");
    });
    assert!(
        out.info.width > 0 && out.info.height > 0,
        "decoded image has zero dimensions"
    );
}
