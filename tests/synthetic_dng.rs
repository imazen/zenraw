//! Behaviour pinned against a hand-built DNG, so it needs no external corpus.
//!
//! Covers rawloader panic isolation, camera matrix direction, and the
//! bounded and extended f32 output contracts.
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

#![cfg(any(feature = "rawloader", feature = "rawler"))]

use zenraw::{OutputMode, RawDecodeConfig};

#[path = "support/dng_builder.rs"]
mod dng_builder;
use dng_builder::*;

const XYZ_TO_CAMERA: [[f32; 3]; 4] = [
    [0.5, 0.5, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, 0.0],
];

fn calibrated_linear_dng() -> Vec<u8> {
    let (width, height) = (8u16, 6u16);
    let signed_matrix = [1i32, 1, 0, 0, 2, 0, 0, 0, 2];
    let entries = vec![
        short(0x0100, width),
        short(0x0101, height),
        short(0x0102, 16),
        short(0x0103, 1),
        short(0x0106, 34892), // LinearRaw, so the oracle has no CFA interpolation.
        ascii(0x010f, "ZenRaw"),
        ascii(0x0110, "AsymmetricMatrix"),
        long(0x0111, 0),
        short(0x0112, 6), // Rotate 90° clockwise.
        short(0x0115, 3),
        long(0x0116, u32::from(height)),
        long(0x0117, u32::from(width) * u32::from(height) * 6),
        short(0x011c, 1),
        bytes(0xc612, &[1, 4, 0, 0]),
        long(0xc61d, 1024),
        Entry {
            tag: 0xc621, // ColorMatrix1, row-major.
            kind: 10,    // SRATIONAL.
            count: 9,
            value: signed_matrix
                .into_iter()
                .flat_map(|n| n.to_le_bytes().into_iter().chain(2i32.to_le_bytes()))
                .collect(),
        },
        Entry {
            tag: 0xc628, // AsShotNeutral.
            kind: 5,     // RATIONAL.
            count: 3,
            value: [1u32; 6].into_iter().flat_map(u32::to_le_bytes).collect(),
        },
        short(0xc65a, 21), // CalibrationIlluminant1: D65.
    ];
    let mut strip: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| [1024 - 16 * x - 32 * y, 256, 512]))
        .flat_map(u16::to_le_bytes)
        .collect();
    strip.extend_from_slice(&[0; 64]);
    build_tiff(entries, &strip, 0x0111)
}

fn linear_config(mode: OutputMode, exposure_ev: f32) -> RawDecodeConfig {
    RawDecodeConfig::new()
        .with_output(mode)
        .with_crop(false)
        .with_orientation(false)
        .with_wb([1.0; 3])
        .with_exposure_ev(exposure_ev)
}

