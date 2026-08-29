//! Behaviour pinned against a hand-built DNG, so it needs no external corpus.
//!
//! Two things are covered: `zenraw::probe` must be panic-isolated on the
//! rawloader backend, and the documented clamping of the f32 output modes must
//! stay true.
//!
//! # Panic isolation
//!
//! `decode::decode` has always wrapped its `rawloader::decode` call in
//! `std::panic::catch_unwind`, and both rawler entry points do the same (see
//! `tests/rawler_panic.rs`). `decode::probe` called `rawloader::decode`
//! directly, so a panic inside rawloader unwound straight through the host —
//! on the metadata-only path callers reach for *because* it is supposed to be
//! the cheap, safe one.
//!
//! The reproducer below is a hand-built 4×4 uncompressed DNG whose `CFAPattern`
//! tag has five entries. `rawloader::CFA::new` matches the pattern length
//! against `0 | 4 | 16 | 36 | 144` and `panic!("Unknown CFA size …")` on
//! anything else, so `DngDecoder::get_cfa` panics while assembling the
//! `RawImage`. Nothing about the file is otherwise malformed: it is a real
//! TIFF, it parses, and `synthetic_dng_is_recognised_as_raw` proves the same
//! builder with a 4-entry pattern probes all the way to metadata.
//!
//! **What these tests do and do not prove.** rawloader 0.37.2 wraps its own
//! `decode_unsafe` in `catch_unwind` (`decoders/mod.rs`), so a panicking
//! *decoder* is already converted to an `Err` before zenraw sees it — these
//! cases therefore pass with or without the zenraw-side guard, and they pin
//! the resulting error shape rather than the guard itself. `Buffer::new` runs
//! outside that upstream guard, the behaviour is not part of rawloader's
//! documented contract, and `tests/rawler_panic.rs` records exactly the same
//! reasoning for the rawler backend, so the zenraw-side `catch_unwind` remains
//! the defense-in-depth. That it is live on the probe path was verified by
//! injecting a `panic!` immediately inside the guard: with the guard the two
//! panic cases below return `RawError::Malformed`; with it removed they unwind
//! through the caller while `decode_contains_rawloader_panic` keeps passing.

#![cfg(all(feature = "rawloader", not(feature = "rawler")))]

use enough::Unstoppable;
use zenraw::{OutputMode, RawDecodeConfig};

// TIFF field types.
const BYTE: u16 = 1;
const ASCII: u16 = 2;
const SHORT: u16 = 3;
const LONG: u16 = 4;

/// One IFD entry, with its value either inline (≤ 4 bytes) or in the heap blob.
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    /// Raw value bytes, already little-endian.
    value: Vec<u8>,
}

fn short(tag: u16, v: u16) -> Entry {
    Entry {
        tag,
        kind: SHORT,
        count: 1,
        value: v.to_le_bytes().to_vec(),
    }
}

fn long(tag: u16, v: u32) -> Entry {
    Entry {
        tag,
        kind: LONG,
        count: 1,
        value: v.to_le_bytes().to_vec(),
    }
}

fn ascii(tag: u16, s: &str) -> Entry {
    let mut value = s.as_bytes().to_vec();
    value.push(0);
    Entry {
        tag,
        kind: ASCII,
        count: value.len() as u32,
        value,
    }
}

fn bytes(tag: u16, v: &[u8]) -> Entry {
    Entry {
        tag,
        kind: BYTE,
        count: v.len() as u32,
        value: v.to_vec(),
    }
}

