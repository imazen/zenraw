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

use rawler::imgop::xyz::Illuminant;

use crate::color;
use crate::decode::{OutputMode, RawDecodeConfig, RawDecodeOutput, RawInfo, SensorLayout};
use crate::demosaic::{CfaPattern, demosaic_to_rgb_f32, demosaic_xtrans_bilinear};
use crate::error::{IntoBufferError, RawError, Result};

/// Extract xyz_to_cam matrix from rawler's color_matrix HashMap.
///
/// rawler's `xyz_to_cam` field is deprecated (all zeros). The actual
/// matrix lives in `color_matrix: HashMap<Illuminant, FlatColorMatrix>`.
/// Prefers D65, falls back to D50, then any available illuminant.
fn extract_xyz_to_cam(raw: &rawler::RawImage) -> [[f32; 3]; 4] {
    let mat = raw
        .color_matrix
        .get(&Illuminant::D65)
        .or_else(|| raw.color_matrix.get(&Illuminant::D50))
        .or_else(|| raw.color_matrix.values().next());

    if let Some(flat) = mat.filter(|f| f.len() >= 9) {
        return [
            [flat[0], flat[1], flat[2]],
            [flat[3], flat[4], flat[5]],
            [flat[6], flat[7], flat[8]],
            [0.0, 0.0, 0.0],
        ];
    }
    // Fallback: check deprecated field
    if raw.xyz_to_cam.iter().any(|r| r.iter().any(|&v| v != 0.0)) {
        return raw.xyz_to_cam;
    }
    // No matrix available — identity-ish fallback
    [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0],
    ]
}

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

    // Bound probe outputs against the default pixel cap; even probe-only
    // callers shouldn't see arbitrarily large dimensions reported back.
    let limit = crate::decode::RawDecodeConfig::default().max_pixels;
    match (width as u64).checked_mul(height as u64) {
        Some(p) if p > limit => {
            return Err(at!(RawError::LimitExceeded(format!(
                "image {width}x{height} = {p} pixels exceeds probe limit of {limit}"
            ))));
        }
        None => {
            return Err(at!(RawError::LimitExceeded(format!(
                "image {width}x{height} dimensions overflow"
            ))));
        }
        _ => {}
    }

    let xyz_to_cam = extract_xyz_to_cam(&raw);
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();

    let crop_rect = raw.crop_area.as_ref().map(|r| {
        [
            r.p.y as u32,
            (raw.width as u32).saturating_sub(r.p.x as u32 + r.d.w as u32),
            (raw.height as u32).saturating_sub(r.p.y as u32 + r.d.h as u32),
            r.p.x as u32,
        ]
    });
    let active_area = raw
        .active_area
        .as_ref()
        .map(|r| [r.p.x as u32, r.p.y as u32, r.d.w as u32, r.d.h as u32]);

    let sensor_layout = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(cfg) => {
            let s = cfg.cfa.to_string();
            if s.len() > 4 {
                SensorLayout::XTrans
            } else {
                SensorLayout::Bayer
            }
        }
        RawPhotometricInterpretation::LinearRaw => SensorLayout::LinearRaw,
        _ => SensorLayout::Unknown,
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
        bit_depth: Some(crate::decode::bits_from_whitelevel(white[0] as u32)),
        wb_coeffs: raw.wb_coeffs,
        color_matrix: xyz_to_cam,
        black_levels: black,
        white_levels: white,
        crop_rect,
        active_area,
        baseline_exposure: None,
        sensor_layout,
    })
}