#[test]
fn probe_matrix_direction_and_extended_linear_highlights() {
    let data = calibrated_linear_dng();
    let probe = zenraw::probe(&data, None).unwrap();
    assert_eq!(probe.color_matrix, XYZ_TO_CAMERA);
    assert_eq!(probe.orientation, 6);

    let camera_raw =
        zenraw::decode(&data, &linear_config(OutputMode::CameraRaw, 0.0), None).unwrap();
    assert_eq!(
        camera_raw.pixels.spec(),
        &zenpixels::ImageSpec::RGBF32_LINEAR.with_primaries(zenpixels::Primaries::Unknown)
    );

    // Solve camera = [(X + Y) / 2, Y, Z] by substitution once.
    // The top-left sensor pixel has unity white balance with codes
    // [1024, 256, 512] at white level 1024 and black level zero.
    let camera = [1.0f64, 0.25, 0.5];
    let xyz = [2.0 * camera[0] - camera[1], camera[1], camera[2]];
    assert_eq!(xyz, [1.75, 0.25, 0.5]);
    for (row, value) in probe.color_matrix.iter().take(3).zip(camera) {
        let mapped: f64 = row.iter().zip(xyz).map(|(&a, b)| f64::from(a) * b).sum();
        assert_eq!(mapped, value);
    }

    // Reference evaluation used exact rational Bradford cone scaling from
    // equal-energy XYZ to D50 and Gaussian elimination against the D50 sRGB
    // matrix. The input XYZ was solved above by substitution.
    let pipeline =
        zenraw::dng_render::DngPipeline::from_raw_info(&probe, zenraw::OutputPrimaries::Srgb)
            .unwrap();
    let d50_expected = [4.84921275069852, -1.233107103329011, 0.6333115326714663];
    for (row, wanted) in pipeline.camera_to_output.iter().zip(d50_expected) {
        let value: f64 = row.iter().zip(camera).map(|(a, b)| a * b).sum();
        assert!((value - wanted).abs() < 5e-12, "{value} vs {wanted}");
    }

    // IEC sRGB D65 XYZ→RGB coefficients. Since the authored camera matrix
    // and its inverse preserve [1,1,1], the renderer's neutral normalization
    // is the sum of each XYZ→sRGB row.
    let expected = [
        [3.2404542, -1.5371385, -0.4985314],
        [-0.9692660, 1.8760108, 0.0415560],
        [0.0556434, -0.2040259, 1.0572252],
    ]
    .map(|row| row.iter().zip(xyz).map(|(&a, b)| a * b).sum::<f64>() / row.iter().sum::<f64>());
    assert!(expected[0] > 1.0 && expected[1] < 0.0);

    let extended =
        zenraw::decode(&data, &linear_config(OutputMode::LinearExtended, 0.0), None).unwrap();
    let view = extended.pixels.as_view();
    let values = view.as_contiguous_typed::<f32>().unwrap();
    assert_eq!((view.width(), view.rows()), (8, 6));
    assert_eq!(extended.info.color_matrix, probe.color_matrix);
    assert_eq!(extended.info.orientation, 6);
    assert_eq!(view.spec(), &zenpixels::ImageSpec::RGBF32_LINEAR);
    for (&actual, expected) in values.iter().zip(expected) {
        assert!(
            (f64::from(actual) - expected).abs() < 2e-6,
            "{actual} vs {expected}"
        );
    }

    let exposed =
        zenraw::decode(&data, &linear_config(OutputMode::LinearExtended, 3.0), None).unwrap();
    let exposed_view = exposed.pixels.as_view();
    let exposed_values = exposed_view.as_contiguous_typed::<f32>().unwrap();
    assert_eq!(exposed_values.len(), values.len());
    for (&unexposed, &exposed) in values.iter().zip(exposed_values) {
        assert_eq!(exposed.to_bits(), (unexposed * 8.0).to_bits());
    }

    let bounded = zenraw::decode(&data, &linear_config(OutputMode::Linear, 0.0), None).unwrap();
    let bounded_view = bounded.pixels.as_view();
    let bounded_values = bounded_view.as_contiguous_typed::<f32>().unwrap();
    assert!(bounded_values.iter().all(|v| (0.0..=1.0).contains(v)));
    assert_eq!(&bounded_values[..2], &[1.0, 0.0]);
    let bounded_exposed =
        zenraw::decode(&data, &linear_config(OutputMode::Linear, 3.0), None).unwrap();
    assert_eq!(
        bounded_exposed.pixels.as_view().row_as::<f32>(0).unwrap()[0].to_bits(),
        8.0f32.to_bits()
    );
}

#[cfg(feature = "zencodec")]
#[test]
fn extended_dng_correct_consumes_orientation_once_with_a_float_preference() {
    use std::borrow::Cow;
    use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
    use zencodec::{Orientation, OrientationHint};

    let data = calibrated_linear_dng();
    let config = linear_config(OutputMode::LinearExtended, 0.0);
    let stored = zenraw::decode(&data, &config, None).unwrap();
    assert_eq!(stored.info.orientation, 6);
    let stored_view = stored.pixels.as_view();
    let manual = zenpixels_convert::apply_orientation(&stored_view, Orientation::Rotate90).unwrap();

    let job = zenraw::RawDecoderConfig::from_config(config.clone())
        .job()
        .with_orientation(OrientationHint::Correct);
    let prediction = job.output_info(&data).unwrap();
    let decoded = job
        .decoder(Cow::Borrowed(&data), &[zenpixels::ImageSpec::RGBF32_LINEAR])
        .unwrap()
        .decode()
        .unwrap();
    let corrected = zenraw::decode(&data, &config.with_orientation(true), None).unwrap();
    assert_eq!(corrected.info.color_matrix, stored.info.color_matrix);
    assert_eq!(corrected.info.orientation, 1);
    assert_eq!(decoded.info().orientation, Orientation::Identity);
    assert_eq!(prediction.orientation_applied, Orientation::Rotate90);
    assert_eq!((decoded.width(), decoded.height()), (6, 8));
    assert_eq!(decoded.pixels().spec(), stored_view.spec());
    let bytes = manual.as_view().as_contiguous_bytes().unwrap();
    assert_eq!(decoded.pixels().as_contiguous_bytes().unwrap(), bytes);
    assert_eq!(
        corrected.pixels.as_view().as_contiguous_bytes().unwrap(),
        bytes
    );

    let row = decoded.pixels().row_as::<f32>(0).unwrap();
    let first_sensor_pixel = stored_view.row_as::<f32>(0).unwrap();
    // Sensor (0,0) maps to display (5,0).
    assert_eq!(row[15].to_bits(), first_sensor_pixel[0].to_bits());
    assert!(row[15] > 1.0 && row[16] < 0.0);
}

