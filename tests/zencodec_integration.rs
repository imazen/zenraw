//! Integration tests for the zencodec trait implementation.
//!
//! These exercise RawDecoderConfig/RawDecodeJob/RawDecoder, which wrap
//! the core decode pipeline behind zencodec's DecoderConfig trait.

#![cfg(feature = "zencodec")]

use std::borrow::Cow;

use zencodec::ResourceLimits;
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zenraw::{OutputMode, OutputPrimaries, RawDecodeConfig};

// ── Helpers ─────────────────────────────────────────────────────────────

fn find_raw_file() -> Option<Vec<u8>> {
    let dirs = ["/mnt/v/input/raw-samples/", "/mnt/v/input/fivek/dng/"];
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(
                ext.to_lowercase().as_str(),
                "dng" | "cr2" | "nef" | "arw" | "rw2" | "orf"
            ) && let Ok(data) = std::fs::read(&path)
                && zenraw::is_raw_file(&data)
            {
                eprintln!("Using: {}", path.display());
                return Some(data);
            }
        }
    }
    None
}

// ── Static methods ──────────────────────────────────────────────────────

#[test]
fn formats_returns_dng_and_raw() {
    let formats = zenraw::RawDecoderConfig::formats();
    assert_eq!(formats.len(), 2);
}

#[test]
fn supported_descriptors_non_empty() {
    let descs = zenraw::RawDecoderConfig::supported_descriptors();
    assert!(descs.len() >= 2);
}

#[test]
fn capabilities_flags() {
    let caps = zenraw::RawDecoderConfig::capabilities();
    assert!(caps.stop());
    assert!(caps.enforces_max_pixels());
}

// ── Config construction ─────────────────────────────────────────────────

#[test]
fn config_new_and_from_config() {
    let _default = zenraw::RawDecoderConfig::new();
    let custom = RawDecodeConfig::new()
        .with_output(OutputMode::Linear)
        .with_target(OutputPrimaries::DisplayP3);
    let _wrapped = zenraw::RawDecoderConfig::from_config(custom);
}

// ── Probe + output_info ─────────────────────────────────────────────────

#[test]
fn probe_and_output_info() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::new();
    let job = config.job();

    let info = job.probe(&data).expect("probe failed");
    assert!(info.width > 0);
    assert!(info.height > 0);

    let output_info = job.output_info(&data).expect("output_info failed");
    assert!(output_info.width > 0);
    assert!(output_info.height > 0);
    assert_eq!(
        output_info.native_format.format,
        zenpixels::PixelFormat::Rgb16
    );
}

#[test]
fn output_info_linear() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::from_config(
        RawDecodeConfig::new().with_output(OutputMode::Linear),
    );
    let job = config.job();
    let output_info = job.output_info(&data).expect("output_info failed");
    assert_eq!(
        output_info.native_format.format,
        zenpixels::PixelFormat::RgbF32
    );
}

#[test]
fn output_info_camera_raw_primaries_unknown() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::from_config(
        RawDecodeConfig::new().with_output(OutputMode::CameraRaw),
    );
    let job = config.job();
    let output_info = job.output_info(&data).expect("output_info failed");
    assert_eq!(
        output_info.native_format.primaries,
        zenpixels::ColorPrimaries::Unknown
    );
}

#[test]
fn output_info_display_p3_primaries() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::from_config(
        RawDecodeConfig::new().with_target(OutputPrimaries::DisplayP3),
    );
    let job = config.job();
    let output_info = job.output_info(&data).expect("output_info failed");
    assert_eq!(
        output_info.native_format.primaries,
        zenpixels::ColorPrimaries::DisplayP3
    );
}

// ── Full decode via zencodec ────────────────────────────────────────────

#[test]
fn full_decode_via_zencodec() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::new();
    let job = config.job();
    let decoder = job
        .decoder(Cow::Borrowed(&data), &[])
        .expect("decoder creation failed");
    let output = decoder.decode().expect("decode failed");

    assert!(output.width() > 0);
    assert!(output.height() > 0);
}

#[test]
fn decode_prefers_linear_when_requested() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::new();
    let job = config.job();
    let decoder = job
        .decoder(
            Cow::Borrowed(&data),
            &[zenpixels::PixelDescriptor::RGBF32_LINEAR],
        )
        .expect("decoder creation failed");
    let output = decoder.decode().expect("decode failed");

    assert_eq!(output.descriptor().format, zenpixels::PixelFormat::RgbF32);
}

// ── Resource limits ─────────────────────────────────────────────────────

#[test]
fn limit_max_pixels_rejects() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::new();
    let job = config
        .job()
        .with_limits(ResourceLimits::default().with_max_pixels(100));
    let result = job.decoder(Cow::Borrowed(&data), &[]);
    assert!(result.is_err());
}

#[test]
fn limit_max_input_bytes_rejects() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::new();
    let job = config
        .job()
        .with_limits(ResourceLimits::default().with_max_input_bytes(10));
    let result = job.decoder(Cow::Borrowed(&data), &[]);
    assert!(result.is_err());
}

#[test]
fn limit_max_memory_rejects() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::new();
    let job = config
        .job()
        .with_limits(ResourceLimits::default().with_max_memory(100));
    let result = job.decoder(Cow::Borrowed(&data), &[]);
    assert!(result.is_err());
}

// ── Unsupported paths ───────────────────────────────────────────────────

