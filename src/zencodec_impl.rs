//! zencodec trait implementations for RAW/DNG.
//!
//! Provides [`RawDecoderConfig`] implementing the [`DecoderConfig`] trait from zencodec.
//! Feature-gated behind `zencodec`.

extern crate std;

use alloc::borrow::Cow;

use whereat::{At, at};
use zencodec::Unsupported;
use zencodec::decode::{
    Decode, DecodeCapabilities, DecodeOutput, DecodeRowSink, OutputInfo, SinkError,
};
use zencodec::{ImageFormat, ImageFormatDefinition, ImageInfo, ResourceLimits};
use zenpixels::PixelDescriptor;

use crate::decode::RawDecodeConfig;
use crate::error::RawError;

// ── Format definition ──────────────────────────────────────────────────

/// ImageFormatDefinition for DNG files.
pub static DNG_FORMAT: ImageFormatDefinition = ImageFormatDefinition::new(
    "dng",
    None,
    "Digital Negative",
    "dng",
    &["dng"],
    "image/x-adobe-dng",
    &["image/x-adobe-dng", "image/x-dng"],
    false, // alpha
    false, // animation
    true,  // lossless
    true,  // lossy (some DNG files use lossy JPEG compression)
    1024,  // need to parse IFD to detect
    detect_dng,
);

/// ImageFormatDefinition for generic camera RAW files.
pub static RAW_FORMAT: ImageFormatDefinition = ImageFormatDefinition::new(
    "raw",
    None,
    "Camera RAW",
    "raw",
    &[
        "cr2", "nef", "arw", "srf", "sr2", "rw2", "pef", "orf", "erf", "raf", "3fr", "iiq",
    ],
    "image/x-raw",
    &["image/x-raw", "image/x-dcraw"],
    false, // alpha
    false, // animation
    true,  // lossless
    true,  // lossy
    12,    // enough for TIFF header + magic detection
    detect_raw,
);

fn detect_dng(data: &[u8]) -> bool {
    crate::is_raw_file(data) && is_dng_header(data)
}

fn detect_raw(data: &[u8]) -> bool {
    crate::is_raw_file(data) && !is_dng_header(data)
}

fn is_dng_header(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }
    let is_tiff = (data[0] == b'I' && data[1] == b'I' && data[2] == 42 && data[3] == 0)
        || (data[0] == b'M' && data[1] == b'M' && data[2] == 0 && data[3] == 42);
    if !is_tiff {
        return false;
    }
    let search_len = data.len().min(4096);
    let le = data[0] == b'I';
    for i in 0..search_len.saturating_sub(1) {
        if le {
            if data[i] == 0x12 && data[i + 1] == 0xC6 {
                return true;
            }
        } else if data[i] == 0xC6 && data[i + 1] == 0x12 {
            return true;
        }
    }
    false
}

// ── Supported output descriptors ───────────────────────────────────────

static DECODE_DESCRIPTORS: &[PixelDescriptor] =
    &[PixelDescriptor::RGB8_SRGB, PixelDescriptor::RGBF32_LINEAR];

// ── Capabilities ───────────────────────────────────────────────────────

static RAW_DECODE_CAPABILITIES: DecodeCapabilities = DecodeCapabilities::EMPTY
    .with_exif(true)
    .with_stop(true)
    .with_enforces_max_pixels(true);

// ── DecoderConfig ──────────────────────────────────────────────────────

/// RAW/DNG decoder config implementing [`zencodec::decode::DecoderConfig`].
#[derive(Clone, Debug)]
pub struct RawDecoderConfig {
    inner: RawDecodeConfig,
}

impl RawDecoderConfig {
    /// Create with default settings.
    pub fn new() -> Self {
        Self {
            inner: RawDecodeConfig::default(),
        }
    }

    /// Create from an existing [`RawDecodeConfig`].
    pub fn from_config(config: RawDecodeConfig) -> Self {
        Self { inner: config }
    }
}

impl Default for RawDecoderConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl zencodec::decode::DecoderConfig for RawDecoderConfig {
    type Error = At<RawError>;
    type Job<'a> = RawDecodeJob<'a>;

    fn formats() -> &'static [ImageFormat] {
        // Custom formats — callers should register DNG_FORMAT and RAW_FORMAT
        // with the ImageFormatRegistry. We return an empty slice here because
        // these are Custom formats, not built-in ImageFormat variants.
        &[]
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        DECODE_DESCRIPTORS
    }

    fn capabilities() -> &'static DecodeCapabilities {
        &RAW_DECODE_CAPABILITIES
    }

    fn job(&self) -> Self::Job<'_> {
        RawDecodeJob {
            config: &self.inner,
            stop: None,
            limits: ResourceLimits::default(),
        }
    }
}

