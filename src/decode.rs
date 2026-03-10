//! RAW/DNG decoding to zenpixels buffers.
//!
//! Takes camera RAW file bytes, demosaics the Bayer sensor data, applies
//! white balance and color matrix correction, and produces pixel buffers.
//!
//! By default, output is **scene-referred linear f32** (`RGBF32_LINEAR`).
//! Set `apply_gamma(true)` for display-referred sRGB u8 output.

extern crate std;

use alloc::vec::Vec;

use enough::Stop;
#[cfg(feature = "rawloader")]
use whereat::at;
#[cfg(feature = "rawloader")]
use zenpixels::{PixelBuffer, PixelDescriptor};

#[cfg(feature = "rawloader")]
use crate::color;
#[cfg(feature = "rawloader")]
use crate::demosaic::{DemosaicMethod, demosaic_to_rgb_f32};
use crate::error::{RawError, Result};

/// Configuration for RAW/DNG decoding.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RawDecodeConfig {
    /// Demosaicing algorithm to use.
    pub demosaic: DemosaicMethod,
    /// Maximum pixel count (width × height) before rejecting.
    pub max_pixels: u64,
    /// Whether to apply sRGB gamma curve (true → display-referred sRGB u8,
    /// false → scene-referred linear f32).
    ///
    /// Default: `false` (scene-referred linear output).
    pub apply_gamma: bool,
    /// Whether to apply the crop specified in the RAW metadata.
    pub apply_crop: bool,
}

impl Default for RawDecodeConfig {
    fn default() -> Self {
        Self {
            demosaic: DemosaicMethod::default(),
            max_pixels: 200_000_000, // 200 megapixels
            apply_gamma: false,
            apply_crop: true,
        }
    }
}

impl RawDecodeConfig {
    /// Create a config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the demosaicing method.
    #[must_use]
    pub fn with_demosaic(mut self, method: DemosaicMethod) -> Self {
        self.demosaic = method;
        self
    }

    /// Set maximum allowed pixel count.
    #[must_use]
    pub fn with_max_pixels(mut self, max: u64) -> Self {
        self.max_pixels = max;
        self
    }

    /// Set whether to apply sRGB gamma (default: false).
    ///
    /// When true, output is display-referred sRGB RGB8.
    /// When false (default), output is scene-referred linear f32.
    #[must_use]
    pub fn with_gamma(mut self, apply: bool) -> Self {
        self.apply_gamma = apply;
        self
    }

    /// Set whether to apply the crop from RAW metadata (default: true).
    #[must_use]
    pub fn with_crop(mut self, apply: bool) -> Self {
        self.apply_crop = apply;
        self
    }
}

/// Output from RAW/DNG decoding.
#[derive(Debug)]
#[non_exhaustive]
pub struct RawDecodeOutput {
    /// Decoded pixel buffer (RGB8 sRGB or RGBF32 linear, depending on config).
    pub pixels: PixelBuffer,
    /// Decoded image metadata.
    pub info: RawInfo,
}

/// Metadata extracted from RAW/DNG files.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RawInfo {
    /// Image width after crop and processing.
    pub width: u32,
    /// Image height after crop and processing.
    pub height: u32,
    /// Camera make.
    pub make: alloc::string::String,
    /// Camera model.
    pub model: alloc::string::String,
    /// Original sensor width before crop.
    pub sensor_width: u32,
    /// Original sensor height before crop.
    pub sensor_height: u32,
    /// CFA pattern description (e.g., "RGGB").
    pub cfa_pattern: alloc::string::String,
    /// Whether the source was a DNG file.
    pub is_dng: bool,
    /// EXIF orientation (rawloader encoding).
    pub orientation: u16,
}

/// Probe a RAW/DNG file for metadata without decoding pixels.
///
/// Returns metadata about the image (dimensions, camera info, etc.).
#[cfg(feature = "rawloader")]
pub fn probe(data: &[u8], stop: &dyn Stop) -> Result<RawInfo> {
    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let raw =
        rawloader::decode(&mut std::io::Cursor::new(data)).map_err(|e| at!(RawError::from(e)))?;

    let is_dng = is_dng_data(data);

    Ok(RawInfo {
        width: raw.width as u32,
        height: raw.height as u32,
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
        sensor_width: raw.width as u32,
        sensor_height: raw.height as u32,
        cfa_pattern: raw.cfa.to_string(),
        is_dng,
        orientation: orientation_to_u16(&raw.orientation),
    })
}