/// A 4×4, 16-bit, uncompressed DNG whose `CFAPattern` has `pattern_len`
/// entries. Five is not a CFA size rawloader knows, so it panics.
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
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
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn synthetic_dng_is_recognised_as_raw() {
    let data = dng_with_cfa_pattern_len(4);
    assert!(
        zenraw::is_raw_file(&data),
        "synthetic DNG should sniff as RAW"
    );
    // A 4-entry CFAPattern is a size rawloader accepts, so this one gets all
    // the way through metadata extraction.
    let info = zenraw::probe(&data, None).expect("4-entry CFAPattern must probe cleanly");
    assert_eq!(info.width, 4);
    assert_eq!(info.height, 4);
    assert_eq!(info.cfa_pattern, "RGBR");
}

/// **The regression.** A 5-entry `CFAPattern` panics inside rawloader; `probe`
/// must turn that into an error instead of unwinding through the caller.
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn probe_contains_rawloader_panic() {
    let data = dng_with_cfa_pattern_len(5);
    let err = zenraw::probe(&data, None)
        .expect_err("a CFA size rawloader cannot parse must not probe successfully");
    assert!(
        matches!(err.error(), zenraw::RawError::Malformed(_)),
        "expected Malformed, got {err:?}"
    );
}

/// The same file through `decode`, which has always caught. Pinning both here
/// keeps the two entry points from drifting apart again.
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn decode_contains_rawloader_panic() {
    let data = dng_with_cfa_pattern_len(5);
    let err = zenraw::decode(&data, &RawDecodeConfig::default(), None)
        .expect_err("a CFA size rawloader cannot parse must not decode successfully");
    assert!(
        matches!(err.error(), zenraw::RawError::Malformed(_)),
        "expected Malformed, got {err:?}"
    );
}

