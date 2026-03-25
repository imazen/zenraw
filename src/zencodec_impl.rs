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
use zencodec::{
    ImageFormat, ImageFormatDefinition, ImageInfo, ImageSequence, Orientation, ResourceLimits,
};
use zenpixels::PixelDescriptor;

use crate::decode::{self, RawDecodeConfig};
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
        "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "rw2", "pef", "orf", "erf", "raf", "3fr",
        "iiq", "dcr", "kdc", "mrw", "rwl", "srw",
    ],
    "image/x-raw",
    &["image/x-raw", "image/x-dcraw"],
    false, // alpha
    false, // animation
    true,  // lossless
    true,  // lossy
    12,    // enough for TIFF header + BMFF ftyp detection
    detect_raw,
);

fn detect_dng(data: &[u8]) -> bool {
    crate::is_raw_file(data) && is_dng_header(data)
}

fn detect_raw(data: &[u8]) -> bool {
    crate::is_raw_file(data) && !is_dng_header(data)
}

fn is_dng_header(data: &[u8]) -> bool {
    crate::decode::is_dng_data(data)
}

/// Detect the image format (DNG vs generic RAW) from file bytes.
fn detect_format(data: &[u8]) -> ImageFormat {
    if crate::is_raw_file(data) && crate::decode::is_dng_data(data) {
        ImageFormat::Custom(&DNG_FORMAT)
    } else {
        ImageFormat::Custom(&RAW_FORMAT)
    }
}

/// Build an ImageInfo from our RawInfo + original file data.
///
/// Populates orientation, bit depth, and XMP metadata (when the xmp feature is enabled).
fn build_image_info(data: &[u8], raw_info: &decode::RawInfo) -> ImageInfo {
    let format = detect_format(data);
    let orientation = Orientation::from_exif(raw_info.orientation);

    let mut info = ImageInfo::new(raw_info.width, raw_info.height, format)
        .with_sequence(ImageSequence::Single)
        .with_orientation(orientation);

    if let Some(bd) = raw_info.bit_depth {
        info = info.with_bit_depth(bd);
    }

    // Attach XMP metadata when available
    #[cfg(feature = "xmp")]
    if let Some(xmp_xml) = crate::xmp::extract_xmp(data) {
        info = info.with_xmp(xmp_xml.into_bytes());
    }

    info
}

// ── Supported output descriptors ───────────────────────────────────────

static DECODE_DESCRIPTORS: &[PixelDescriptor] =
    &[PixelDescriptor::RGB8_SRGB, PixelDescriptor::RGBF32_LINEAR];

// ── Capabilities ───────────────────────────────────────────────────────

static RAW_DECODE_CAPABILITIES: DecodeCapabilities = DecodeCapabilities::EMPTY
    .with_exif(true)
    .with_stop(true)
    .with_enforces_max_pixels(true)
    .with_enforces_max_memory(true)
    .with_enforces_max_input_bytes(true);

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
        static FORMATS: [ImageFormat; 2] = [
            ImageFormat::Custom(&DNG_FORMAT),
            ImageFormat::Custom(&RAW_FORMAT),
        ];
        &FORMATS
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
    stop: Option<zencodec::StopToken>,
    limits: ResourceLimits,
}

impl<'a> zencodec::decode::DecodeJob<'a> for RawDecodeJob<'a> {
    type Error = At<RawError>;
    type Dec = RawDecoder<'a>;
    type StreamDec = Unsupported<At<RawError>>;
    type AnimationFrameDec = Unsupported<At<RawError>>;

    fn with_stop(mut self, stop: zencodec::StopToken) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn probe(&self, data: &[u8]) -> Result<ImageInfo, Self::Error> {
        let stop: &dyn enough::Stop = self
            .stop
            .as_ref()
            .map_or(&enough::Unstoppable as &dyn enough::Stop, |s| s);
        let info = crate::probe(data, stop)?;
        Ok(build_image_info(data, &info))
    }

    fn output_info(&self, data: &[u8]) -> Result<OutputInfo, Self::Error> {
        let info = self.probe(data)?;

        // Scene-referred: linear f32 by default, sRGB u8 only when gamma requested
        let descriptor = if self.config.apply_gamma {
            PixelDescriptor::RGB8_SRGB
        } else {
            PixelDescriptor::RGBF32_LINEAR
        };

        // When orientation is applied, output has display dimensions (may be swapped)
        let (w, h) = if self.config.apply_orientation {
            (info.display_width(), info.display_height())
        } else {
            (info.width, info.height)
        };

        Ok(OutputInfo::full_decode(w, h, descriptor))
    }

    fn decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<Self::Dec, Self::Error> {
        // Check input size limits
        self.limits
            .check_input_size(data.len() as u64)
            .map_err(|e| at!(RawError::LimitExceeded(e.to_string())))?;

        // Check if caller prefers linear f32
        let mut config = self.config.clone();
        for pref in preferred {
            if pref.format == zenpixels::PixelFormat::RgbF32 {
                config.apply_gamma = false;
                break;
            }
        }

        // Probe header for dimensions to check width/height/memory limits
        let stop: &dyn enough::Stop = self
            .stop
            .as_ref()
            .map_or(&enough::Unstoppable as &dyn enough::Stop, |s| s);
        let info = crate::probe(&data, stop)?;

        // Check dimension limits (max_width, max_height, max_pixels)
        self.limits
            .check_dimensions(info.width, info.height)
            .map_err(|e| at!(RawError::LimitExceeded(e.to_string())))?;

        // Check memory limits — estimate output buffer size
        let bytes_per_pixel: u64 = if config.apply_gamma { 3 } else { 12 }; // RGB8 or RGBF32
        let estimated_bytes = info.width as u64 * info.height as u64 * bytes_per_pixel;
        self.limits
            .check_memory(estimated_bytes)
            .map_err(|e| at!(RawError::LimitExceeded(e.to_string())))?;

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

    fn animation_frame_decoder(
        self,
        _data: Cow<'a, [u8]>,
        _preferred: &[PixelDescriptor],
    ) -> Result<Self::AnimationFrameDec, Self::Error> {
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
    stop: Option<zencodec::StopToken>,
}

impl<'a> Decode for RawDecoder<'a> {
    type Error = At<RawError>;

    fn decode(self) -> Result<DecodeOutput, Self::Error> {
        let stop: &dyn enough::Stop = match &self.stop {
            Some(s) => s,
            None => &enough::Unstoppable,
        };
        let output = crate::decode(&self.data, &self.config, stop)?;
        let info = build_image_info(&self.data, &output.info);
        Ok(DecodeOutput::new(output.pixels, info))
    }
}
