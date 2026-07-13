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
    ImageFormat, ImageFormatDefinition, ImageInfo, ImageSequence, Orientation, OrientationHint,
    ResourceLimits,
};
use zenpixels::PixelDescriptor;

use crate::decode::{self, OutputMode, RawDecodeConfig};
use crate::error::{RawError, RawLimitKind};

/// Wrap a `zencodec::LimitExceeded` (from a `ResourceLimits::check_*` call)
/// into a native `RawError::LimitExceeded`, preserving the checked
/// [`RawLimitKind`] instead of collapsing it into an opaque string.
fn wrap_limit(e: zencodec::LimitExceeded) -> RawError {
    RawError::LimitExceeded(RawLimitKind::from_zencodec(e.kind()), e.to_string())
}

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
    let orientation = Orientation::from_exif(raw_info.orientation as u8).unwrap_or_default();

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

// ══════════════════════════════════════════════════════════════════════
// Orientation (EXIF tag 274) — adapter-only baking
// ══════════════════════════════════════════════════════════════════════
//
// The native RAW pipeline (`crate::decode`) carries an `apply_orientation`
// flag that, when true, bakes the image's *intrinsic* EXIF orientation into
// the decoded buffer (via `crate::orient::apply_orientation`) at the f32 stage
// and reports `orientation = 1`. When false it leaves the pixels in their
// stored sensor orientation and reports the intrinsic tag.
//
// The zencodec adapter follows zencodec's orientation contract instead — it
// honors the caller's `OrientationHint` (default `Preserve`), which diverges
// from the native default (`apply_orientation = true`). To keep the bake path
// uniform across all hints, the adapter always runs the native decode with
// `apply_orientation = false` (stored pixels + intrinsic tag) and then bakes
// the *resolved* orientation itself onto the decoded buffer:
//   - `Preserve` (default): no bake. Report stored (coded) dims + intrinsic
//     EXIF tag; the caller applies the orientation (e.g. via `display_width`).
//   - `Correct` / `CorrectAndTransform` / `ExactTransform`: physically bake the
//     resolved orientation into the buffer (via `crate::orient::
//     apply_orientation_bytes`), then report the display dims + `Identity`.
//
// Baking is a pure pixel permutation, so doing it on the final RGB16/RGBF32
// buffer is bit-exact identical to the native f32-stage bake (no resampling,
// no arithmetic) — pixels are preserved exactly.

/// Whether `hint` puts the decoder on the bake path (transform the pixels) vs.
/// the preserve path (leave them stored-orientation).
///
/// This is the local equivalent of
/// [`OrientationHint::bakes()`](zencodec::OrientationHint::bakes), kept inlined
/// so the orientation-adapter tests can exercise it directly without
/// constructing a full decode. [`Preserve`](OrientationHint::Preserve) is the
/// only hint that leaves pixels untouched; every other hint bakes.
fn hint_bakes(hint: OrientationHint) -> bool {
    !matches!(hint, OrientationHint::Preserve)
}

/// Resolve the net [`Orientation`] to bake into the *stored* pixels for `hint`,
/// given the image's intrinsic EXIF `intrinsic` orientation.
///
/// - [`Preserve`](OrientationHint::Preserve): nothing to bake — returns
///   [`Identity`](Orientation::Identity) (callers gate on [`hint_bakes`] first,
///   so this arm is a defensive default).
/// - [`Correct`](OrientationHint::Correct): the intrinsic orientation (applying
///   it to the stored pixels yields the upright image).
/// - [`ExactTransform`](OrientationHint::ExactTransform): the literal transform,
///   ignoring EXIF.
/// - [`CorrectAndTransform`](OrientationHint::CorrectAndTransform): the intrinsic
///   correction first, then the requested transform.
fn resolve_orientation(hint: OrientationHint, intrinsic: Orientation) -> Orientation {
    match hint {
        OrientationHint::Preserve => Orientation::Identity,
        OrientationHint::Correct => intrinsic,
        OrientationHint::ExactTransform(t) => t,
        OrientationHint::CorrectAndTransform(t) => intrinsic.then(t),
        // `OrientationHint` is `#[non_exhaustive]`; treat any future variant as a
        // no-op bake rather than guessing — the reported tag stays consistent.
        _ => Orientation::Identity,
    }
}