/// Other CFA lengths rawloader rejects, so the guard is not tuned to one input.
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn probe_contains_panic_for_several_bad_cfa_sizes() {
    for len in [1usize, 2, 3, 5, 7, 9, 35, 37] {
        let data = dng_with_cfa_pattern_len(len);
        let result = zenraw::probe(&data, None);
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
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
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

#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn decode_f32(mode: zenraw::OutputMode) -> Vec<f32> {
    let data = dng_full_range();
    let config = RawDecodeConfig::new().with_output(mode);
    let out = zenraw::decode(&data, &config, None).expect("synthetic DNG should decode");
    let bytes = out.pixels.as_view().contiguous_bytes().unwrap();
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
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
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

/// CameraRaw retains demosaic overshoot in camera coordinates.
///
/// Sensor samples are normalised with `clamp(0.0, 1.0)`, but demosaicing runs
/// next and the Malvar-He-Cutler kernels apply a Laplacian correction that only
/// floors at zero (`.max(0.0)`) — it has no ceiling, so a high-contrast
/// neighbourhood overshoots past `1.0`. `Linear` and `Develop` then lose that
/// overshoot to the colour matrix's clamp; `CameraRaw` skips the colour
/// pipeline entirely and keeps it.
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
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

/// In bounded Linear mode `exposure_ev` multiplies *after*
/// the clamp, so it is what a caller reaches for to get headroom back.
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn exposure_ev_multiplies_after_the_clamp() {
    let data = dng_full_range();
    let config = RawDecodeConfig::new()
        .with_output(zenraw::OutputMode::Linear)
        .with_exposure_ev(3.0);
    let out = zenraw::decode(&data, &config, None).expect("synthetic DNG should decode");
    let bytes = out.pixels.as_view().contiguous_bytes().unwrap();
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
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn sensor_layout_distinguishes_bayer_from_xtrans() {
    let bayer = zenraw::probe(&dng_with_cfa_pattern_len(4), None).expect("probe 2x2");
    assert_eq!(bayer.sensor_layout, zenraw::SensorLayout::Bayer);

    let xtrans = zenraw::probe(&dng_with_cfa_pattern_len(36), None).expect("probe 6x6");
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
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
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
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
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
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn decode_preserves_measured_channel_for_xtrans_cfa() {
    let (data, expected) = dng_with_cfa_tile(&XTRANS_TILE, 6);

    let info = zenraw::probe(&data, None).expect("probe 6x6 DNG");
    assert_eq!(info.sensor_layout, zenraw::SensorLayout::XTrans);

    for method in [
        zenraw::DemosaicMethod::Bilinear,
        zenraw::DemosaicMethod::MalvarHeCutler,
    ] {
        let config = RawDecodeConfig::new()
            .with_output(OutputMode::CameraRaw)
            .with_demosaic(method);
        let out = zenraw::decode(&data, &config, None).expect("decode 6x6 DNG");
        let w = out.info.width as usize;
        let h = out.info.height as usize;
        let bytes = out.pixels.as_view().contiguous_bytes().unwrap();
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
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn decode_preserves_measured_channel_for_bayer_cfa() {
    let tile: [u8; 4] = [0, 1, 1, 2]; // RGGB
    let (data, expected) = dng_with_cfa_tile(&tile, 2);

    let info = zenraw::probe(&data, None).expect("probe 2x2 DNG");
    assert_eq!(info.sensor_layout, zenraw::SensorLayout::Bayer);

    for method in [
        zenraw::DemosaicMethod::Bilinear,
        zenraw::DemosaicMethod::MalvarHeCutler,
    ] {
        let config = RawDecodeConfig::new()
            .with_output(OutputMode::CameraRaw)
            .with_demosaic(method);
        let out = zenraw::decode(&data, &config, None).expect("decode 2x2 DNG");
        let w = out.info.width as usize;
        let h = out.info.height as usize;
        let bytes = out.pixels.as_view().contiguous_bytes().unwrap();
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

// ── Cancellation ──────────────────────────────────────────────────────────
//
// The O(pixels) passes (normalize → demosaic → crop → orient → colour →
// quantise) all poll `stop` at bounded intervals. A 1 MP synthetic frame is
// big enough to cross every interior check cadence (per-32-row in the
// demosaic kernels, per-256-row in crop, per-256K elements elsewhere), so a
// counting token can prove the checks are actually reached mid-pipeline —
// not only at the pre-existing phase boundaries.

/// A DNG with a plain ascending gradient at `w`×`h`, Bayer RGGB.
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn dng_sized(w: u16, h: u16) -> Vec<u8> {
    let entries = vec![
        short(0x0100, w),
        short(0x0101, h),
        short(0x0102, 16),
        short(0x0103, 1),
        short(0x0106, 32803),
        ascii(0x010F, "ZenRaw"),
        ascii(0x0110, "SyntheticDng"),
        long(0x0111, 0),
        short(0x0115, 1),
        long(0x0117, w as u32 * h as u32 * 2),
        bytes(0x828E, &[0, 1, 1, 2]), // RGGB
        bytes(0xC612, &[1, 4, 0, 0]),
        short(0xC61D, u16::MAX),
    ];
    let mut strip = Vec::new();
    for i in 0..(w as usize * h as usize) {
        strip.extend_from_slice(&((i * 61 % u16::MAX as usize) as u16).to_le_bytes());
    }
    strip.extend_from_slice(&[0u8; 64]);
    build_tiff(entries, &strip, 0x0111)
}

/// `Stop` that cancels once `check()` has been called `budget` times.
/// `budget = u64::MAX` records calls without ever firing.
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
struct CountdownStop {
    remaining: std::sync::atomic::AtomicU64,
}

#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
impl CountdownStop {
    fn new(budget: u64) -> Self {
        Self {
            remaining: std::sync::atomic::AtomicU64::new(budget),
        }
    }
    fn calls(&self) -> u64 {
        u64::MAX - self.remaining.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
impl enough::Stop for CountdownStop {
    fn check(&self) -> Result<(), enough::StopReason> {
        use std::sync::atomic::Ordering::Relaxed;
        let mut remaining = self.remaining.load(Relaxed);
        loop {
            if remaining == 0 {
                return Err(enough::StopReason::Cancelled);
            }
            match self
                .remaining
                .compare_exchange_weak(remaining, remaining - 1, Relaxed, Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(current) => remaining = current,
            }
        }
    }

    fn should_stop(&self) -> bool {
        self.remaining.load(std::sync::atomic::Ordering::Relaxed) == 0
    }
    fn may_stop(&self) -> bool {
        true
    }
}

/// A token that never fires must not perturb output bytes anywhere in the
/// pipeline — the checks are a pure observation point.
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn decode_with_non_firing_stop_is_byte_identical() {
    let data = dng_sized(1024, 1024);
    let config = RawDecodeConfig::new().with_output(OutputMode::Develop);

    let base = zenraw::decode(&data, &config, None)
        .expect("baseline decode")
        .pixels
        .as_view()
        .contiguous_bytes()
        .unwrap()
        .into_owned();

    let metered = CountdownStop::new(u64::MAX);
    let out = zenraw::decode(&data, &config, Some(&how_far::StopOnly::borrowed(&metered)))
        .expect("decode with non-firing stop")
        .pixels
        .as_view()
        .contiguous_bytes()
        .unwrap()
        .into_owned();

    assert_eq!(base, out, "stop checks changed the output bytes");
    assert!(
        metered.calls() > 10,
        "only {} interior checks fired on a 1 MP decode — the pipeline polls \
     far more often than that",
        metered.calls()
    );
}

/// A token that fires mid-pipeline must surface `RawError::Stopped`, not a
/// partial image. Small budgets land inside normalize/demosaic — the spans
/// that were unpolled before this work.
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn decode_cancelled_mid_pipeline_returns_stopped() {
    let data = dng_sized(1024, 1024);
    let config = RawDecodeConfig::new().with_output(OutputMode::Develop);

    for budget in [0u64, 2, 6, 20] {
        let stop = CountdownStop::new(budget);
        let err = zenraw::decode(&data, &config, Some(&how_far::StopOnly::borrowed(&stop)))
            .expect_err("decode should report cancellation");
        assert!(
            matches!(
                err.error(),
                zenraw::RawError::Stopped(enough::StopReason::Cancelled)
            ),
            "budget {budget}: expected Stopped(Cancelled), got {err:?}"
        );
    }
}

/// Cancellation is an error even when the token only reports it once.
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn single_poll_cancellation_is_never_lost() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct FireOnce {
        calls: AtomicUsize,
        fire_at: usize,
    }
    impl enough::Stop for FireOnce {
        fn check(&self) -> Result<(), enough::StopReason> {
            if self.calls.fetch_add(1, Ordering::Relaxed) == self.fire_at {
                Err(enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let data = dng_sized(128, 96);
    let config = RawDecodeConfig::new().with_output(OutputMode::Develop);
    let count = FireOnce {
        calls: AtomicUsize::new(0),
        fire_at: usize::MAX,
    };
    zenraw::decode(&data, &config, Some(&how_far::StopOnly::borrowed(&count)))
        .expect("uncancelled decode");
    for fire_at in 0..count.calls.load(Ordering::Relaxed) {
        let stop = FireOnce {
            calls: AtomicUsize::new(0),
            fire_at,
        };
        let err = zenraw::decode(&data, &config, Some(&how_far::StopOnly::borrowed(&stop)))
            .expect_err("each observed cancellation must terminate decoding");
        assert!(
            matches!(err.error(), zenraw::RawError::Stopped(_)),
            "poll {fire_at}: {err}"
        );
    }
}

/// Incremental (sans-I/O) decode through `dyn_incremental_decoder`: the
/// testkit's incremental contract on synthetic Bayer and X-Trans DNGs.
#[cfg(feature = "zencodec")]
#[test]
#[cfg(all(feature = "rawloader", not(feature = "rawler")))]
fn incremental_contract_holds() {
    use zencodec::decode::DecoderConfig as _;
    use zenraw::RawDecoderConfig;

    assert!(RawDecoderConfig::capabilities().incremental());
    for (name, data) in [
        ("full_range", dng_full_range()),
        ("bayer", dng_with_cfa_tile(&[0, 1, 1, 2], 2).0),
        ("xtrans", dng_with_cfa_tile(&XTRANS_TILE, 6).0),
    ] {
        zencodec_testkit::check_incremental_capability_honesty(RawDecoderConfig::new(), &data)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}