/// Serialise a little-endian, single-IFD TIFF.
///
/// Layout: 8-byte header, IFD at offset 8, then the heap (out-of-line entry
/// values), then `strip` (the pixel data `StripOffsets` points at).
fn build_tiff(mut entries: Vec<Entry>, strip: &[u8], strip_offset_tag: u16) -> Vec<u8> {
    entries.sort_by_key(|e| e.tag);

    let ifd_start = 8usize;
    let ifd_len = 2 + entries.len() * 12 + 4;
    let heap_start = ifd_start + ifd_len;

    // Lay the heap out first so we know where the strip lands.
    let mut heap = Vec::new();
    let mut placements = Vec::with_capacity(entries.len());
    for e in &entries {
        if e.value.len() <= 4 {
            placements.push(None);
        } else {
            if heap.len() % 2 == 1 {
                heap.push(0); // keep values word-aligned
            }
            placements.push(Some(heap_start + heap.len()));
            heap.extend_from_slice(&e.value);
        }
    }
    let strip_offset = (heap_start + heap.len()) as u32;

    let mut out = Vec::new();
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&(ifd_start as u32).to_le_bytes());

    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (e, place) in entries.iter().zip(&placements) {
        out.extend_from_slice(&e.tag.to_le_bytes());
        out.extend_from_slice(&e.kind.to_le_bytes());
        out.extend_from_slice(&e.count.to_le_bytes());
        let field: [u8; 4] = match place {
            Some(off) => (*off as u32).to_le_bytes(),
            None if e.tag == strip_offset_tag => strip_offset.to_le_bytes(),
            None => {
                let mut f = [0u8; 4];
                f[..e.value.len()].copy_from_slice(&e.value);
                f
            }
        };
        out.extend_from_slice(&field);
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // no next IFD

    assert_eq!(out.len(), heap_start, "IFD length miscomputed");
    out.extend_from_slice(&heap);
    assert_eq!(out.len(), strip_offset as usize, "strip offset miscomputed");
    out.extend_from_slice(strip);
    out
}

/// A 4×4, 16-bit, uncompressed DNG whose `CFAPattern` has `pattern_len`
/// entries. Five is not a CFA size rawloader knows, so it panics.
fn dng_with_cfa_pattern_len(pattern_len: usize) -> Vec<u8> {
    const W: u32 = 4;
    const H: u32 = 4;

    // 0 = R, 1 = G, 2 = B in rawloader's CFAPattern encoding.
    let pattern: Vec<u8> = (0..pattern_len).map(|i| (i % 3) as u8).collect();

    let entries = vec![
        short(0x0100, W as u16),       // ImageWidth
        short(0x0101, H as u16),       // ImageLength
        short(0x0102, 16),             // BitsPerSample
        short(0x0103, 1),              // Compression = uncompressed
        short(0x0106, 32803),          // PhotometricInt = CFA (not LinearRaw)
        ascii(0x010F, "ZenRaw"),       // Make
        ascii(0x0110, "SyntheticDng"), // Model
        long(0x0111, 0),               // StripOffsets — patched by build_tiff
        short(0x0115, 1),              // SamplesPerPixel
        long(0x0117, W * H * 2),       // StripByteCounts
        bytes(0x828E, &pattern),       // CFAPattern
        bytes(0xC612, &[1, 4, 0, 0]),  // DNGVersion — selects DngDecoder
        short(0xC61D, u16::MAX),       // WhiteLevel
    ];

    // Uncompressed 16-bit sensor data, plus slack so the row reader cannot run
    // off the end of the buffer.
    let strip = vec![0x11u8; (W * H * 2) as usize + 64];
    build_tiff(entries, &strip, 0x0111)
}

/// The file must be a *valid* TIFF that rawloader parses — otherwise the test
/// would prove nothing about panic isolation, only about format sniffing.
#[test]
fn synthetic_dng_is_recognised_as_raw() {
    let data = dng_with_cfa_pattern_len(4);
    assert!(
        zenraw::is_raw_file(&data),
        "synthetic DNG should sniff as RAW"
    );
    // A 4-entry CFAPattern is a size rawloader accepts, so this one gets all
    // the way through metadata extraction.
    let info = zenraw::probe(&data, &Unstoppable).expect("4-entry CFAPattern must probe cleanly");
    assert_eq!(info.width, 4);
    assert_eq!(info.height, 4);
    assert_eq!(info.cfa_pattern, "RGBR");
}

/// **The regression.** A 5-entry `CFAPattern` panics inside rawloader; `probe`
/// must turn that into an error instead of unwinding through the caller.
#[test]
fn probe_contains_rawloader_panic() {
    let data = dng_with_cfa_pattern_len(5);
    let err = zenraw::probe(&data, &Unstoppable)
        .expect_err("a CFA size rawloader cannot parse must not probe successfully");
    assert!(
        matches!(err.error(), zenraw::RawError::Malformed(_)),
        "expected Malformed, got {err:?}"
    );
}