/// Bake `hint` into a decoded [`DecodeOutput`] whose pixels are in *stored*
/// orientation (the native decode ran with `apply_orientation = false`) and
/// whose `ImageInfo` reports the stored dims + the intrinsic EXIF tag.
///
/// On the [`Preserve`](OrientationHint::Preserve) path ([`hint_bakes`] is
/// `false`) the output is returned unchanged.
///
/// Otherwise the resolved orientation (see [`resolve_orientation`]) is physically
/// applied to the pixels via [`crate::orient::apply_orientation_bytes`], and the
/// reported `ImageInfo` is rewritten to the baked buffer's dimensions with
/// [`Orientation::Identity`] (the pixels are final — no orientation remains to
/// apply). The intrinsic EXIF orientation is read from the `ImageInfo` the decode
/// path already computed, so it matches what `probe()` reports.
fn apply_orientation_to_output(output: DecodeOutput, hint: OrientationHint) -> DecodeOutput {
    if !hint_bakes(hint) {
        return output;
    }
    let mut info = output.info().clone();
    let intrinsic = info.orientation;
    let resolved = resolve_orientation(hint, intrinsic);

    let buf = output.into_buffer();

    // Even when the resolved transform is Identity (e.g. `Correct` on an upright
    // image) we still rewrite the reported orientation to Identity so a consumer
    // never double-applies the (now-stale) intrinsic tag. The pixel copy is
    // skipped in that case.
    if resolved.is_identity() {
        info = info.with_orientation(Orientation::Identity);
        return DecodeOutput::new(buf, info);
    }

    let descriptor = buf.descriptor();
    let bpp = descriptor.bytes_per_pixel();
    let w = buf.width() as usize;
    let h = buf.height() as usize;
    // `copy_to_contiguous_bytes` returns exactly `w * h * bpp` tight bytes,
    // stripping any alignment offset / stride padding — so the byte baker (which
    // assumes a tight `w * h * bpp` layout) is always fed clean pixel data.
    let bytes = buf.copy_to_contiguous_bytes();

    let (baked, nw, nh) =
        crate::orient::apply_orientation_bytes(&bytes, w, h, bpp, resolved.to_exif() as u16);

    let baked_buf = zenpixels::PixelBuffer::from_vec(baked, nw as u32, nh as u32, descriptor)
        .expect("apply_orientation_to_output: baked buffer matches output geometry");

    // `ImageInfo` has no dimension setter; the fields are public. Report the
    // baked buffer's geometry + Identity (pixels are now final).
    info.width = nw as u32;
    info.height = nh as u32;
    info = info.with_orientation(Orientation::Identity);

    DecodeOutput::new(baked_buf, info)
}

/// Rewrite a probe [`ImageInfo`] (stored dims + intrinsic EXIF tag, as produced
/// by [`build_image_info`]) to match what [`apply_orientation_to_output`] reports
/// for `hint`.
///
/// On the [`Preserve`](OrientationHint::Preserve) path the info is returned
/// unchanged. On a bake hint the dims are set to the resolved orientation's
/// output geometry and the tag becomes [`Orientation::Identity`].
fn report_probe_for_hint(mut info: ImageInfo, hint: OrientationHint) -> ImageInfo {
    if !hint_bakes(hint) {
        return info;
    }
    let resolved = resolve_orientation(hint, info.orientation);
    let (ow, oh) = resolved.output_dimensions(info.width, info.height);
    info.width = ow;
    info.height = oh;
    info.with_orientation(Orientation::Identity)
}

// ── Supported output descriptors ───────────────────────────────────────

