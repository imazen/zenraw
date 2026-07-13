//! Error types for RAW/DNG decoding.
//!
//! Each variant maps to exactly one coarse category in zencodec's
//! `ErrorCategory` taxonomy (`Image` / `Request` / `Resource` / `Io` /
//! `Internal` / `Stopped`) via the `CategorizedError` impl below — gated on
//! the optional `zencodec` feature, since that taxonomy's types only exist
//! when the dependency is compiled in. The variant names mirror the category
//! they land in, so the mapping in `category()` reads as confirmation rather
//! than new information.

use alloc::string::String;

/// Errors from RAW/DNG decode operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RawError {
    /// The RAW/DNG bytes are corrupt or structurally invalid — a recognized
    /// format/dialect that failed to parse. Categorized as an image-bytes
    /// fault (malformed content) when the `zencodec` feature is enabled.
    #[error("malformed RAW/DNG data: {0}")]
    Malformed(String),

    /// Input — the file bytes themselves, or an internal derived buffer such
    /// as darktable's PFM interchange output — ended before a complete image
    /// could be read. Categorized as an image-bytes fault (incomplete input)
    /// when the `zencodec` feature is enabled.
    #[error("unexpected end of RAW/DNG data: {0}")]
    UnexpectedEof(String),

    /// The bytes are a recognized-but-unsupported RAW dialect — e.g. a
    /// camera model/mode the active backend doesn't implement. A different
    /// backend (or a future version) might handle it. Categorized as an
    /// image-bytes fault (unsupported type) when the `zencodec` feature is
    /// enabled.
    #[error("unsupported RAW/DNG variant: {0}")]
    UnsupportedType(String),

    /// A structurally-valid RAW image uses a sensor/CFA feature this crate's
    /// demosaic pipeline hasn't implemented (a non-Bayer/non-X-Trans CFA, an
    /// unrecognized pattern shape). Categorized as an image-bytes fault
    /// (unsupported feature) when the `zencodec` feature is enabled.
    #[error("unsupported RAW/DNG feature: {0}")]
    UnsupportedFeature(String),

    /// A well-formed zencodec API operation this codec doesn't support
    /// (row-level/streaming decode, animation decode). Only constructible
    /// when the `zencodec` feature is enabled — categorized as a
    /// caller-request fault, delegating to the wrapped operation's own
    /// category.
    #[cfg(feature = "zencodec")]
    #[error("{0}")]
    UnsupportedOperation(zencodec::UnsupportedOperation),

    /// Caller-supplied parameters are invalid — a bad path, a bad config
    /// value — independent of any image content. Categorized as a
    /// caller-request fault (invalid parameters) when the `zencodec` feature
    /// is enabled.
    #[error("invalid parameters: {0}")]
    InvalidParameters(String),

    /// A caller-supplied data buffer has the wrong size for the operation
    /// (e.g. a raw-linear pixel slice that doesn't match the configured
    /// width × height × channels). Categorized as a caller-request fault
    /// (invalid buffer) when the `zencodec` feature is enabled.
    #[error("invalid buffer: {0}")]
    InvalidBuffer(String),

    /// A configured resource ceiling was exceeded (pixel count, decode
    /// working-set bytes, input size, ...). The [`RawLimitKind`] identifies
    /// which one. Categorized as a resource fault (configured limit) when
    /// the `zencodec` feature is enabled.
    #[error("limit exceeded: {1}")]
    LimitExceeded(RawLimitKind, String),

    /// A memory allocation failed, or a size computation overflowed the
    /// platform's address space (so the allocation could never succeed
    /// regardless of available RAM). Distinct from
    /// [`LimitExceeded`](Self::LimitExceeded): this is genuine exhaustion /
    /// an unallocatable size, not a configured cap. Categorized as a
    /// resource fault (out of memory) when the `zencodec` feature is
    /// enabled.
    #[error("out of memory: {0}")]
    OutOfMemory(String),

    /// An I/O or external-process-execution failure: temp file creation,
    /// darktable-cli subprocess spawn/wait/timeout, reading darktable's PFM
    /// output. Categorized as an I/O fault when the `zencodec` feature is
    /// enabled.
    #[error("I/O error: {0}")]
    Io(String),

    /// An external dependency is missing, or misbehaved in a way not
    /// attributable to the image bytes or the caller's request — no decode
    /// backend compiled in, or `darktable-cli` not found in `PATH`.
    /// Categorized as an internal/unclassified-dependency fault when the
    /// `zencodec` feature is enabled — an honest "unclassified", not a
    /// permanent home; a call site that only ever produces this is a
    /// taxonomy gap worth closing later.
    #[error("dependency unavailable: {0}")]
    Dependency(String),

    /// Operation stopped by cooperative cancellation. Categorized as a
    /// lifecycle fault (cancelled/timed out) when the `zencodec` feature is
    /// enabled.
    #[error("stopped: {0}")]
    Stopped(enough::StopReason),

    /// Pixel buffer construction failed. Every call site in this crate
    /// builds both the byte buffer and the dimensions passed to
    /// [`zenpixels::PixelBuffer::from_vec`] itself (never directly from
    /// caller-supplied geometry), so
    /// [`AllocationFailed`](zenpixels::BufferError::AllocationFailed) is
    /// genuine memory exhaustion — every other variant reflects a
    /// size-math defect internal to this crate rather than the image or the
    /// caller. See the `category()` impl for the split.
    #[error("buffer error: {0}")]
    Buffer(zenpixels::BufferError),
}