/// Decode a RAW/DNG file to a pixel buffer.
///
/// The full pipeline:
/// 1. Parse RAW file with rawloader
/// 2. Normalize sensor data to [0, 1] using black/white levels
/// 3. Demosaic Bayer CFA data to RGB
/// 4. Apply white balance + camera→sRGB color matrix
/// 5. Optionally apply sRGB gamma curve
/// 6. Optionally crop to the camera's recommended region
/// 7. Return as a PixelBuffer
#[cfg(feature = "rawloader")]
pub fn decode(data: &[u8], config: &RawDecodeConfig, stop: &dyn Stop) -> Result<RawDecodeOutput> {
    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 1: Parse
    let raw =
        rawloader::decode(&mut std::io::Cursor::new(data)).map_err(|e| at!(RawError::from(e)))?;

    let width = raw.width;
    let height = raw.height;

    // Check limits
    let pixels = width as u64 * height as u64;
    if pixels > config.max_pixels {
        return Err(at!(RawError::LimitExceeded(alloc::format!(
            "image {width}x{height} = {pixels} pixels exceeds limit of {}",
            config.max_pixels
        ))));
    }

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 2: Extract and normalize sensor data to f32 [0, 1]
    let normalized = normalize_raw_data(&raw).map_err(|e| at!(e))?;

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // For non-Bayer data (cpp > 1), skip demosaicing
    if raw.cpp > 1 {
        return decode_non_bayer(raw, normalized, config, stop);
    }

    // Step 3: Demosaic
    let mut rgb = demosaic_to_rgb_f32(&normalized, width, height, &raw.cfa, config.demosaic);

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 4: Color pipeline (WB + camera→sRGB)
    color::apply_color_pipeline(&mut rgb, raw.wb_coeffs, raw.xyz_to_cam);

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 5: Crop
    let (cropped_rgb, out_w, out_h) = if config.apply_crop {
        apply_crop(&rgb, width, height, &raw.crops)
    } else {
        (rgb, width, height)
    };

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let is_dng = is_dng_data(data);

    // Step 6: Convert to output format
    if config.apply_gamma {
        // Apply sRGB gamma and convert to u8
        let mut gamma_rgb = cropped_rgb;
        color::apply_srgb_gamma(&mut gamma_rgb);
        let u8_data = color::f32_to_u8_srgb(&gamma_rgb);

        let descriptor = PixelDescriptor::RGB8_SRGB;
        let buf = PixelBuffer::from_vec(u8_data, out_w as u32, out_h as u32, descriptor)
            .map_err(|e| at!(RawError::Buffer(e.into_inner())))?;

        Ok(RawDecodeOutput {
            pixels: buf,
            info: RawInfo {
                width: out_w as u32,
                height: out_h as u32,
                make: raw.clean_make.clone(),
                model: raw.clean_model.clone(),
                sensor_width: raw.width as u32,
                sensor_height: raw.height as u32,
                cfa_pattern: raw.cfa.to_string(),
                is_dng,
                orientation: orientation_to_u16(&raw.orientation),
            },
        })
    } else {
        // Linear f32 output
        let byte_data: Vec<u8> = cropped_rgb.iter().flat_map(|&v| v.to_ne_bytes()).collect();

        let descriptor = PixelDescriptor::RGBF32_LINEAR;
        let buf = PixelBuffer::from_vec(byte_data, out_w as u32, out_h as u32, descriptor)
            .map_err(|e| at!(RawError::Buffer(e.into_inner())))?;

        Ok(RawDecodeOutput {
            pixels: buf,
            info: RawInfo {
                width: out_w as u32,
                height: out_h as u32,
                make: raw.clean_make.clone(),
                model: raw.clean_model.clone(),
                sensor_width: raw.width as u32,
                sensor_height: raw.height as u32,
                cfa_pattern: raw.cfa.to_string(),
                is_dng,
                orientation: orientation_to_u16(&raw.orientation),
            },
        })
    }
}