static DECODE_DESCRIPTORS: &[PixelDescriptor] =
    &[PixelDescriptor::RGB16_SRGB, PixelDescriptor::RGBF32_LINEAR];

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
    type Job<'a> = RawDecodeJob;

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

    fn estimate_decode_resources(
        &self,
        image: &zencodec::estimate::ImageCharacteristics,
        compute: &zencodec::estimate::ComputeEnvironment,
    ) -> zencodec::estimate::ResourceEstimate {
        use zencodec::estimate::{ResourceEstimate, ThreadingInformation};

        let pixels = image.pixels();

        // The RAW develop pipeline's working set is dominated by `RGB f32`
        // buffers (12 B/px): the normalized sensor buffer, the demosaic output,
        // and a transient crop/orientation copy. The output buffer
        // (`image.descriptor()`, RGB16 6 B/px or RGBF32 12 B/px) is smaller than
        // or equal to those.
        //
        // Anchor (heaptrack, `examples/heaptrack_decode.rs`, see CHANGELOG):
        // a 6.12 MP NEF develops at **245.9 MiB** peak — ≈ 3.3× the 73 MiB
        // RGB-f32 intermediate. The model below — fixed overhead + 3× the
        // RGB-f32 frame — reproduces that point (24 MiB + 6.12 MP × 36 B ≈
        // 245 MiB). The fixed term also covers the ~20 MiB one-time `rawloader`
        // camera-metadata DB the first decode deserializes and caches.
        const FIXED_OVERHEAD: u64 = 24 << 20; // 24 MiB
        const RGB_F32_BYTES_PER_PX: u64 = 12;
        // Three RGB-f32-equivalent buffers live concurrently at the peak
        // (normalized + demosaic output + crop/orient transient).
        let working_set = pixels.saturating_mul(RGB_F32_BYTES_PER_PX * 3);
        let peak = FIXED_OVERHEAD.saturating_add(working_set);

        // Single-threaded per image (rawloader / rawler decode serially).
        // Throughput is a conservative ~50 Mpix/s placeholder — the heaptrack
        // harness measures memory, not wall time, so this is not yet
        // benchmark-calibrated.
        const THROUGHPUT_MPIX_PER_S: u64 = 50;
        let wall_ms = pixels / (THROUGHPUT_MPIX_PER_S * 1000).max(1);

        ResourceEstimate::new(peak, wall_ms)
            .with_threading(ThreadingInformation::SERIAL)
            .at_cores(compute.cores())
    }

    fn job<'a>(self) -> Self::Job<'a> {
        RawDecodeJob {
            config: self.inner,
            stop: None,
            limits: ResourceLimits::default(),
            orientation: OrientationHint::Preserve,
        }
    }
}

// ── DecodeJob ──────────────────────────────────────────────────────────

/// Per-operation decode job for RAW/DNG files.
pub struct RawDecodeJob {
    config: RawDecodeConfig,
    stop: Option<zencodec::StopToken>,
    limits: ResourceLimits,
    /// How to resolve the EXIF orientation during decode. Default
    /// [`OrientationHint::Preserve`] — pixels stay in stored sensor orientation
    /// and the intrinsic tag is reported. See
    /// [`DecodeJob::with_orientation`](zencodec::decode::DecodeJob::with_orientation).
    ///
    /// Note this diverges from the native [`RawDecodeConfig`] default
    /// (`apply_orientation = true`); the adapter follows zencodec's `Preserve`
    /// default and bakes itself on the non-`Preserve` hints.
    orientation: OrientationHint,
}

impl<'a> zencodec::decode::DecodeJob<'a> for RawDecodeJob {
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

    fn with_orientation(mut self, hint: OrientationHint) -> Self {
        self.orientation = hint;
        self
    }

    fn probe(&self, data: &[u8]) -> Result<ImageInfo, Self::Error> {
        let stop: &dyn enough::Stop = self
            .stop
            .as_ref()
            .map_or(&enough::Unstoppable as &dyn enough::Stop, |s| s);
        let info = crate::probe(data, stop)?;
        let info = build_image_info(data, &info);
        // Report consistently with what `decode()` produces under this hint.
        // `Preserve` (default) keeps the stored dims + intrinsic EXIF tag that
        // `build_image_info` already set; the bake hints report the display
        // (post-orientation) dims + `Identity`.
        Ok(report_probe_for_hint(info, self.orientation))
    }