/// The same file through `decode`, which has always caught. Pinning both here
/// keeps the two entry points from drifting apart again.
#[test]
fn decode_contains_rawloader_panic() {
    let data = dng_with_cfa_pattern_len(5);
    let err = zenraw::decode(&data, &RawDecodeConfig::default(), &Unstoppable)
        .expect_err("a CFA size rawloader cannot parse must not decode successfully");
    assert!(
        matches!(err.error(), zenraw::RawError::Malformed(_)),
        "expected Malformed, got {err:?}"
    );
}

/// Other CFA lengths rawloader rejects, so the guard is not tuned to one input.
#[test]
fn probe_contains_panic_for_several_bad_cfa_sizes() {
    for len in [1usize, 2, 3, 5, 7, 9, 35, 37] {
        let data = dng_with_cfa_pattern_len(len);
        let result = zenraw::probe(&data, &Unstoppable);
        assert!(
            result.is_err(),
            "CFAPattern of length {len} should not probe successfully"
        );
    }
}

// ── Output-mode clamping ──────────────────────────────────────────────────
//
// `OutputMode::Linear` was documented "not clamped to [0, 1]" while
// `normalize_raw_data` clamps sensor samples against the black/white levels
// and `color::apply_color_matrix` clamps every component again as it writes
// it. The docs now say clamped; these pin the behaviour they describe so the
// two cannot drift apart again silently.

/// A DNG whose sensor samples span the full black→white range, so a missing
/// clamp shows up as out-of-range output rather than staying accidentally
/// inside `[0, 1]`. The default colour matrix has negative off-diagonal terms,
/// so saturated and near-saturated sites drive components past both ends.
fn dng_full_range() -> Vec<u8> {
    const W: u32 = 4;
    const H: u32 = 4;
    let entries = vec![
        short(0x0100, W as u16),
        short(0x0101, H as u16),
        short(0x0102, 16),
        short(0x0103, 1),
        short(0x0106, 32803),
        ascii(0x010F, "ZenRaw"),
        ascii(0x0110, "SyntheticDng"),
        long(0x0111, 0),
        short(0x0115, 1),
        long(0x0117, W * H * 2),
        bytes(0x828E, &[0, 1, 1, 2]), // RGGB
        bytes(0xC612, &[1, 4, 0, 0]),
        short(0xC61D, u16::MAX),
    ];
    // Alternate hard-black and hard-white sites: maximally chromatic input.
    let mut strip = Vec::new();
    for i in 0..(W * H) as usize {
        let v: u16 = if i % 2 == 0 { 0 } else { u16::MAX };
        strip.extend_from_slice(&v.to_le_bytes());
    }
    strip.extend_from_slice(&[0u8; 64]);
    build_tiff(entries, &strip, 0x0111)
}