/// Handle non-Bayer data (cpp > 1, e.g., Foveon or some DNGs with embedded RGB).
#[cfg(feature = "rawloader")]
fn decode_non_bayer(
    raw: rawloader::RawImage,
    normalized: Vec<f32>,
    config: &RawDecodeConfig,
    stop: &dyn Stop,
) -> Result<RawDecodeOutput> {
    let width = raw.width;
    let height = raw.height;
    let cpp = raw.cpp;

    // Convert to 3-channel RGB, dropping extra channels
    let mut rgb = Vec::with_capacity(width * height * 3);
    for i in 0..width * height {
        let base = i * cpp;
        rgb.push(if base < normalized.len() {
            normalized[base]
        } else {
            0.0
        });
        rgb.push(if base + 1 < normalized.len() {
            normalized[base + 1]
        } else {
            0.0
        });
        rgb.push(if base + 2 < normalized.len() {
            normalized[base + 2]
        } else {
            0.0
        });
    }

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Apply color pipeline
    color::apply_color_pipeline(&mut rgb, raw.wb_coeffs, raw.xyz_to_cam);

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let (cropped_rgb, out_w, out_h) = if config.apply_crop {
        apply_crop(&rgb, width, height, &raw.crops)
    } else {
        (rgb, width, height)
    };

    let is_dng = false; // Can't easily check without original data here

    if config.apply_gamma {
        let mut gamma_rgb = cropped_rgb;
        color::apply_srgb_gamma(&mut gamma_rgb);
        let u8_data = color::f32_to_u8_srgb(&gamma_rgb);

        let buf = PixelBuffer::from_vec(
            u8_data,
            out_w as u32,
            out_h as u32,
            PixelDescriptor::RGB8_SRGB,
        )
        .map_err(|e| at!(RawError::Buffer(e.into_inner())))?;

        Ok(RawDecodeOutput {
            pixels: buf,
            info: RawInfo {
                width: out_w as u32,
                height: out_h as u32,
                make: raw.clean_make,
                model: raw.clean_model,
                sensor_width: raw.width as u32,
                sensor_height: raw.height as u32,
                cfa_pattern: raw.cfa.to_string(),
                is_dng,
                orientation: orientation_to_u16(&raw.orientation),
            },
        })
    } else {
        let byte_data: Vec<u8> = cropped_rgb.iter().flat_map(|&v| v.to_ne_bytes()).collect();

        let buf = PixelBuffer::from_vec(
            byte_data,
            out_w as u32,
            out_h as u32,
            PixelDescriptor::RGBF32_LINEAR,
        )
        .map_err(|e| at!(RawError::Buffer(e.into_inner())))?;

        Ok(RawDecodeOutput {
            pixels: buf,
            info: RawInfo {
                width: out_w as u32,
                height: out_h as u32,
                make: raw.clean_make,
                model: raw.clean_model,
                sensor_width: raw.width as u32,
                sensor_height: raw.height as u32,
                cfa_pattern: raw.cfa.to_string(),
                is_dng,
                orientation: orientation_to_u16(&raw.orientation),
            },
        })
    }
}

/// Normalize raw sensor data to f32 [0, 1] using black/white levels.
#[cfg(feature = "rawloader")]
fn normalize_raw_data(raw: &rawloader::RawImage) -> core::result::Result<Vec<f32>, RawError> {
    let width = raw.width;
    let height = raw.height;
    let cpp = raw.cpp;
    let total = width * height * cpp;

    let black = raw.blacklevels;
    let white = raw.whitelevels;

    match &raw.data {
        rawloader::RawImageData::Integer(data) => {
            if data.len() < total {
                return Err(RawError::InvalidInput(alloc::format!(
                    "expected {} pixels, got {}",
                    total,
                    data.len()
                )));
            }

            let mut out = Vec::with_capacity(total);
            for (i, &sample) in data.iter().enumerate().take(total) {
                let ch = if cpp == 1 {
                    raw.cfa.color_at(i / width, i % width)
                } else {
                    i % cpp
                };
                let bl = black[ch.min(3)] as f32;
                let wl = white[ch.min(3)] as f32;
                let range = (wl - bl).max(1.0);
                let val = (sample as f32 - bl) / range;
                out.push(val.clamp(0.0, 1.0));
            }
            Ok(out)
        }
        rawloader::RawImageData::Float(data) => {
            if data.len() < total {
                return Err(RawError::InvalidInput(alloc::format!(
                    "expected {} pixels, got {}",
                    total,
                    data.len()
                )));
            }

            let mut out = Vec::with_capacity(total);
            for (i, &sample) in data.iter().enumerate().take(total) {
                let ch = if cpp == 1 {
                    raw.cfa.color_at(i / width, i % width)
                } else {
                    i % cpp
                };
                let bl = black[ch.min(3)] as f32;
                let wl = white[ch.min(3)] as f32;
                let range = (wl - bl).max(1.0);
                let val = (sample - bl) / range;
                out.push(val.clamp(0.0, 1.0));
            }
            Ok(out)
        }
    }
}