/// Decode a RAW/DNG file to a pixel buffer using rawler.
pub fn decode(data: &[u8], config: &RawDecodeConfig, stop: &dyn Stop) -> Result<RawDecodeOutput> {
    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // RAW files have substantial headers; reject obviously-too-short inputs
    if data.len() < 64 {
        return Err(at!(RawError::Decode(
            "input too short to be a valid RAW file".into()
        )));
    }

    // Step 1: Parse
    let source = RawSource::new_from_slice(data);
    let params = RawDecodeParams::default();
    // rawler can panic on malformed inputs, so catch those and convert to
    // errors — mirrors the rawloader backend in decode.rs so neither path can
    // crash the host on crafted input.
    let raw = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rawler::decode(&source, &params)
    }))
    .map_err(|_| at!(RawError::Decode("rawler panicked on malformed input".into())))?
    .map_err(|e| at!(RawError::Decode(format!("{e}"))))?;

    let xyz_to_cam = extract_xyz_to_cam(&raw);
    let width = raw.width;
    let height = raw.height;

    // Check limits (pixels + RGB f32 working-set).
    crate::decode::enforce_decode_limits(width as u64, height as u64, config)?;

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 2: Extract and normalize sensor data to f32 [0, 1]
    let normalized = normalize_raw_data(&raw).map_err(|e| at!(e))?;

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // For non-Bayer data (cpp > 1), skip demosaicing
    if raw.cpp > 1 {
        return decode_non_bayer(raw, normalized, config, stop, data, xyz_to_cam);
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

    let rgb = if cfa_str.len() > 4 {
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

    // Step 4: Crop before color (Develop mode needs dimensions for DngPipeline)
    let (cropped_rgb, out_w, out_h) = if config.apply_crop {
        apply_rawler_crop(&rgb, width, height, &raw)
    } else {
        (rgb, width, height)
    };

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 5: Apply EXIF orientation
    let raw_orient = orientation_to_u16(&raw.orientation);
    let (final_rgb, final_w, final_h, final_orient) = if config.apply_orientation && raw_orient > 1
    {
        let (data, w, h) = crate::orient::apply_orientation(cropped_rgb, out_w, out_h, raw_orient);
        (data, w, h, 1u16)
    } else {
        (cropped_rgb, out_w, out_h, raw_orient)
    };

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let is_dng = crate::decode::is_dng_data(data);

    // Step 6: Color pipeline
    match config.output {
        OutputMode::Develop => {
            // Auto-develop: use DngPipeline with tone curve for display-ready output
            auto_develop_output(
                final_rgb,
                final_w,
                final_h,
                &raw,
                xyz_to_cam,
                is_dng,
                final_orient,
                config,
            )
        }
        OutputMode::Linear => {
            // WB + color matrix, output f32
            let mut colored = final_rgb;
            let wb = if let Some(override_wb) = config.wb_override {
                [
                    override_wb[0],
                    override_wb[1],
                    override_wb[2],
                    override_wb[1],
                ]
            } else {
                raw.wb_coeffs
            };
            color::apply_color_pipeline(&mut colored, wb, xyz_to_cam, config.target);

            // Apply exposure_ev if nonzero
            if config.exposure_ev.abs() > 1e-6 {
                let mult = 2.0f32.powf(config.exposure_ev);
                for v in colored.iter_mut() {
                    *v *= mult;
                }
            }

            build_linear_output(
                colored,
                final_w,
                final_h,
                &raw,
                xyz_to_cam,
                is_dng,
                final_orient,
                config.target.to_color_primaries(),
            )
        }
        OutputMode::CameraRaw => {
            // No color processing — pass through camera-space data
            build_linear_output(
                final_rgb,
                final_w,
                final_h,
                &raw,
                xyz_to_cam,
                is_dng,
                final_orient,
                zenpixels::ColorPrimaries::Unknown,
            )
        }
    }
}

/// Auto-develop: render camera-space linear f32 to display-ready sRGB u16
/// using DngPipeline with a filmic tone curve.
#[allow(clippy::too_many_arguments)]
fn auto_develop_output(
    camera_linear: Vec<f32>,
    width: usize,
    height: usize,
    raw: &rawler::RawImage,
    xyz_to_cam: [[f32; 3]; 4],
    is_dng: bool,
    orientation: u16,
    config: &RawDecodeConfig,
) -> Result<RawDecodeOutput> {
    use crate::dng_render::DngPipeline;

    let cfa_pattern = extract_cfa_pattern(raw);
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();

    let sensor_layout = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(cfg) => {
            let s = cfg.cfa.to_string();
            if s.len() > 4 {
                SensorLayout::XTrans
            } else {
                SensorLayout::Bayer
            }
        }
        RawPhotometricInterpretation::LinearRaw => SensorLayout::LinearRaw,
        _ => SensorLayout::Unknown,
    };

    // Build RawInfo for DngPipeline::from_raw_info
    let info = RawInfo {
        width: width as u32,
        height: height as u32,
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
        sensor_width: raw.width as u32,
        sensor_height: raw.height as u32,
        cfa_pattern: cfa_pattern.clone(),
        is_dng,
        orientation,
        bit_depth: Some(crate::decode::bits_from_whitelevel(white[0] as u32)),
        wb_coeffs: raw.wb_coeffs,
        color_matrix: xyz_to_cam,
        black_levels: black,
        white_levels: white,
        crop_rect: None,
        active_area: None,
        baseline_exposure: None,
        sensor_layout,
    };

    // Build color pipeline (DngPipeline for proper Bradford adaptation + WB)
    let pipeline = DngPipeline::from_raw_info(&info, config.target);

    let u16_data = if let Some(pipeline) = pipeline {
        // DngPipeline handles: exposure + WB + color matrix → linear output primaries
        // We skip its tone curve and apply dt_sigmoid with hue preservation instead
        let mut pixels = camera_linear;

        // 1. BaselineExposure + user exposure_ev
        let total_ev = pipeline.baseline_exposure as f32 + config.exposure_ev;
        let ev_mult = 2.0f32.powf(total_ev);
        if (ev_mult - 1.0).abs() > 1e-6 {
            for v in pixels.iter_mut() {
                *v *= ev_mult;
            }
        }

        // 2. Camera → output color matrix (WB baked in)
        crate::dng_render::apply_matrix_rgb(&mut pixels, &pipeline.camera_to_output);

        // Clamp negatives (matrix can produce them)
        for v in pixels.iter_mut() {
            *v = v.max(0.0);
        }

        // 3. Exposure boost (~1.85x, matching the zenfilters FiveK parity baseline)
        let exposure_boost = 1.85f32;
        for v in pixels.iter_mut() {
            *v *= exposure_boost;
        }

        // 4. Scene-referred sigmoid tone mapping with hue preservation
        let params = crate::dt_sigmoid::default_params();
        crate::dt_sigmoid::apply_dt_sigmoid(&mut pixels, &params);

        // 5. sRGB gamma → u16
        crate::dng_render::linear_to_srgb_u16(&pixels)
    } else {
        // Fallback: basic color pipeline + gamma if DngPipeline can't be built
        let mut rgb = camera_linear;
        color::apply_color_pipeline(&mut rgb, raw.wb_coeffs, xyz_to_cam, config.target);

        // Apply exposure_ev if nonzero
        if config.exposure_ev.abs() > 1e-6 {
            let mult = 2.0f32.powf(config.exposure_ev);
            for v in rgb.iter_mut() {
                *v *= mult;
            }
        }

        color::apply_srgb_gamma(&mut rgb);
        color::f32_to_u16(&rgb)
    };

    let buf = PixelBuffer::from_vec(
        u16_data,
        width as u32,
        height as u32,
        PixelDescriptor::RGB16_SRGB.with_primaries(config.target.to_color_primaries()),
    )
    .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

    Ok(RawDecodeOutput { pixels: buf, info })
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
    xyz_to_cam: [[f32; 3]; 4],
) -> Result<RawDecodeOutput> {
    let width = raw.width;
    let height = raw.height;
    let cpp = raw.cpp;

    let mut rgb = crate::simd::extract_rgb_from_cpp(&normalized, width * height, cpp);

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    if config.output != OutputMode::CameraRaw {
        let wb = if let Some(override_wb) = config.wb_override {
            [
                override_wb[0],
                override_wb[1],
                override_wb[2],
                override_wb[1],
            ]
        } else {
            raw.wb_coeffs
        };
        color::apply_color_pipeline(&mut rgb, wb, xyz_to_cam, config.target);

        // Apply exposure_ev if nonzero
        if config.exposure_ev.abs() > 1e-6 {
            let mult = 2.0f32.powf(config.exposure_ev);
            for v in rgb.iter_mut() {
                *v *= mult;
            }
        }
    }

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

    let primaries = config.target.to_color_primaries();

    match config.output {
        OutputMode::Develop => {
            // For non-bayer, use basic develop (no DngPipeline)
            build_develop_output(
                final_rgb,
                final_w,
                final_h,
                &raw,
                xyz_to_cam,
                is_dng,
                final_orient,
                primaries,
            )
        }
        OutputMode::Linear | OutputMode::CameraRaw => build_linear_output(
            final_rgb,
            final_w,
            final_h,
            &raw,
            xyz_to_cam,
            is_dng,
            final_orient,
            primaries,
        ),
    }
}