    fn output_info(&self, data: &[u8]) -> Result<OutputInfo, Self::Error> {
        // Probe for stored dims + intrinsic orientation (independent of hint).
        let stop: &dyn enough::Stop = self
            .stop
            .as_ref()
            .map_or(&enough::Unstoppable as &dyn enough::Stop, |s| s);
        let raw_info = crate::probe(data, stop)?;
        let info = build_image_info(data, &raw_info);

        let descriptor =
            match self.config.output {
                OutputMode::Develop => PixelDescriptor::RGB16_SRGB
                    .with_primaries(self.config.target.to_color_primaries()),
                OutputMode::Linear => PixelDescriptor::RGBF32_LINEAR
                    .with_primaries(self.config.target.to_color_primaries()),
                OutputMode::CameraRaw => PixelDescriptor::RGBF32_LINEAR
                    .with_primaries(zenpixels::ColorPrimaries::Unknown),
            };

        // Report the post-orientation output geometry + the transform the decoder
        // will bake. `Preserve` bakes nothing (output = stored dims, `Identity`
        // recorded); a bake hint outputs the resolved orientation's dims and
        // records it.
        let resolved = if hint_bakes(self.orientation) {
            resolve_orientation(self.orientation, info.orientation)
        } else {
            Orientation::Identity
        };
        let (ow, oh) = resolved.output_dimensions(info.width, info.height);

        Ok(OutputInfo::full_decode(ow, oh, descriptor).with_orientation_applied(resolved))
    }

    fn decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<Self::Dec, Self::Error> {
        // Check input size limits
        self.limits
            .check_input_size(data.len() as u64)
            .map_err(|e| at!(wrap_limit(e)))?;

        // Check if caller prefers linear f32
        let mut config = self.config.clone();
        for pref in preferred {
            if pref.format == zenpixels::PixelFormat::RgbF32 {
                config.output = OutputMode::Linear;
                break;
            }
        }

        // Honor the caller's allocation-fallibility preference at the zencodec
        // boundary. The big untrusted-sized RGB-f32 / sensor buffers default to
        // the fallible (`try_reserve`) path; explicit `Fallible` / `Infallible`
        // override every site. The direct `decode()` API leaves this
        // `CodecDefault` → behaviour unchanged. See [`crate::alloc_util`].
        config.alloc_pref = self.limits.prefer_fallible_allocations.into();

        // The adapter owns orientation: always decode in stored sensor
        // orientation (intrinsic tag reported), then bake the resolved hint on
        // the decoded buffer in `Decode::decode`. This keeps `Preserve` /
        // `Correct` / `*Transform` on one uniform path and lets the arbitrary
        // transforms (which the native decode cannot produce) be applied.
        config.apply_orientation = false;

        // Probe header for dimensions to check width/height/memory limits
        let stop: &dyn enough::Stop = self
            .stop
            .as_ref()
            .map_or(&enough::Unstoppable as &dyn enough::Stop, |s| s);
        let info = crate::probe(&data, stop)?;

        // Check dimension limits (max_width, max_height, max_pixels)
        self.limits
            .check_dimensions(info.width, info.height)
            .map_err(|e| at!(wrap_limit(e)))?;

        // Check memory limits — estimate output buffer size
        let bytes_per_pixel: u64 = match config.output {
            OutputMode::Develop => 6,                         // RGB16
            OutputMode::Linear | OutputMode::CameraRaw => 12, // RGBF32
        };
        let estimated_bytes = info.width as u64 * info.height as u64 * bytes_per_pixel;
        self.limits
            .check_memory(estimated_bytes)
            .map_err(|e| at!(wrap_limit(e)))?;

        // Apply resource limits
        if let Some(max_px) = self.limits.max_pixels {
            config.max_pixels = max_px;
        }

        Ok(RawDecoder {
            data,
            config,
            stop: self.stop,
            orientation: self.orientation,
        })
    }

