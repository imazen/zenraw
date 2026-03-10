//! Camera RAW and DNG decoder with zenpixels integration.
//!
//! Multiple decode backends available:
//! - **`rawloader`** (default) — lightweight, Bayer-only, ~200 cameras
//! - **`rawler`** — broader support: CR3, X-Trans, JXL-compressed DNG, ~300+ cameras
//! - **`darktable`** — shells out to darktable-cli for 900+ cameras with full pipeline
//!
//! Output is **scene-referred linear f32** (`RGBF32_LINEAR`) by default.
//! Enable `apply_gamma` for display-referred sRGB u8 output.
//!
//! # Supported formats
//!
//! - **DNG** (Adobe Digital Negative, including iPhone ProRAW with `rawler`)
//! - **CR2** (Canon)
//! - **CR3** (Canon, with `rawler`)
//! - **NEF/NRW** (Nikon)
//! - **ARW/SRF/SR2** (Sony)
//! - **RAF** (Fujifilm)
//! - **RW2** (Panasonic/Leica)
//! - **PEF** (Pentax)
//! - **ORF** (Olympus)
//! - **ERF** (Epson)
//! - **3FR** (Hasselblad)
//! - **IIQ** (Phase One)
//! - And many more
//!
//! # Quick start
//!
//! ```no_run
//! use zenraw::{decode, RawDecodeConfig};
//! use enough::Unstoppable;
//!
//! let data: &[u8] = &[]; // your RAW file bytes
//! let output = decode(data, &RawDecodeConfig::default(), &Unstoppable)?;
//! println!("{}x{} {}", output.info.width, output.info.height, output.info.model);
//! # Ok::<(), whereat::At<zenraw::RawError>>(())
//! ```
//!
//! # Processing pipeline
//!
//! 1. Parse camera RAW file
//! 2. Normalize sensor values using black/white levels
//! 3. Demosaic Bayer CFA pattern → RGB (Malvar-He-Cutler by default)
//! 4. Apply white balance coefficients
//! 5. Apply camera→XYZ→sRGB color matrix
//! 6. Optionally apply sRGB gamma curve
//! 7. Optionally apply crop from camera metadata

#![forbid(unsafe_code)]

extern crate alloc;

// Crate info for whereat error tracing
whereat::define_at_crate_info!();

pub mod color;
pub mod decode;
pub mod demosaic;
mod error;

#[cfg(feature = "rawler")]
mod rawler_backend;

#[cfg(feature = "darktable")]
pub mod darktable;

#[cfg(feature = "exif")]
pub mod exif;

#[cfg(feature = "zencodec")]
mod zencodec_impl;
#[cfg(feature = "zencodec")]
pub use zencodec_impl::{DNG_FORMAT, RAW_FORMAT, RawDecoderConfig};

pub use decode::{RawDecodeConfig, RawDecodeOutput, RawInfo};
pub use demosaic::DemosaicMethod;
pub use error::RawError;

// ── Public decode/probe dispatch ──────────────────────────────────────

/// Decode a RAW/DNG file to a pixel buffer.
///
/// Uses the best available backend: rawler > rawloader.
/// Both produce scene-referred linear f32 by default.
#[allow(clippy::needless_return)]
pub fn decode(
    data: &[u8],
    config: &decode::RawDecodeConfig,
    stop: &dyn enough::Stop,
) -> crate::error::Result<decode::RawDecodeOutput> {
    #[cfg(feature = "rawler")]
    {
        return rawler_backend::decode(data, config, stop);
    }
    #[cfg(all(feature = "rawloader", not(feature = "rawler")))]
    {
        return decode::decode(data, config, stop);
    }
    #[cfg(not(any(feature = "rawloader", feature = "rawler")))]
    {
        let _ = (data, config, stop);
        Err(whereat::at!(RawError::Unsupported(
            "no decode backend enabled — enable `rawloader` or `rawler` feature".into()
        )))
    }
}

/// Probe a RAW/DNG file for metadata without decoding pixels.
#[allow(clippy::needless_return)]
pub fn probe(data: &[u8], stop: &dyn enough::Stop) -> crate::error::Result<decode::RawInfo> {
    #[cfg(feature = "rawler")]
    {
        return rawler_backend::probe(data, stop);
    }
    #[cfg(all(feature = "rawloader", not(feature = "rawler")))]
    {
        return decode::probe(data, stop);
    }
    #[cfg(not(any(feature = "rawloader", feature = "rawler")))]
    {
        let _ = (data, stop);
        Err(whereat::at!(RawError::Unsupported(
            "no decode backend enabled — enable `rawloader` or `rawler` feature".into()
        )))
    }
}

/// Detect whether a byte slice looks like a supported RAW/DNG file.
pub fn is_raw_file(data: &[u8]) -> bool {
    decode::is_raw_file(data)
}

/// Result type alias for zenraw operations.
pub type Result<T> = core::result::Result<T, whereat::At<RawError>>;
