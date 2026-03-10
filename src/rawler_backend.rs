//! rawler-based RAW/DNG backend.
//!
//! Uses the [`rawler`] crate (dnglab project) for broader camera support
//! including CR3, X-Trans, and JPEG XL-compressed DNG.

extern crate std;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use enough::Stop;
use rawler::decoders::RawDecodeParams;
use rawler::rawimage::RawPhotometricInterpretation;
use rawler::rawsource::RawSource;
use whereat::at;
use zenpixels::{PixelBuffer, PixelDescriptor};

use crate::color;
use crate::decode::{RawDecodeConfig, RawDecodeOutput, RawInfo};
use crate::demosaic::{CfaPattern, demosaic_to_rgb_f32, demosaic_xtrans_bilinear};
use crate::error::{IntoBufferError, RawError, Result};

/// Probe a RAW/DNG file for metadata without decoding pixels.
pub fn probe(data: &[u8], stop: &dyn Stop) -> Result<RawInfo> {
    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let source = RawSource::new_from_slice(data);
    let params = RawDecodeParams::default();
    let raw =
        rawler::decode(&source, &params).map_err(|e| at!(RawError::Decode(format!("{e}"))))?;

    let cfa_pattern = extract_cfa_pattern(&raw);
    let is_dng = crate::decode::is_dng_data(data);

    // Use crop area if available for dimensions
    let (width, height) = if let Some(ref crop) = raw.crop_area {
        (crop.d.w as u32, crop.d.h as u32)
    } else if let Some(ref active) = raw.active_area {
        (active.d.w as u32, active.d.h as u32)
    } else {
        (raw.width as u32, raw.height as u32)
    };

    Ok(RawInfo {
        width,
        height,
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
        sensor_width: raw.width as u32,
        sensor_height: raw.height as u32,
        cfa_pattern,
        is_dng,
        orientation: orientation_to_u16(&raw.orientation),
        bit_depth: Some(crate::decode::bits_from_whitelevel(
            raw.whitelevel.as_bayer_array()[0] as u32,
        )),
    })
}