    fn push_decoder(
        self,
        data: Cow<'a, [u8]>,
        sink: &mut dyn DecodeRowSink,
        preferred: &[PixelDescriptor],
    ) -> Result<OutputInfo, Self::Error> {
        // A `DecodeRowSink` failure is an output-boundary fault (the sink is
        // caller-supplied — writing decoded rows into it failed), the same
        // convention `Io` covers for an output sink elsewhere in this crate.
        let wrap = |e: SinkError| at!(RawError::Io(e.to_string()));
        zencodec::helpers::copy_decode_to_sink(self, data, sink, preferred, wrap)
    }

    fn streaming_decoder(
        self,
        _data: Cow<'a, [u8]>,
        _preferred: &[PixelDescriptor],
    ) -> Result<Self::StreamDec, Self::Error> {
        Err(at!(RawError::UnsupportedOperation(
            zencodec::UnsupportedOperation::RowLevelDecode
        )))
    }

    fn animation_frame_decoder(
        self,
        _data: Cow<'a, [u8]>,
        _preferred: &[PixelDescriptor],
    ) -> Result<Self::AnimationFrameDec, Self::Error> {
        Err(at!(RawError::UnsupportedOperation(
            zencodec::UnsupportedOperation::AnimationDecode
        )))
    }
}

// ── Decoder (one-shot) ─────────────────────────────────────────────────

/// One-shot RAW/DNG decoder.
pub struct RawDecoder<'a> {
    data: Cow<'a, [u8]>,
    config: RawDecodeConfig,
    stop: Option<zencodec::StopToken>,
    /// Resolved from [`RawDecodeJob::orientation`]; drives the bake path in
    /// [`Decode::decode`](zencodec::decode::Decode::decode). The native decode
    /// runs with `apply_orientation = false`, so the buffer here is always in
    /// stored sensor orientation regardless of this hint.
    orientation: OrientationHint,
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
        let decoded = DecodeOutput::new(output.pixels, info);
        // `Preserve` (default) returns the decoded output unchanged: stored
        // pixels + stored dims + intrinsic EXIF tag. A bake hint physically
        // rotates the buffer and rewrites the reported dims/tag to match.
        Ok(apply_orientation_to_output(decoded, self.orientation))
    }
}

// ══════════════════════════════════════════════════════════════════════
// Orientation adapter-logic tests (deterministic, no RAW fixture)
// ══════════════════════════════════════════════════════════════════════
//
// These exercise the adapter's orientation contract directly against synthetic
// `DecodeOutput`/`ImageInfo` values: no decode, no corpus, fully reproducible
// in CI. End-to-end decode-bake on real RAW files is covered (corpus-gated) in
// `tests/orientation.rs`.

#[cfg(test)]
mod orientation_tests {
    use super::*;
    use alloc::vec::Vec;
    use zenpixels::{PixelBuffer, PixelDescriptor};

    /// Build a synthetic RGB16 `DecodeOutput`: a `w×h` image whose pixel (r,c)
    /// is three identical u16 channels of value `(r*16 + c) + 1`, with the given
    /// intrinsic EXIF `Orientation` recorded on the `ImageInfo` (mirroring what
    /// the native decode reports when `apply_orientation = false`).
    fn synth_output(w: u32, h: u32, intrinsic: Orientation) -> DecodeOutput {
        let bpp = 6usize; // RGB16
        let mut bytes = Vec::with_capacity(w as usize * h as usize * bpp);
        for r in 0..h {
            for c in 0..w {
                let id = ((r * 16 + c) as u16) + 1;
                for _ in 0..3 {
                    bytes.extend_from_slice(&id.to_le_bytes());
                }
            }
        }
        let buf = PixelBuffer::from_vec(bytes, w, h, PixelDescriptor::RGB16_SRGB).unwrap();
        let info =
            ImageInfo::new(w, h, ImageFormat::Custom(&DNG_FORMAT)).with_orientation(intrinsic);
        DecodeOutput::new(buf, info)
    }