#[test]
fn streaming_decoder_unsupported() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::new();
    let job = config.job();
    let result = job.streaming_decoder(Cow::Borrowed(&data), &[]);
    assert!(result.is_err());
}

#[test]
fn animation_decoder_unsupported() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::new();
    let job = config.job();
    let result = job.animation_frame_decoder(Cow::Borrowed(&data), &[]);
    assert!(result.is_err());
}

// ── Decode with non-sRGB primaries ──────────────────────────────────────

#[test]
fn decode_bt2020_primaries() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::from_config(
        RawDecodeConfig::new()
            .with_target(OutputPrimaries::Bt2020)
            .with_output(OutputMode::Linear),
    );
    let job = config.job();
    let decoder = job
        .decoder(Cow::Borrowed(&data), &[])
        .expect("decoder creation failed");
    let output = decoder.decode().expect("decode failed");

    assert_eq!(
        output.descriptor().primaries,
        zenpixels::ColorPrimaries::Bt2020
    );
}

#[test]
fn decode_display_p3_develop() {
    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let config = zenraw::RawDecoderConfig::from_config(
        RawDecodeConfig::new().with_target(OutputPrimaries::DisplayP3),
    );
    let job = config.job();
    let decoder = job
        .decoder(Cow::Borrowed(&data), &[])
        .expect("decoder creation failed");
    let output = decoder.decode().expect("decode failed");

    assert_eq!(
        output.descriptor().primaries,
        zenpixels::ColorPrimaries::DisplayP3
    );
}

// ── estimate_decode_resources (deterministic, no corpus) ─────────────────

#[test]
fn estimate_decode_resources_scales_and_is_serial() {
    use zencodec::estimate::{ComputeEnvironment, ImageCharacteristics, ThreadingInformation};
    use zenpixels::PixelDescriptor;

    let config = zenraw::RawDecoderConfig::new();
    let compute = ComputeEnvironment::new().with_cores(8);

    // A 6 MP image: the modelled peak is dominated by the RGB-f32 working set
    // (≈ 3 × W·H·12 B) plus fixed overhead — in the right ballpark of the
    // measured 245.9 MiB / 6.12 MP develop anchor (see the impl + CHANGELOG).
    let img6 = ImageCharacteristics::new(3000, 2000, PixelDescriptor::RGB16_SRGB);
    let est6 = config.estimate_decode_resources(&img6, &compute);

    let peak6 = est6.peak_memory_bytes_est().expect("peak modelled");
    let pixels6 = 3000u64 * 2000;
    // ≥ 3 RGB-f32 frames (the concurrent working set); well under 10× (sanity).
    assert!(
        peak6 >= pixels6 * 12 * 3,
        "peak below the RGB-f32 working set"
    );
    assert!(peak6 < pixels6 * 12 * 10, "peak implausibly large");
    // Matches the measured 6.12 MP / 245.9 MiB anchor within a small factor.
    assert!(
        (200 << 20..=320 << 20).contains(&peak6),
        "6 MP peak {peak6} outside the measured-anchor band"
    );

    // Larger image → strictly larger peak (monotonic in pixels).
    let img24 = ImageCharacteristics::new(6000, 4000, PixelDescriptor::RGB16_SRGB);
    let est24 = config.estimate_decode_resources(&img24, &compute);
    assert!(
        est24.peak_memory_bytes_est().unwrap() > peak6,
        "peak must grow with pixel count"
    );

    // RAW decode is single-threaded per image: wall time does not scale with
    // cores, so 1-core and 8-core estimates must agree.
    assert_eq!(est6.threading(), Some(ThreadingInformation::SERIAL));
    let est6_1core = config.estimate_decode_resources(&img6, &ComputeEnvironment::new());
    assert_eq!(
        est6.wall_ms(),
        est6_1core.wall_ms(),
        "serial decode wall time must not scale with cores"
    );
}

// ── AllocPreference (corpus-gated, follows this file's convention) ────────

/// Decoding under `AllocPreference::Fallible` (the `try_reserve` path) and
/// `Infallible` (the `vec!` path) must produce byte-identical pixels to the
/// default (`CodecDefault`) decode — the allocation strategy never changes the
/// output. Corpus-gated like every other decode test here (the
/// `find_raw_file()` directories are the caller-visible gate).
#[test]
fn fallible_alloc_decode_matches_default() {
    use zencodec::AllocPreference;

    let Some(data) = find_raw_file() else {
        eprintln!("Skipping: no RAW files found");
        return;
    };

    let decode_bytes = |pref: Option<AllocPreference>| -> Vec<u8> {
        let job = zenraw::RawDecoderConfig::new().job();
        let job = match pref {
            Some(p) => job.with_limits(ResourceLimits::none().with_prefer_fallible_allocations(p)),
            None => job,
        };
        let out = job
            .decoder(Cow::Borrowed(&data), &[])
            .expect("decoder creation failed")
            .decode()
            .expect("decode failed");
        out.into_buffer().copy_to_contiguous_bytes()
    };

    let default = decode_bytes(None); // CodecDefault
    let fallible = decode_bytes(Some(AllocPreference::Fallible));
    let infallible = decode_bytes(Some(AllocPreference::Infallible));

    assert_eq!(
        default, fallible,
        "Fallible decode must be byte-identical to the default decode"
    );
    assert_eq!(
        default, infallible,
        "Infallible decode must be byte-identical to the default decode"
    );
}