fn decode_f32(mode: zenraw::OutputMode) -> Vec<f32> {
    let data = dng_full_range();
    let config = RawDecodeConfig::new().with_output(mode);
    let out = zenraw::decode(&data, &config, &Unstoppable).expect("synthetic DNG should decode");
    let bytes = out.pixels.copy_to_contiguous_bytes();
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// `OutputMode::Linear` is clamped — the colour matrix writes clamped
/// components, so nothing escapes `[0, 1]`.
#[test]
fn linear_output_is_clamped_to_unit_range() {
    let values = decode_f32(zenraw::OutputMode::Linear);
    assert!(!values.is_empty(), "no samples decoded");
    for (i, v) in values.iter().enumerate() {
        assert!(
            (0.0..=1.0).contains(v),
            "Linear sample {i} = {v} escaped [0, 1]; OutputMode::Linear documents a clamp"
        );
    }
}

/// `CameraRaw` is the mode that is genuinely not clamped above `1.0`.
///
/// Sensor samples are normalised with `clamp(0.0, 1.0)`, but demosaicing runs
/// next and the Malvar-He-Cutler kernels apply a Laplacian correction that only
/// floors at zero (`.max(0.0)`) — it has no ceiling, so a high-contrast
/// neighbourhood overshoots past `1.0`. `Linear` and `Develop` then lose that
/// overshoot to the colour matrix's clamp; `CameraRaw` skips the colour
/// pipeline entirely and keeps it.
#[test]
fn camera_raw_preserves_demosaic_overshoot_above_one() {
    let values = decode_f32(zenraw::OutputMode::CameraRaw);
    assert!(!values.is_empty(), "no samples decoded");
    for (i, v) in values.iter().enumerate() {
        assert!(*v >= 0.0, "CameraRaw sample {i} = {v} is negative");
        assert!(v.is_finite(), "CameraRaw sample {i} is not finite");
    }
    assert!(
        values.iter().any(|v| *v > 1.0),
        "CameraRaw is documented as unclamped above 1.0, but every sample was \
         inside [0, 1] — either the demosaic overshoot or the mode changed"
    );
}

/// `exposure_ev` is the one documented way past `1.0`: it multiplies *after*
/// the clamp, so it is what a caller reaches for to get headroom back.
#[test]
fn exposure_ev_multiplies_after_the_clamp() {
    let data = dng_full_range();
    let config = RawDecodeConfig::new()
        .with_output(zenraw::OutputMode::Linear)
        .with_exposure_ev(3.0);
    let out = zenraw::decode(&data, &config, &Unstoppable).expect("synthetic DNG should decode");
    let bytes = out.pixels.copy_to_contiguous_bytes();
    let values: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    assert!(
        values.iter().any(|v| *v > 1.0),
        "exposure_ev is applied after the clamp, so it must be able to exceed 1.0"
    );
}

// ── Sensor-layout reporting ───────────────────────────────────────────────

/// A 2×2 CFA is Bayer, and a 6×6 CFA is X-Trans. The rawloader backend used to
/// report `Bayer` for every `cpp == 1` sensor, so an X-Trans file was described
/// as Bayer to callers.
#[test]
fn sensor_layout_distinguishes_bayer_from_xtrans() {
    let bayer = zenraw::probe(&dng_with_cfa_pattern_len(4), &Unstoppable).expect("probe 2x2");
    assert_eq!(bayer.sensor_layout, zenraw::SensorLayout::Bayer);

    let xtrans = zenraw::probe(&dng_with_cfa_pattern_len(36), &Unstoppable).expect("probe 6x6");
    assert_eq!(
        xtrans.sensor_layout,
        zenraw::SensorLayout::XTrans,
        "a 36-entry CFAPattern is X-Trans's 6×6 tile, not Bayer"
    );
}

// ── End-to-end X-Trans demosaic through the public `decode` ───────────────

/// Build a DNG with the given square CFA tile whose sensor samples encode the
/// colour of each site, so the measured channel is checkable after decoding.
///
/// Sample values are chosen well inside the black/white range so normalisation
/// (`(v - black) / (white - black)`, clamped) is exact and distinct per colour.
fn dng_with_cfa_tile(tile: &[u8], tile_size: usize) -> (Vec<u8>, Vec<f32>) {
    assert_eq!(tile.len(), tile_size * tile_size);
    const W: u32 = 24;
    const H: u32 = 18;

    // 0 = R, 1 = G, 2 = B — the same encoding rawloader's CFAPattern uses.
    let level = |c: u8| -> u16 {
        match c {
            0 => 48_000,
            1 => 32_000,
            _ => 16_000,
        }
    };

    let mut strip = Vec::new();
    let mut expected = Vec::new();
    for row in 0..H as usize {
        for col in 0..W as usize {
            let c = tile[(row % tile_size) * tile_size + (col % tile_size)];
            let v = level(c);
            strip.extend_from_slice(&v.to_le_bytes());
            expected.push(v as f32 / u16::MAX as f32);
        }
    }
    strip.extend_from_slice(&[0u8; 64]);

    let entries = vec![
        short(0x0100, W as u16),
        short(0x0101, H as u16),
        short(0x0102, 16),
        short(0x0103, 1),
        short(0x0106, 32803),
        ascii(0x010F, "ZenRaw"),
        ascii(0x0110, "SyntheticDng"),
        long(0x0111, 0),
        short(0x0115, 1),
        long(0x0117, W * H * 2),
        bytes(0x828E, tile),
        bytes(0xC612, &[1, 4, 0, 0]),
        short(0xC61D, u16::MAX),
    ];
    (build_tiff(entries, &strip, 0x0111), expected)
}

/// The standard Fujifilm X-Trans 6×6 tile, in rawloader's colour encoding.
#[rustfmt::skip]
const XTRANS_TILE: [u8; 36] = [
    1, 2, 1, 1, 0, 1,
    0, 1, 0, 2, 1, 2,
    1, 2, 1, 1, 0, 1,
    1, 0, 1, 1, 2, 1,
    2, 1, 2, 0, 1, 0,
    1, 0, 1, 1, 2, 1,
];

/// **The X-Trans regression, end to end.** `zenraw::decode` on the default
/// backend must leave the channel the sensor actually measured untouched at
/// every site of a 6×6 CFA. `OutputMode::CameraRaw` skips white balance and the
/// colour matrix, so what comes out is the demosaic result directly.
///
/// Before the CFA-tile dispatch, the interior of this image was demosaiced with
/// the 2×2 Bayer kernel — which reads `cfa_tile[row & 1][col & 1]` and so wrote
/// the measured value into the wrong channel for most interior sites, while the
/// clamped border path used the real `color_at`.
///
/// rawloader's own camera database carries 19 models with a 36-character
/// (6×6) `color_pattern` — every X-Trans Fujifilm it supports — so this was
/// reachable on real files, not only on synthetic ones.
#[test]
fn decode_preserves_measured_channel_for_xtrans_cfa() {
    let (data, expected) = dng_with_cfa_tile(&XTRANS_TILE, 6);

    let info = zenraw::probe(&data, &Unstoppable).expect("probe 6x6 DNG");
    assert_eq!(info.sensor_layout, zenraw::SensorLayout::XTrans);

    for method in [
        zenraw::DemosaicMethod::Bilinear,
        zenraw::DemosaicMethod::MalvarHeCutler,
    ] {
        let config = RawDecodeConfig::new()
            .with_output(OutputMode::CameraRaw)
            .with_demosaic(method);
        let out = zenraw::decode(&data, &config, &Unstoppable).expect("decode 6x6 DNG");
        let w = out.info.width as usize;
        let h = out.info.height as usize;
        let bytes = out.pixels.copy_to_contiguous_bytes();
        let px: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        assert_eq!(px.len(), w * h * 3, "{method:?}: unexpected buffer size");

        for row in 0..h {
            for col in 0..w {
                let known = XTRANS_TILE[(row % 6) * 6 + (col % 6)] as usize;
                let want = expected[row * w + col];
                let got = px[(row * w + col) * 3 + known];
                assert!(
                    (got - want).abs() < 1e-4,
                    "{method:?}: measured channel {known} clobbered at ({row},{col}): \
                     got {got}, sensor said {want}"
                );
            }
        }
    }
}

/// The same end-to-end path on a 2×2 Bayer tile must be unchanged — this is the
/// byte-level guard that the dispatch did not disturb Bayer sensors.
#[test]
fn decode_preserves_measured_channel_for_bayer_cfa() {
    let tile: [u8; 4] = [0, 1, 1, 2]; // RGGB
    let (data, expected) = dng_with_cfa_tile(&tile, 2);

    let info = zenraw::probe(&data, &Unstoppable).expect("probe 2x2 DNG");
    assert_eq!(info.sensor_layout, zenraw::SensorLayout::Bayer);

    for method in [
        zenraw::DemosaicMethod::Bilinear,
        zenraw::DemosaicMethod::MalvarHeCutler,
    ] {
        let config = RawDecodeConfig::new()
            .with_output(OutputMode::CameraRaw)
            .with_demosaic(method);
        let out = zenraw::decode(&data, &config, &Unstoppable).expect("decode 2x2 DNG");
        let w = out.info.width as usize;
        let h = out.info.height as usize;
        let bytes = out.pixels.copy_to_contiguous_bytes();
        let px: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        for row in 0..h {
            for col in 0..w {
                let known = tile[(row % 2) * 2 + (col % 2)] as usize;
                let want = expected[row * w + col];
                let got = px[(row * w + col) * 3 + known];
                assert!(
                    (got - want).abs() < 1e-4,
                    "{method:?}: Bayer measured channel clobbered at ({row},{col})"
                );
            }
        }
    }
}