    /// The three-u16 pixel id at (row, col) in a tight RGB16 buffer's bytes.
    fn id_at(bytes: &[u8], width: u32, row: u32, col: u32) -> u16 {
        let bpp = 6usize;
        let i = (row as usize * width as usize + col as usize) * bpp;
        let id = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        // All three channels must match — proves whole-pixel moves.
        for ch in 0..3 {
            let v = u16::from_le_bytes([bytes[i + ch * 2], bytes[i + ch * 2 + 1]]);
            assert_eq!(v, id, "pixel ({row},{col}) channel {ch} mismatch");
        }
        id
    }

    #[test]
    fn hint_bakes_only_false_for_preserve() {
        assert!(!hint_bakes(OrientationHint::Preserve));
        assert!(hint_bakes(OrientationHint::Correct));
        assert!(hint_bakes(OrientationHint::ExactTransform(
            Orientation::Rotate90
        )));
        assert!(hint_bakes(OrientationHint::CorrectAndTransform(
            Orientation::Rotate180
        )));
    }

    #[test]
    fn resolve_orientation_semantics() {
        let intrinsic = Orientation::Rotate90;
        // Preserve → nothing to bake.
        assert_eq!(
            resolve_orientation(OrientationHint::Preserve, intrinsic),
            Orientation::Identity
        );
        // Correct → the intrinsic.
        assert_eq!(
            resolve_orientation(OrientationHint::Correct, intrinsic),
            Orientation::Rotate90
        );
        // ExactTransform → literal, EXIF ignored.
        assert_eq!(
            resolve_orientation(
                OrientationHint::ExactTransform(Orientation::FlipH),
                intrinsic
            ),
            Orientation::FlipH
        );
        // CorrectAndTransform → intrinsic.then(t).
        assert_eq!(
            resolve_orientation(
                OrientationHint::CorrectAndTransform(Orientation::Rotate90),
                intrinsic
            ),
            intrinsic.then(Orientation::Rotate90)
        );
    }

    #[test]
    fn preserve_returns_stored_dims_and_intrinsic_tag_unbaked() {
        // 3×2 image, intrinsic Rotate90. Preserve must NOT touch pixels or dims.
        let out = synth_output(3, 2, Orientation::Rotate90);
        let before: Vec<u8> = out.pixels().contiguous_bytes().to_vec();
        let baked = apply_orientation_to_output(out, OrientationHint::Preserve);
        assert_eq!((baked.width(), baked.height()), (3, 2), "stored dims");
        assert_eq!(
            baked.info().orientation,
            Orientation::Rotate90,
            "intrinsic tag preserved"
        );
        assert_eq!(
            baked.pixels().contiguous_bytes().as_ref(),
            &before[..],
            "pixels unbaked"
        );
    }

    #[test]
    fn correct_bakes_intrinsic_reports_display_dims_and_identity() {
        // 3×2 image, intrinsic Rotate90 (EXIF 6) → display 2×3, tag Identity.
        let out = synth_output(3, 2, Orientation::Rotate90);
        let baked = apply_orientation_to_output(out, OrientationHint::Correct);
        assert_eq!((baked.width(), baked.height()), (2, 3), "display dims");
        assert_eq!(baked.info().orientation, Orientation::Identity);
        // Pixel oracle for Rotate90 CW: display(dr,dc) ← src(h-1-dc, dr).
        // src ids: (0,0)=1 (0,1)=2 (0,2)=3 ; (1,0)=17 (1,1)=18 (1,2)=19.
        let b = baked.pixels().contiguous_bytes();
        let w = baked.width();
        assert_eq!(id_at(&b, w, 0, 0), 17); // ← src(1,0)
        assert_eq!(id_at(&b, w, 0, 1), 1); // ← src(0,0)
        assert_eq!(id_at(&b, w, 2, 0), 19); // ← src(1,2)
        assert_eq!(id_at(&b, w, 2, 1), 3); // ← src(0,2)
    }