/// Apply crop from RAW metadata.
///
/// crops is [top, right, bottom, left] in rawloader convention.
#[cfg(feature = "rawloader")]
fn apply_crop(
    rgb: &[f32],
    width: usize,
    height: usize,
    crops: &[usize; 4],
) -> (Vec<f32>, usize, usize) {
    let top = crops[0];
    let right = crops[1];
    let bottom = crops[2];
    let left = crops[3];

    // Validate crop dimensions
    if top + bottom >= height || left + right >= width {
        // Invalid crop — return uncropped
        return (rgb.to_vec(), width, height);
    }

    let new_w = width - left - right;
    let new_h = height - top - bottom;

    let mut cropped = Vec::with_capacity(new_w * new_h * 3);
    for row in top..height - bottom {
        let src_start = (row * width + left) * 3;
        let src_end = src_start + new_w * 3;
        cropped.extend_from_slice(&rgb[src_start..src_end]);
    }

    (cropped, new_w, new_h)
}

/// Check if data appears to be a DNG file (TIFF with DNG version tag).
pub(crate) fn is_dng_data(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }
    // TIFF header check
    let is_tiff = (data[0] == b'I' && data[1] == b'I' && data[2] == 42 && data[3] == 0)
        || (data[0] == b'M' && data[1] == b'M' && data[2] == 0 && data[3] == 42);
    if !is_tiff {
        return false;
    }
    // Look for DNGVersion tag (0xC612) in the first 4KB
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

/// Convert rawloader Orientation to EXIF u16 value.
#[cfg(feature = "rawloader")]
fn orientation_to_u16(orient: &rawloader::Orientation) -> u16 {
    match orient {
        rawloader::Orientation::Normal => 1,
        rawloader::Orientation::HorizontalFlip => 2,
        rawloader::Orientation::Rotate180 => 3,
        rawloader::Orientation::VerticalFlip => 4,
        rawloader::Orientation::Transpose => 5,
        rawloader::Orientation::Rotate90 => 6,
        rawloader::Orientation::Transverse => 7,
        rawloader::Orientation::Rotate270 => 8,
        _ => 1,
    }
}

/// Detect whether a byte slice looks like a supported RAW/DNG file.
///
/// Checks for TIFF-based RAW formats and known camera RAW magic bytes.
pub fn is_raw_file(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }

    // TIFF-based formats (DNG, CR2, NEF, ARW, etc.)
    let is_tiff = (data[0] == b'I' && data[1] == b'I' && data[2] == 42 && data[3] == 0)
        || (data[0] == b'M' && data[1] == b'M' && data[2] == 0 && data[3] == 42);

    if is_tiff {
        return true;
    }

    // Olympus ORF (uses TIFF-like but with different magic in some variants)
    if data[0] == b'I' && data[1] == b'I' && data[2] == 0x52 && data[3] == 0x4F {
        return true;
    }

    // Fuji RAF
    if data.len() >= 8 && &data[..8] == b"FUJIFILM" {
        return true;
    }

    // Panasonic RW2 (TIFF variant with 0x55 marker)
    if data[0] == b'I' && data[1] == b'I' && data[2] == 0x55 && data[3] == 0x00 {
        return true;
    }

    false
}
