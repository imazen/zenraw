//! Error types for RAW/DNG decoding.

use alloc::string::String;

/// Errors from RAW/DNG decode operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RawError {
    /// RAW decoding error from the underlying rawloader crate.
    #[error("RAW decode error: {0}")]
    Decode(String),

    /// Invalid input (dimensions, buffer size, pixel format, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Unsupported RAW feature (camera model, compression, etc.).
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Resource limit exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(String),

    /// Operation stopped by cooperative cancellation.
    #[error("stopped: {0}")]
    Stopped(enough::StopReason),

    /// Pixel buffer error.
    #[error("buffer error: {0}")]
    Buffer(zenpixels::BufferError),
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

/// Helper trait to extract `BufferError` from either `BufferError` or `At<BufferError>`.
///
/// zenpixels 0.1.0 (crates.io) returns bare `BufferError`, while local versions
/// return `At<BufferError>`. This trait lets the same code work with both.
pub(crate) trait IntoBufferError {
    fn into_buffer_error(self) -> zenpixels::BufferError;
}

impl IntoBufferError for zenpixels::BufferError {
    fn into_buffer_error(self) -> zenpixels::BufferError {
        self
    }
}

impl IntoBufferError for whereat::At<zenpixels::BufferError> {
    fn into_buffer_error(self) -> zenpixels::BufferError {
        self.decompose().0
    }
}

#[cfg(feature = "rawloader")]
impl From<rawloader::RawLoaderError> for RawError {
    fn from(e: rawloader::RawLoaderError) -> Self {
        RawError::Decode(e.to_string())
    }
}

/// Result type alias for zenraw operations with location tracking.
pub type Result<T> = core::result::Result<T, whereat::At<RawError>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_variants() {
        let e = RawError::Decode("bad jpeg".into());
        assert!(e.to_string().contains("bad jpeg"));

        let e = RawError::InvalidInput("wrong size".into());
        assert!(e.to_string().contains("wrong size"));

        let e = RawError::Unsupported("no CR3".into());
        assert!(e.to_string().contains("no CR3"));

        let e = RawError::LimitExceeded("too big".into());
        assert!(e.to_string().contains("too big"));

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
    fn into_buffer_error_bare() {
        let be = zenpixels::BufferError::InvalidDimensions;
        let result = be.into_buffer_error();
        assert!(matches!(result, zenpixels::BufferError::InvalidDimensions));
    }

    #[test]
    fn into_buffer_error_at() {
        let at_be = whereat::at!(zenpixels::BufferError::InvalidDimensions);
        let result = at_be.into_buffer_error();
        assert!(matches!(result, zenpixels::BufferError::InvalidDimensions));
    }
}