/// Which resource ceiling a [`RawError::LimitExceeded`] refers to.
///
/// A crate-local mirror of the subset of `zencodec::LimitKind` that RAW/DNG
/// decode can trip, so this compiles without the optional `zencodec`
/// feature. Maps 1:1 onto `zencodec::LimitKind` in the `CategorizedError`
/// impl below.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawLimitKind {
    /// Image width exceeded a configured or built-in ceiling.
    Width,
    /// Image height exceeded a configured or built-in ceiling.
    Height,
    /// Pixel count (width × height) exceeded a configured or built-in
    /// ceiling.
    Pixels,
    /// A decode working-set / output byte budget was exceeded.
    Memory,
    /// Input data size exceeded a configured ceiling.
    InputSize,
}

impl core::fmt::Display for RawLimitKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Width => f.write_str("width"),
            Self::Height => f.write_str("height"),
            Self::Pixels => f.write_str("pixels"),
            Self::Memory => f.write_str("memory"),
            Self::InputSize => f.write_str("input size"),
        }
    }
}

#[cfg(feature = "zencodec")]
impl RawLimitKind {
    /// Convert to the corresponding `zencodec::LimitKind`. Total: every
    /// variant here has an exact counterpart, since this is a strict subset
    /// mirror (not the reverse direction — see [`from_zencodec`](Self::from_zencodec)).
    pub(crate) fn to_zencodec(self) -> zencodec::LimitKind {
        match self {
            Self::Width => zencodec::LimitKind::Width,
            Self::Height => zencodec::LimitKind::Height,
            Self::Pixels => zencodec::LimitKind::Pixels,
            Self::Memory => zencodec::LimitKind::Memory,
            Self::InputSize => zencodec::LimitKind::InputSize,
        }
    }

    /// Convert from a `zencodec::LimitKind` — used at the
    /// `zencodec::ResourceLimits::check_*` boundary (`zencodec_impl.rs`) to
    /// preserve the checked kind instead of collapsing it to a string.
    /// Lossy in one direction only: `zencodec::LimitKind` is
    /// `#[non_exhaustive]` and carries more variants (`Frames`, `Duration`,
    /// `OutputSize`, `TotalPixels`, `Scans`, `DecompressionRatio`) than RAW
    /// decode's `check_input_size` / `check_dimensions` / `check_memory`
    /// calls ever produce — those fall back to `Memory` (the closest
    /// resource-pressure analogue) rather than failing to compile on a
    /// future library addition.
    pub(crate) fn from_zencodec(k: zencodec::LimitKind) -> Self {
        match k {
            zencodec::LimitKind::Width => Self::Width,
            zencodec::LimitKind::Height => Self::Height,
            zencodec::LimitKind::Pixels => Self::Pixels,
            zencodec::LimitKind::Memory => Self::Memory,
            zencodec::LimitKind::InputSize => Self::InputSize,
            _ => Self::Memory,
        }
    }
}

