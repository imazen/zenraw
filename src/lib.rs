//! Camera RAW and DNG decoder with zenpixels integration.
//!
//! Wraps the [`rawloader`] crate with demosaicing, white balance, and color
//! matrix correction to produce sRGB pixel buffers compatible with the zen*
//! codec ecosystem.
//!
//! # Supported formats
//!
//! All formats supported by `rawloader`:
//! - **DNG** (Adobe Digital Negative)
//! - **CR2** (Canon)
//! - **NEF/NRW** (Nikon)
//! - **ARW/SRF/SR2** (Sony)
//! - **RAF** (Fujifilm)
//! - **RW2** (Panasonic/Leica)
//! - **PEF** (Pentax)
//! - **ORF** (Olympus)
//! - **ERF** (Epson)
//! - **3FR** (Hasselblad)
//! - **IIQ** (Phase One)
//! - And many more (~30 formats)
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
//! 1. Parse camera RAW file (rawloader)
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

#[cfg(feature = "zencodec")]
mod zencodec_impl;
#[cfg(feature = "zencodec")]
pub use zencodec_impl::{DNG_FORMAT, RAW_FORMAT, RawDecoderConfig};

pub use decode::{RawDecodeConfig, RawDecodeOutput, RawInfo, decode, is_raw_file, probe};
pub use demosaic::DemosaicMethod;
pub use error::RawError;

/// Result type alias for zenraw operations.
pub type Result<T> = core::result::Result<T, whereat::At<RawError>>;