// ── DecodeJob ──────────────────────────────────────────────────────────

/// Per-operation decode job for RAW/DNG files.
pub struct RawDecodeJob<'a> {
    config: &'a RawDecodeConfig,
    stop: Option<&'a dyn enough::Stop>,
    limits: ResourceLimits,
}

impl<'a> zencodec::decode::DecodeJob<'a> for RawDecodeJob<'a> {
    type Error = At<RawError>;
    type Dec = RawDecoder<'a>;
    type StreamDec = Unsupported<At<RawError>>;
    type FullFrameDec = Unsupported<At<RawError>>;

    fn with_stop(mut self, stop: &'a dyn enough::Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn probe(&self, data: &[u8]) -> Result<ImageInfo, Self::Error> {
        let stop: &dyn enough::Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let info = crate::probe(data, stop)?;

        let format = if crate::is_raw_file(data) && is_dng_header(data) {
            ImageFormat::Custom(&DNG_FORMAT)
        } else {
            ImageFormat::Custom(&RAW_FORMAT)
        };

        Ok(ImageInfo::new(info.width, info.height, format)
            .with_frame_count(1)
            .with_bit_depth(16)) // Most RAW files are 12-14 bit, stored as 16
    }

    fn output_info(&self, data: &[u8]) -> Result<OutputInfo, Self::Error> {
        let info = self.probe(data)?;

        // Scene-referred: linear f32 by default, sRGB u8 only when gamma requested
        let descriptor = if self.config.apply_gamma {
            PixelDescriptor::RGB8_SRGB
        } else {
            PixelDescriptor::RGBF32_LINEAR
        };

        Ok(OutputInfo::full_decode(info.width, info.height, descriptor))
    }

    fn decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<Self::Dec, Self::Error> {
        // Check if caller prefers linear f32
        let mut config = self.config.clone();
        for pref in preferred {
            if pref.format == zenpixels::PixelFormat::RgbF32 {
                config.apply_gamma = false;
                break;
            }
        }

        // Apply resource limits
        if let Some(max_px) = self.limits.max_pixels {
            config.max_pixels = max_px;
        }

        Ok(RawDecoder {
            data,
            config,
            stop: self.stop,
        })
    }

    fn push_decoder(
        self,
        data: Cow<'a, [u8]>,
        sink: &mut dyn DecodeRowSink,
        preferred: &[PixelDescriptor],
    ) -> Result<OutputInfo, Self::Error> {
        let wrap = |e: SinkError| at!(RawError::Decode(e.to_string()));
        zencodec::helpers::copy_decode_to_sink(self, data, sink, preferred, wrap)
    }

    fn streaming_decoder(
        self,
        _data: Cow<'a, [u8]>,
        _preferred: &[PixelDescriptor],
    ) -> Result<Self::StreamDec, Self::Error> {
        Err(at!(RawError::Unsupported(
            "streaming decode not supported for RAW files".into()
        )))
    }

    fn full_frame_decoder(
        self,
        _data: Cow<'a, [u8]>,
        _preferred: &[PixelDescriptor],
    ) -> Result<Self::FullFrameDec, Self::Error> {
        Err(at!(RawError::Unsupported(
            "animation decode not supported for RAW files".into()
        )))
    }
}

// ── Decoder (one-shot) ─────────────────────────────────────────────────

/// One-shot RAW/DNG decoder.
pub struct RawDecoder<'a> {
    data: Cow<'a, [u8]>,
    config: RawDecodeConfig,
    stop: Option<&'a dyn enough::Stop>,
}

impl<'a> Decode for RawDecoder<'a> {
    type Error = At<RawError>;

    fn decode(self) -> Result<DecodeOutput, Self::Error> {
        let stop: &dyn enough::Stop = self.stop.unwrap_or(&enough::Unstoppable);
        let output = crate::decode(&self.data, &self.config, stop)?;

        let format = if decode::is_raw_file(&self.data) && is_dng_header(&self.data) {
            ImageFormat::Custom(&DNG_FORMAT)
        } else {
            ImageFormat::Custom(&RAW_FORMAT)
        };

        let info = ImageInfo::new(output.info.width, output.info.height, format)
            .with_frame_count(1)
            .with_bit_depth(16);

        Ok(DecodeOutput::new(output.pixels, info))
    }
}