impl From<enough::StopReason> for RawError {
    fn from(reason: enough::StopReason) -> Self {
        RawError::Stopped(reason)
    }
}

impl From<zenpixels::BufferError> for RawError {
    fn from(e: zenpixels::BufferError) -> Self {
        RawError::Buffer(e)
    }
}

#[cfg(feature = "zencodec")]
impl From<zencodec::UnsupportedOperation> for RawError {
    fn from(op: zencodec::UnsupportedOperation) -> Self {
        RawError::UnsupportedOperation(op)
    }
}

/// `rawloader`'s error is an opaque message (a single struct, no sub-kinds),
/// so every failure collapses to [`Malformed`](RawError::Malformed) — the
/// overwhelmingly common cause (a parser rejecting this specific file's
/// bytes). Explicit truncation (`data.len() < 64`) is checked separately by
/// call sites before invoking rawloader, and reported as
/// [`UnexpectedEof`](RawError::UnexpectedEof).
#[cfg(feature = "rawloader")]
impl From<rawloader::RawLoaderError> for RawError {
    fn from(e: rawloader::RawLoaderError) -> Self {
        RawError::Malformed(e.to_string())
    }
}

/// `rawler`'s error distinguishes an unsupported camera/mode from a generic
/// decode failure — route each to its precise category instead of
/// collapsing both into one bucket.
#[cfg(feature = "rawler")]
impl From<rawler::RawlerError> for RawError {
    fn from(e: rawler::RawlerError) -> Self {
        match &e {
            rawler::RawlerError::Unsupported { .. } => RawError::UnsupportedType(e.to_string()),
            rawler::RawlerError::DecoderFailed(_) => RawError::Malformed(e.to_string()),
        }
    }
}

/// Result type alias for zenraw operations with location tracking.
pub type Result<T> = core::result::Result<T, whereat::At<RawError>>;

/// Bridge a bare [`RawError`] into the shared
/// [`CodecError`](zencodec::CodecError) envelope (Pattern B).
///
/// `zenraw`'s own zencodec trait impls (`RawDecoderConfig` / `RawDecodeJob` /
/// `RawDecoder`) keep `type Error = At<RawError>` — the concrete native
/// error, unchanged by this migration — so this bridge is for a *consumer*
/// that wants the shared, codec-agnostic envelope instead: `.start_at()`
/// begins the location trace; [`CodecError::of`] then reads the
/// [`category`](zencodec::CategorizedError::category) *and* the
/// [`codec_name`](zencodec::CategorizedError::codec_name) from the value,
/// keeping the trace on the outside.
///
/// Already-located `At<RawError>` values convert via `.map_err(CodecError::of)`
/// instead — the orphan rule forbids a `From<At<RawError>>` impl here (`At`
/// is not a fundamental type, so `At<RawError>` is not a local type).
#[cfg(feature = "zencodec")]
impl From<RawError> for whereat::At<zencodec::CodecError> {
    #[track_caller]
    fn from(e: RawError) -> Self {
        use whereat::ErrorAtExt;
        zencodec::CodecError::of(e.start_at())
    }
}

