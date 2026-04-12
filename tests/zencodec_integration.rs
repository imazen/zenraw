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