/// Decode a RAW/DNG file to a pixel buffer using rawler.
pub fn decode(data: &[u8], config: &RawDecodeConfig, stop: &dyn Stop) -> Result<RawDecodeOutput> {
    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 1: Parse
    let source = RawSource::new_from_slice(data);
    let params = RawDecodeParams::default();
    let raw =
        rawler::decode(&source, &params).map_err(|e| at!(RawError::Decode(format!("{e}"))))?;

    let width = raw.width;
    let height = raw.height;

    // Check limits
    let pixels = width as u64 * height as u64;
    if pixels > config.max_pixels {
        return Err(at!(RawError::LimitExceeded(format!(
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
        return decode_non_bayer(raw, normalized, config, stop, data);
    }

    // Step 3: Demosaic — extract CFA
    let cfa = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(cfg) => &cfg.cfa,
        _ => {
            return Err(at!(RawError::Unsupported(
                "no CFA pattern for demosaicing".into()
            )));
        }
    };

    // Convert CFA and demosaic
    let cfa_str = cfa.to_string();

    let mut rgb = if cfa_str.len() > 4 {
        // X-Trans (6x6) or other non-Bayer CFA — use generic demosaic
        let pattern_size = (cfa_str.len() as f64).sqrt() as usize;
        if pattern_size * pattern_size != cfa_str.len() {
            return Err(at!(RawError::Unsupported(format!(
                "non-square CFA pattern not supported: len={}",
                cfa_str.len()
            ))));
        }
        let mut colors = Vec::with_capacity(cfa_str.len());
        for r in 0..pattern_size {
            for c in 0..pattern_size {
                colors.push(cfa.color_at(r, c) as u8);
            }
        }
        let cfa_pattern = CfaPattern::new(colors, pattern_size, pattern_size);
        demosaic_xtrans_bilinear(&normalized, width, height, &cfa_pattern)
    } else {
        // Standard 2x2 Bayer
        let rl_cfa = rawloader::CFA::new(&cfa_str);
        demosaic_to_rgb_f32(&normalized, width, height, &rl_cfa, config.demosaic)
    };

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 4: Color pipeline (WB + camera→sRGB)
    color::apply_color_pipeline(&mut rgb, raw.wb_coeffs, raw.xyz_to_cam);

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 5: Crop using rawler's crop_area or active_area
    let (cropped_rgb, out_w, out_h) = if config.apply_crop {
        apply_rawler_crop(&rgb, width, height, &raw)
    } else {
        (rgb, width, height)
    };

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let is_dng = crate::decode::is_dng_data(data);

    // Step 6: Apply EXIF orientation
    let raw_orient = orientation_to_u16(&raw.orientation);
    let (final_rgb, final_w, final_h, final_orient) = if config.apply_orientation && raw_orient > 1
    {
        let (data, w, h) = crate::orient::apply_orientation(cropped_rgb, out_w, out_h, raw_orient);
        (data, w, h, 1u16)
    } else {
        (cropped_rgb, out_w, out_h, raw_orient)
    };

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 7: Convert to output format
    build_output(
        final_rgb,
        final_w,
        final_h,
        config,
        &raw,
        is_dng,
        final_orient,
    )
}

// ── Internal helpers ──────────────────────────────────────────────────

/// Extract CFA pattern string from RawImage.
fn extract_cfa_pattern(raw: &rawler::RawImage) -> String {
    match &raw.photometric {
        RawPhotometricInterpretation::Cfa(cfg) => cfg.cfa.to_string(),
        RawPhotometricInterpretation::LinearRaw => String::from("LinearRaw"),
        _ => String::from("Unknown"),
    }
}

/// Normalize rawler sensor data to f32 \[0, 1\].
fn normalize_raw_data(raw: &rawler::RawImage) -> core::result::Result<Vec<f32>, RawError> {
    let width = raw.width;
    let height = raw.height;
    let cpp = raw.cpp;
    let total = width * height * cpp;

    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();

    let cfa_opt = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(cfg) => Some(&cfg.cfa),
        _ => None,
    };

    match &raw.data {
        rawler::RawImageData::Integer(data) => {
            if data.len() < total {
                return Err(RawError::InvalidInput(format!(
                    "expected {} pixels, got {}",
                    total,
                    data.len()
                )));
            }

            let mut out = Vec::with_capacity(total);
            for (i, &sample) in data.iter().enumerate().take(total) {
                let ch = if cpp == 1 {
                    if let Some(cfa) = cfa_opt {
                        cfa.color_at(i / width, i % width)
                    } else {
                        0
                    }
                } else {
                    i % cpp
                };
                let bl = black[ch.min(3)];
                let wl = white[ch.min(3)];
                let range = (wl - bl).max(1.0);
                let val = (sample as f32 - bl) / range;
                out.push(val.clamp(0.0, 1.0));
            }
            Ok(out)
        }
        rawler::RawImageData::Float(data) => {
            if data.len() < total {
                return Err(RawError::InvalidInput(format!(
                    "expected {} pixels, got {}",
                    total,
                    data.len()
                )));
            }

            // Fast path: uniform black/white levels (non-CFA or all channels same)
            let uniform_bw = cpp > 1
                || (black[0] == black[1]
                    && black[1] == black[2]
                    && white[0] == white[1]
                    && white[1] == white[2]);

            if uniform_bw {
                let bl = black[0];
                let wl = white[0];
                let range = (wl - bl).max(1.0);
                let inv_range = 1.0 / range;
                Ok(crate::simd::normalize_uniform(
                    &data[..total],
                    bl,
                    inv_range,
                ))
            } else {
                let mut out = Vec::with_capacity(total);
                for (i, &sample) in data.iter().enumerate().take(total) {
                    let ch = if cpp == 1 {
                        if let Some(cfa) = cfa_opt {
                            cfa.color_at(i / width, i % width)
                        } else {
                            0
                        }
                    } else {
                        i % cpp
                    };
                    let bl = black[ch.min(3)];
                    let wl = white[ch.min(3)];
                    let range = (wl - bl).max(1.0);
                    let val = (sample - bl) / range;
                    out.push(val.clamp(0.0, 1.0));
                }
                Ok(out)
            }
        }
    }
}

/// Apply crop from rawler's crop_area or active_area (Rect-based).
fn apply_rawler_crop(
    rgb: &[f32],
    width: usize,
    height: usize,
    raw: &rawler::RawImage,
) -> (Vec<f32>, usize, usize) {
    let rect = raw.crop_area.as_ref().or(raw.active_area.as_ref());

    let Some(rect) = rect else {
        return (rgb.to_vec(), width, height);
    };

    let left = rect.p.x;
    let top = rect.p.y;
    let new_w = rect.d.w;
    let new_h = rect.d.h;

    // Validate
    if left + new_w > width || top + new_h > height {
        return (rgb.to_vec(), width, height);
    }

    let mut cropped = Vec::with_capacity(new_w * new_h * 3);
    for row in top..top + new_h {
        let src_start = (row * width + left) * 3;
        let src_end = src_start + new_w * 3;
        if src_end <= rgb.len() {
            cropped.extend_from_slice(&rgb[src_start..src_end]);
        }
    }

    (cropped, new_w, new_h)
}

/// Handle non-Bayer data (cpp > 1).
fn decode_non_bayer(
    raw: rawler::RawImage,
    normalized: Vec<f32>,
    config: &RawDecodeConfig,
    stop: &dyn Stop,
    original_data: &[u8],
) -> Result<RawDecodeOutput> {
    let width = raw.width;
    let height = raw.height;
    let cpp = raw.cpp;

    let mut rgb = crate::simd::extract_rgb_from_cpp(&normalized, width * height, cpp);

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    color::apply_color_pipeline(&mut rgb, raw.wb_coeffs, raw.xyz_to_cam);

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let (cropped_rgb, out_w, out_h) = if config.apply_crop {
        apply_rawler_crop(&rgb, width, height, &raw)
    } else {
        (rgb, width, height)
    };

    let is_dng = crate::decode::is_dng_data(original_data);

    // Apply EXIF orientation
    let raw_orient = orientation_to_u16(&raw.orientation);
    let (final_rgb, final_w, final_h, final_orient) = if config.apply_orientation && raw_orient > 1
    {
        let (data, w, h) = crate::orient::apply_orientation(cropped_rgb, out_w, out_h, raw_orient);
        (data, w, h, 1u16)
    } else {
        (cropped_rgb, out_w, out_h, raw_orient)
    };

    build_output(
        final_rgb,
        final_w,
        final_h,
        config,
        &raw,
        is_dng,
        final_orient,
    )
}

/// Build final output from processed RGB data.
fn build_output(
    rgb: Vec<f32>,
    width: usize,
    height: usize,
    config: &RawDecodeConfig,
    raw: &rawler::RawImage,
    is_dng: bool,
    orientation: u16,
) -> Result<RawDecodeOutput> {
    let cfa_pattern = extract_cfa_pattern(raw);

    let info = RawInfo {
        width: width as u32,
        height: height as u32,
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
        sensor_width: raw.width as u32,
        sensor_height: raw.height as u32,
        cfa_pattern,
        is_dng,
        orientation,
        bit_depth: Some(crate::decode::bits_from_whitelevel(
            raw.whitelevel.as_bayer_array()[0] as u32,
        )),
    };

    if config.apply_gamma {
        let mut gamma_rgb = rgb;
        color::apply_srgb_gamma(&mut gamma_rgb);
        let u8_data = color::f32_to_u8_srgb(&gamma_rgb);

        let buf = PixelBuffer::from_vec(
            u8_data,
            width as u32,
            height as u32,
            PixelDescriptor::RGB8_SRGB,
        )
        .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

        Ok(RawDecodeOutput { pixels: buf, info })
    } else {
        let byte_data: Vec<u8> = bytemuck::cast_slice::<f32, u8>(&rgb).to_vec();

        let buf = PixelBuffer::from_vec(
            byte_data,
            width as u32,
            height as u32,
            PixelDescriptor::RGBF32_LINEAR,
        )
        .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

        Ok(RawDecodeOutput { pixels: buf, info })
    }
}

/// Convert rawler Orientation to EXIF u16 value.
fn orientation_to_u16(orient: &rawler::Orientation) -> u16 {
    match orient {
        rawler::Orientation::Normal | rawler::Orientation::Unknown => 1,
        rawler::Orientation::HorizontalFlip => 2,
        rawler::Orientation::Rotate180 => 3,
        rawler::Orientation::VerticalFlip => 4,
        rawler::Orientation::Transpose => 5,
        rawler::Orientation::Rotate90 => 6,
        rawler::Orientation::Transverse => 7,
        rawler::Orientation::Rotate270 => 8,
    }
}