    #[test]
    fn exact_transform_ignores_intrinsic_exif() {
        // Intrinsic Rotate180, but ExactTransform(Rotate90) must apply Rotate90.
        let with_exif = synth_output(3, 2, Orientation::Rotate180);
        let baked_a = apply_orientation_to_output(
            with_exif,
            OrientationHint::ExactTransform(Orientation::Rotate90),
        );
        // Same source pixels, intrinsic Identity → ExactTransform(Rotate90)
        // must produce byte-identical output (EXIF was ignored).
        let no_exif = synth_output(3, 2, Orientation::Identity);
        let baked_b = apply_orientation_to_output(
            no_exif,
            OrientationHint::ExactTransform(Orientation::Rotate90),
        );
        assert_eq!((baked_a.width(), baked_a.height()), (2, 3));
        assert_eq!(baked_a.info().orientation, Orientation::Identity);
        assert_eq!(
            baked_a.pixels().contiguous_bytes().as_ref(),
            baked_b.pixels().contiguous_bytes().as_ref(),
            "ExactTransform must ignore the intrinsic EXIF orientation"
        );
    }

    #[test]
    fn correct_and_transform_composes_intrinsic_then_extra() {
        // Intrinsic FlipH, extra Rotate90 → net = FlipH.then(Rotate90).
        let intrinsic = Orientation::FlipH;
        let extra = Orientation::Rotate90;
        let net = intrinsic.then(extra);

        let composed = apply_orientation_to_output(
            synth_output(3, 2, intrinsic),
            OrientationHint::CorrectAndTransform(extra),
        );
        // Equivalent to applying the net transform directly with no EXIF.
        let direct = apply_orientation_to_output(
            synth_output(3, 2, Orientation::Identity),
            OrientationHint::ExactTransform(net),
        );
        assert_eq!(
            (composed.width(), composed.height()),
            (direct.width(), direct.height()),
            "composed dims match the net transform"
        );
        assert_eq!(composed.info().orientation, Orientation::Identity);
        assert_eq!(
            composed.pixels().contiguous_bytes().as_ref(),
            direct.pixels().contiguous_bytes().as_ref(),
            "CorrectAndTransform must equal intrinsic.then(extra)"
        );
    }

    #[test]
    fn correct_on_upright_image_is_noop_but_reports_identity() {
        // Intrinsic Identity + Correct: no pixel change, dims unchanged, tag
        // becomes Identity (it already was) — and the buffer is returned as-is.
        let out = synth_output(3, 2, Orientation::Identity);
        let before: Vec<u8> = out.pixels().contiguous_bytes().to_vec();
        let baked = apply_orientation_to_output(out, OrientationHint::Correct);
        assert_eq!((baked.width(), baked.height()), (3, 2));
        assert_eq!(baked.info().orientation, Orientation::Identity);
        assert_eq!(baked.pixels().contiguous_bytes().as_ref(), &before[..]);
    }

    #[test]
    fn report_probe_matches_decode_dims_and_tag_for_every_hint() {
        // For each hint, `report_probe_for_hint` (what probe/output_info report)
        // must agree with the dims + orientation `apply_orientation_to_output`
        // (what decode produces) yields — the probe/decode consistency contract.
        let intrinsic = Orientation::Rotate90; // axis-swapping → exercises swap
        let hints = [
            OrientationHint::Preserve,
            OrientationHint::Correct,
            OrientationHint::ExactTransform(Orientation::Rotate270),
            OrientationHint::CorrectAndTransform(Orientation::FlipH),
        ];
        for hint in hints {
            let probe_info = report_probe_for_hint(
                ImageInfo::new(3, 2, ImageFormat::Custom(&DNG_FORMAT)).with_orientation(intrinsic),
                hint,
            );
            let decoded = apply_orientation_to_output(synth_output(3, 2, intrinsic), hint);
            assert_eq!(
                (probe_info.width, probe_info.height),
                (decoded.width(), decoded.height()),
                "hint {hint:?}: probe vs decode dims differ"
            );
            assert_eq!(
                probe_info.orientation,
                decoded.info().orientation,
                "hint {hint:?}: probe vs decode orientation differ"
            );
        }
    }
}