// Codec-agnostic error taxonomy (zencodec PR #116, the two-level
// origin-first reshape). Maps every `RawError` variant to exactly one
// `ErrorCategory` so consumers can route on the category — HTTP status,
// retry policy, logging — without matching this enum directly. `zencodec`
// is optional in this crate, so the impl is gated on the feature.
#[cfg(feature = "zencodec")]
impl zencodec::CategorizedError for RawError {
    fn codec_name(&self) -> Option<&'static str> {
        Some("zenraw")
    }

    fn category(&self) -> zencodec::ErrorCategory {
        use zencodec::{
            ImageError, InternalKind, RequestError, ResourceError, UnsupportedImageKind,
        };

        match self {
            // Image-bytes-origin faults.
            RawError::Malformed(_) => ImageError::Malformed.into(),
            RawError::UnexpectedEof(_) => ImageError::UnexpectedEof.into(),
            RawError::UnsupportedType(_) => {
                ImageError::Unsupported(UnsupportedImageKind::Type).into()
            }
            RawError::UnsupportedFeature(_) => {
                ImageError::Unsupported(UnsupportedImageKind::Feature).into()
            }

            // Caller-request-origin faults.
            RawError::UnsupportedOperation(op) => op.category(),
            RawError::InvalidParameters(_) => {
                RequestError::Invalid(zencodec::InvalidKind::Parameters).into()
            }
            RawError::InvalidBuffer(_) => {
                RequestError::Invalid(zencodec::InvalidKind::Buffer).into()
            }

            // Resource-origin faults.
            RawError::LimitExceeded(kind, _) => ResourceError::Limits(kind.to_zencodec()).into(),
            RawError::OutOfMemory(_) => ResourceError::OutOfMemory.into(),

            // I/O.
            RawError::Io(_) => zencodec::ErrorCategory::Io(zencodec::CodecIoKind::opaque()),

            // Unclassified external-dependency fault.
            RawError::Dependency(_) => InternalKind::Dependency.into(),

            // Stopped — delegate to the wrapped StopReason's own category.
            RawError::Stopped(reason) => reason.category(),

            // Buffer construction: every call site computes both the byte
            // buffer and the dimensions itself, so `AllocationFailed` is
            // real OOM; every other shape is an internal size-math defect.
            RawError::Buffer(zenpixels::BufferError::AllocationFailed) => {
                ResourceError::OutOfMemory.into()
            }
            RawError::Buffer(_) => InternalKind::Bug.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_variants() {
        let e = RawError::Malformed("bad jpeg".into());
        assert!(e.to_string().contains("bad jpeg"));

        let e = RawError::InvalidParameters("wrong size".into());
        assert!(e.to_string().contains("wrong size"));

        let e = RawError::UnsupportedFeature("no CR3".into());
        assert!(e.to_string().contains("no CR3"));

        let e = RawError::LimitExceeded(RawLimitKind::Pixels, "too big".into());
        assert!(e.to_string().contains("too big"));

        let e = RawError::OutOfMemory("overflow".into());
        assert!(e.to_string().contains("overflow"));

        let e = RawError::Io("temp dir".into());
        assert!(e.to_string().contains("temp dir"));

        let e = RawError::Dependency("darktable-cli missing".into());
        assert!(e.to_string().contains("darktable-cli missing"));

        let e = RawError::Buffer(zenpixels::BufferError::InvalidDimensions);
        assert!(e.to_string().contains("buffer error"));
    }

    #[test]
    fn from_stop_reason() {
        let reason = enough::StopReason::Cancelled;
        let e: RawError = reason.into();
        assert!(matches!(e, RawError::Stopped(_)));
    }

    #[test]
    fn from_buffer_error() {
        let be = zenpixels::BufferError::InvalidDimensions;
        let e: RawError = be.into();
        assert!(matches!(e, RawError::Buffer(_)));
    }

    #[test]
    fn raw_limit_kind_display() {
        assert_eq!(RawLimitKind::Width.to_string(), "width");
        assert_eq!(RawLimitKind::Height.to_string(), "height");
        assert_eq!(RawLimitKind::Pixels.to_string(), "pixels");
        assert_eq!(RawLimitKind::Memory.to_string(), "memory");
        assert_eq!(RawLimitKind::InputSize.to_string(), "input size");
    }

    #[cfg(feature = "zencodec")]
    mod categorized {
        use super::*;
        use zencodec::{
            CategorizedError, ErrorCategory as C, ImageError as Img, InternalKind as Int,
            InvalidKind, LimitKind as L, RequestError as Req, ResourceError as Res,
            UnsupportedImageKind as UImg,
        };

        #[test]
        fn codec_name_is_zenraw() {
            assert_eq!(RawError::Malformed("x".into()).codec_name(), Some("zenraw"));
        }

        #[test]
        fn image_origin_categories() {
            assert_eq!(
                RawError::Malformed("x".into()).category(),
                C::Image(Img::Malformed)
            );
            assert_eq!(
                RawError::UnexpectedEof("x".into()).category(),
                C::Image(Img::UnexpectedEof)
            );
            assert_eq!(
                RawError::UnsupportedType("x".into()).category(),
                C::Image(Img::Unsupported(UImg::Type))
            );
            assert_eq!(
                RawError::UnsupportedFeature("x".into()).category(),
                C::Image(Img::Unsupported(UImg::Feature))
            );
        }

        #[test]
        fn request_origin_categories() {
            assert_eq!(
                RawError::InvalidParameters("x".into()).category(),
                C::Request(Req::Invalid(InvalidKind::Parameters))
            );
            assert_eq!(
                RawError::InvalidBuffer("x".into()).category(),
                C::Request(Req::Invalid(InvalidKind::Buffer))
            );
            assert_eq!(
                RawError::UnsupportedOperation(zencodec::UnsupportedOperation::RowLevelDecode)
                    .category(),
                C::Request(Req::Unsupported(
                    zencodec::UnsupportedOperation::RowLevelDecode
                ))
            );
            assert_eq!(
                RawError::UnsupportedOperation(zencodec::UnsupportedOperation::AnimationDecode)
                    .category(),
                C::Request(Req::Unsupported(
                    zencodec::UnsupportedOperation::AnimationDecode
                ))
            );
        }

        #[test]
        fn resource_origin_categories() {
            assert_eq!(
                RawError::LimitExceeded(RawLimitKind::Pixels, "x".into()).category(),
                C::Resource(Res::Limits(L::Pixels))
            );
            assert_eq!(
                RawError::LimitExceeded(RawLimitKind::Width, "x".into()).category(),
                C::Resource(Res::Limits(L::Width))
            );
            assert_eq!(
                RawError::LimitExceeded(RawLimitKind::Height, "x".into()).category(),
                C::Resource(Res::Limits(L::Height))
            );
            assert_eq!(
                RawError::LimitExceeded(RawLimitKind::Memory, "x".into()).category(),
                C::Resource(Res::Limits(L::Memory))
            );
            assert_eq!(
                RawError::LimitExceeded(RawLimitKind::InputSize, "x".into()).category(),
                C::Resource(Res::Limits(L::InputSize))
            );
            assert_eq!(
                RawError::OutOfMemory("x".into()).category(),
                C::Resource(Res::OutOfMemory)
            );
        }

        #[test]
        fn io_category_is_opaque() {
            assert_eq!(
                RawError::Io("x".into()).category(),
                C::Io(zencodec::CodecIoKind::opaque())
            );
        }

        #[test]
        fn dependency_is_internal() {
            assert_eq!(
                RawError::Dependency("x".into()).category(),
                C::Internal(Int::Dependency)
            );
        }

        #[test]
        fn stopped_delegates_to_stop_reason() {
            assert_eq!(
                RawError::Stopped(enough::StopReason::Cancelled).category(),
                C::Stopped(enough::StopReason::Cancelled)
            );
            assert_eq!(
                RawError::Stopped(enough::StopReason::TimedOut).category(),
                C::Stopped(enough::StopReason::TimedOut)
            );
        }

        #[test]
        fn buffer_allocation_failed_is_oom_others_are_internal_bug() {
            assert_eq!(
                RawError::Buffer(zenpixels::BufferError::AllocationFailed).category(),
                C::Resource(Res::OutOfMemory)
            );
            assert_eq!(
                RawError::Buffer(zenpixels::BufferError::InvalidDimensions).category(),
                C::Internal(Int::Bug)
            );
            assert_eq!(
                RawError::Buffer(zenpixels::BufferError::StrideTooSmall).category(),
                C::Internal(Int::Bug)
            );
        }

        #[test]
        fn category_is_preserved_through_at() {
            let located = whereat::At::wrap(RawError::UnexpectedEof("eof".into()));
            assert_eq!(located.category(), C::Image(Img::UnexpectedEof));
            assert_eq!(located.codec_name(), Some("zenraw"));
        }

        #[test]
        fn from_zencodec_unsupported_operation() {
            let e: RawError = zencodec::UnsupportedOperation::AnimationDecode.into();
            assert!(matches!(e, RawError::UnsupportedOperation(_)));
        }
    }
}