/// Build RawInfo from rawler RawImage metadata.
#[allow(clippy::too_many_arguments)]
fn build_raw_info(
    width: usize,
    height: usize,
    raw: &rawler::RawImage,
    xyz_to_cam: [[f32; 3]; 4],
    is_dng: bool,
    orientation: u16,
) -> RawInfo {
    let cfa_pattern = extract_cfa_pattern(raw);
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();

    let crop_rect = raw.crop_area.as_ref().map(|r| {
        [
            r.p.y as u32,
            (raw.width as u32).saturating_sub(r.p.x as u32 + r.d.w as u32),
            (raw.height as u32).saturating_sub(r.p.y as u32 + r.d.h as u32),
            r.p.x as u32,
        ]
    });
    let active_area = raw
        .active_area
        .as_ref()
        .map(|r| [r.p.x as u32, r.p.y as u32, r.d.w as u32, r.d.h as u32]);

    let sensor_layout = match &raw.photometric {
        RawPhotometricInterpretation::Cfa(cfg) => {
            let s = cfg.cfa.to_string();
            if s.len() > 4 {
                SensorLayout::XTrans
            } else {
                SensorLayout::Bayer
            }
        }
        RawPhotometricInterpretation::LinearRaw => SensorLayout::LinearRaw,
        _ => SensorLayout::Unknown,
    };

    RawInfo {
        width: width as u32,
        height: height as u32,
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
        sensor_width: raw.width as u32,
        sensor_height: raw.height as u32,
        cfa_pattern,
        is_dng,
        orientation,
        bit_depth: Some(crate::decode::bits_from_whitelevel(white[0] as u32)),
        wb_coeffs: raw.wb_coeffs,
        color_matrix: xyz_to_cam,
        black_levels: black,
        white_levels: white,
        crop_rect,
        active_area,
        baseline_exposure: None,
        sensor_layout,
    }
}

