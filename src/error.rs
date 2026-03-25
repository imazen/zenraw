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