/// Build linear f32 output from processed RGB data.
#[allow(clippy::too_many_arguments)]
fn build_linear_output(
    rgb: Vec<f32>,
    width: usize,
    height: usize,
    raw: &rawler::RawImage,
    xyz_to_cam: [[f32; 3]; 4],
    is_dng: bool,
    orientation: u16,
    primaries: zenpixels::ColorPrimaries,
) -> Result<RawDecodeOutput> {
    let info = build_raw_info(width, height, raw, xyz_to_cam, is_dng, orientation);
    let byte_data: Vec<u8> = bytemuck::cast_slice::<f32, u8>(&rgb).to_vec();

    let buf = PixelBuffer::from_vec(
        byte_data,
        width as u32,
        height as u32,
        PixelDescriptor::RGBF32_LINEAR.with_primaries(primaries),
    )
    .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

    Ok(RawDecodeOutput { pixels: buf, info })
}

/// Build display-ready u16 output (simple path without DngPipeline).
#[allow(clippy::too_many_arguments)]
fn build_develop_output(
    rgb: Vec<f32>,
    width: usize,
    height: usize,
    raw: &rawler::RawImage,
    xyz_to_cam: [[f32; 3]; 4],
    is_dng: bool,
    orientation: u16,
    primaries: zenpixels::ColorPrimaries,
) -> Result<RawDecodeOutput> {
    let info = build_raw_info(width, height, raw, xyz_to_cam, is_dng, orientation);
    let mut gamma_rgb = rgb;
    color::apply_srgb_gamma(&mut gamma_rgb);
    let u16_data = color::f32_to_u16(&gamma_rgb);

    let buf = PixelBuffer::from_vec(
        u16_data,
        width as u32,
        height as u32,
        PixelDescriptor::RGB16_SRGB.with_primaries(primaries),
    )
    .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

    Ok(RawDecodeOutput { pixels: buf, info })
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
