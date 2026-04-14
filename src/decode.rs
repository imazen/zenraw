//! RAW/DNG decoding to zenpixels buffers.
//!
//! Takes camera RAW file bytes, demosaics the Bayer sensor data, applies
//! white balance and color matrix correction, and produces pixel buffers.
//!
//! Output mode is controlled by [`OutputMode`]:
//! - [`Develop`](OutputMode::Develop) (default): display-ready u16 sRGB with tone mapping
//! - [`Linear`](OutputMode::Linear): scene-referred linear f32 with WB + color matrix
//! - [`CameraRaw`](OutputMode::CameraRaw): raw camera values, f32, no color processing

#[cfg(any(feature = "rawloader", feature = "rawler"))]
extern crate std;

#[cfg(any(feature = "rawloader", feature = "rawler"))]
use alloc::vec::Vec;

#[cfg(any(feature = "rawloader", feature = "rawler"))]
use enough::Stop;
#[cfg(feature = "rawloader")]
use whereat::at;
use zenpixels::PixelBuffer;
#[cfg(feature = "rawloader")]
use zenpixels::PixelDescriptor;

pub use crate::dng_render::OutputPrimaries;

#[cfg(feature = "rawloader")]
use crate::color;
use crate::demosaic::DemosaicMethod;
#[cfg(feature = "rawloader")]
use crate::demosaic::demosaic_to_rgb_f32;
#[cfg(feature = "rawloader")]
use crate::error::IntoBufferError;
#[cfg(any(feature = "rawloader", feature = "rawler"))]
use crate::error::{RawError, Result};

/// What rendering to apply during RAW development.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputMode {
    /// Full develop: WB + color matrix + tone curve + gamma.
    /// Produces display-ready u16 (or f32 if sensor data was float) in target primaries.
    #[default]
    Develop,
    /// Linear scene-referred: WB + color matrix only.
    /// Produces f32 linear in target primaries.
    Linear,
    /// Raw camera values: no WB, no color matrix.
    /// Produces f32 in camera color space.
    CameraRaw,
}

/// Configuration for RAW/DNG decoding.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RawDecodeConfig {
    /// Demosaicing algorithm to use.
    pub demosaic: DemosaicMethod,
    /// Maximum pixel count (width × height) before rejecting.
    pub max_pixels: u64,
    /// Output rendering mode.
    ///
    /// - [`Develop`](OutputMode::Develop) (default): display-ready u16 sRGB
    ///   with WB, color matrix, tone curve, and sRGB gamma.
    /// - [`Linear`](OutputMode::Linear): scene-referred f32 linear with WB
    ///   and color matrix applied.
    /// - [`CameraRaw`](OutputMode::CameraRaw): raw camera values as f32,
    ///   no WB or color matrix.
    pub output: OutputMode,
    /// Target output color primaries.
    ///
    /// Controls the RGB primaries of the output buffer. Only affects
    /// [`Develop`](OutputMode::Develop) and [`Linear`](OutputMode::Linear)
    /// modes; [`CameraRaw`](OutputMode::CameraRaw) ignores this.
    ///
    /// Default: [`Srgb`](OutputPrimaries::Srgb).
    pub target: OutputPrimaries,
    /// Exposure compensation in EV (stops).
    ///
    /// Applied as a linear multiplier (`2^exposure_ev`) after WB + color matrix,
    /// before tone mapping and gamma. Positive values brighten; negative darken.
    /// Only affects [`Develop`](OutputMode::Develop) and [`Linear`](OutputMode::Linear).
    ///
    /// Default: `0.0` (no adjustment).
    pub exposure_ev: f32,
    /// Whether to apply the crop specified in the RAW metadata.
    pub apply_crop: bool,
    /// Whether to apply EXIF orientation (rotation/flip) to the output.
    ///
    /// When true (default), the decoded image is rotated/flipped to match
    /// display orientation, and `RawInfo::orientation` is set to 1.
    /// When false, the raw sensor orientation is preserved.
    pub apply_orientation: bool,
    /// Override white balance coefficients (RGB multipliers).
    ///
    /// When set, these multipliers replace the camera's as-shot WB
    /// during the color pipeline. Only has effect in [`Develop`](OutputMode::Develop)
    /// and [`Linear`](OutputMode::Linear) modes.
    ///
    /// Values are relative multipliers (e.g., `[1.0, 1.0, 1.0]` = no WB,
    /// `[2.0, 1.0, 1.5]` = boost red, slight blue).
    pub wb_override: Option<[f32; 3]>,
}

impl Default for RawDecodeConfig {
    fn default() -> Self {
        Self {
            demosaic: DemosaicMethod::default(),
            max_pixels: 200_000_000, // 200 megapixels
            output: OutputMode::Develop,
            target: OutputPrimaries::Srgb,
            exposure_ev: 0.0,
            apply_crop: true,
            apply_orientation: true,
            wb_override: None,
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

    /// Set the output rendering mode.
    ///
    /// - [`Develop`](OutputMode::Develop) (default): display-ready u16 sRGB
    /// - [`Linear`](OutputMode::Linear): scene-referred f32 linear
    /// - [`CameraRaw`](OutputMode::CameraRaw): raw camera f32 values
    #[must_use]
    pub fn with_output(mut self, mode: OutputMode) -> Self {
        self.output = mode;
        self
    }

    /// Set the target output color primaries.
    ///
    /// Affects [`Develop`](OutputMode::Develop) and [`Linear`](OutputMode::Linear);
    /// ignored for [`CameraRaw`](OutputMode::CameraRaw).
    #[must_use]
    pub fn with_target(mut self, primaries: OutputPrimaries) -> Self {
        self.target = primaries;
        self
    }

    /// Set exposure compensation in EV (stops).
    ///
    /// Applied as `2^ev` multiplier after WB + color matrix.
    /// Positive brightens, negative darkens.
    #[must_use]
    pub fn with_exposure_ev(mut self, ev: f32) -> Self {
        self.exposure_ev = ev;
        self
    }

    /// Set whether to apply the crop from RAW metadata (default: true).
    #[must_use]
    pub fn with_crop(mut self, apply: bool) -> Self {
        self.apply_crop = apply;
        self
    }

    /// Set whether to apply EXIF orientation transform (default: true).
    ///
    /// When enabled, the output image matches display orientation and
    /// width/height reflect the rotated dimensions.
    #[must_use]
    pub fn with_orientation(mut self, apply: bool) -> Self {
        self.apply_orientation = apply;
        self
    }

    /// Override white balance with custom RGB multipliers.
    ///
    /// Replaces the camera's as-shot WB during color pipeline processing.
    /// Only effective in [`Develop`](OutputMode::Develop) and [`Linear`](OutputMode::Linear) modes.
    #[must_use]
    pub fn with_wb(mut self, rgb: [f32; 3]) -> Self {
        self.wb_override = Some(rgb);
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

/// Sensor data layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SensorLayout {
    /// Standard 2×2 Bayer CFA (RGGB, BGGR, GRBG, GBRG).
    #[default]
    Bayer,
    /// Fujifilm X-Trans 6×6 CFA.
    XTrans,
    /// Already demosaiced linear RGB (some DNGs, Apple ProRAW).
    LinearRaw,
    /// Unknown or unsupported layout.
    Unknown,
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
    /// EXIF orientation (1–8, EXIF spec).
    pub orientation: u16,
    /// Sensor bit depth (e.g., 10, 12, 14), estimated from white level.
    pub bit_depth: Option<u8>,

    // ── Raw pipeline metadata ──
    /// White balance coefficients as-shot (RGBE, 4 channels).
    /// These are the multipliers the camera recorded for the scene illuminant.
    pub wb_coeffs: [f32; 4],
    /// Camera→XYZ color matrix (4×3, row-major).
    /// Maps camera RGB to CIE XYZ. Used with WB to produce sRGB output.
    pub color_matrix: [[f32; 3]; 4],
    /// Black level per channel (RGBE order, sensor DN units).
    pub black_levels: [f32; 4],
    /// White level per channel (RGBE order, sensor DN units).
    pub white_levels: [f32; 4],
    /// Crop rectangle from camera metadata: `[top, right, bottom, left]` in pixels.
    /// `None` if the camera provided no crop.
    pub crop_rect: Option<[u32; 4]>,
    /// Active sensor area (usable region before aesthetic crop).
    /// Format: `[x, y, width, height]`. `None` if not available.
    pub active_area: Option<[u32; 4]>,
    /// DNG BaselineExposure in EV (how much to scale for correct brightness).
    pub baseline_exposure: Option<f64>,
    /// Sensor data layout (Bayer, X-Trans, LinearRaw, etc.).
    pub sensor_layout: SensorLayout,
}

/// Probe a RAW/DNG file for metadata without decoding pixels.
///
/// Returns metadata about the image (dimensions, camera info, etc.).
#[cfg(feature = "rawloader")]
pub(crate) fn probe(data: &[u8], stop: &dyn Stop) -> Result<RawInfo> {
    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let raw =
        rawloader::decode(&mut std::io::Cursor::new(data)).map_err(|e| at!(RawError::from(e)))?;

    let is_dng = is_dng_data(data);

    let crop_rect = if raw.crops.iter().any(|&c| c > 0) {
        Some([
            raw.crops[0] as u32,
            raw.crops[1] as u32,
            raw.crops[2] as u32,
            raw.crops[3] as u32,
        ])
    } else {
        None
    };

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
        bit_depth: Some(bits_from_whitelevel(raw.whitelevels[0] as u32)),
        wb_coeffs: raw.wb_coeffs,
        color_matrix: raw.xyz_to_cam,
        black_levels: [
            raw.blacklevels[0] as f32,
            raw.blacklevels[1] as f32,
            raw.blacklevels[2] as f32,
            raw.blacklevels[3] as f32,
        ],
        white_levels: [
            raw.whitelevels[0] as f32,
            raw.whitelevels[1] as f32,
            raw.whitelevels[2] as f32,
            raw.whitelevels[3] as f32,
        ],
        crop_rect,
        active_area: None, // rawloader doesn't distinguish active_area from crop
        baseline_exposure: None, // rawloader doesn't extract this
        sensor_layout: if raw.cpp > 1 {
            SensorLayout::LinearRaw
        } else {
            SensorLayout::Bayer
        },
    })
}

/// Decode a RAW/DNG file to a pixel buffer.
///
/// The full pipeline:
/// 1. Parse RAW file with rawloader
/// 2. Normalize sensor data to \[0, 1\] using black/white levels
/// 3. Demosaic Bayer CFA data to RGB
/// 4. Apply white balance + camera→sRGB color matrix
/// 5. Optionally apply sRGB gamma curve
/// 6. Optionally crop to the camera's recommended region
/// 7. Return as a PixelBuffer
#[cfg(feature = "rawloader")]
pub(crate) fn decode(
    data: &[u8],
    config: &RawDecodeConfig,
    stop: &dyn Stop,
) -> Result<RawDecodeOutput> {
    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // RAW files have substantial headers; reject obviously-too-short inputs
    // before passing to rawloader, which can panic on very short data.
    if data.len() < 64 {
        return Err(at!(RawError::Decode(
            "input too short to be a valid RAW file".into()
        )));
    }

    // Step 1: Parse
    // rawloader can panic on malformed inputs, so catch those and convert to errors
    let data_vec = data.to_vec();
    let raw =
        std::panic::catch_unwind(move || rawloader::decode(&mut std::io::Cursor::new(&data_vec)))
            .map_err(|_| {
                at!(RawError::Decode(
                    "rawloader panicked on malformed input".into()
                ))
            })?
            .map_err(|e| at!(RawError::from(e)))?;

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

    // Step 4: Color pipeline (WB + camera→sRGB) — skipped for CameraRaw
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
        color::apply_color_pipeline(&mut rgb, wb, raw.xyz_to_cam, config.target);

        // Apply exposure_ev if nonzero
        if config.exposure_ev.abs() > 1e-6 {
            let mult = 2.0f32.powf(config.exposure_ev);
            for v in rgb.iter_mut() {
                *v *= mult;
            }
        }
    }

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    // Step 5: Crop
    let (cropped_rgb, out_w, out_h) = if config.apply_crop {
        apply_crop(&rgb, width, height, &raw.crops)
    } else {
        (rgb, width, height)
    };

    stop.check().map_err(|r| at!(RawError::from(r)))?;

    let is_dng = is_dng_data(data);

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

    let crop_rect = if raw.crops.iter().any(|&c| c > 0) {
        Some([
            raw.crops[0] as u32,
            raw.crops[1] as u32,
            raw.crops[2] as u32,
            raw.crops[3] as u32,
        ])
    } else {
        None
    };

    // Step 7: Convert to output format
    let info = RawInfo {
        width: final_w as u32,
        height: final_h as u32,
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
        sensor_width: raw.width as u32,
        sensor_height: raw.height as u32,
        cfa_pattern: raw.cfa.to_string(),
        is_dng,
        orientation: final_orient,
        bit_depth: Some(bits_from_whitelevel(raw.whitelevels[0] as u32)),
        wb_coeffs: raw.wb_coeffs,
        color_matrix: raw.xyz_to_cam,
        black_levels: [
            raw.blacklevels[0] as f32,
            raw.blacklevels[1] as f32,
            raw.blacklevels[2] as f32,
            raw.blacklevels[3] as f32,
        ],
        white_levels: [
            raw.whitelevels[0] as f32,
            raw.whitelevels[1] as f32,
            raw.whitelevels[2] as f32,
            raw.whitelevels[3] as f32,
        ],
        crop_rect,
        active_area: None,
        baseline_exposure: None,
        sensor_layout: SensorLayout::Bayer,
    };

    match config.output {
        OutputMode::Develop => {
            let mut gamma_rgb = final_rgb;
            color::apply_srgb_gamma(&mut gamma_rgb);
            let u16_data = color::f32_to_u16(&gamma_rgb);

            let buf = PixelBuffer::from_vec(
                u16_data,
                final_w as u32,
                final_h as u32,
                PixelDescriptor::RGB16_SRGB.with_primaries(config.target.to_color_primaries()),
            )
            .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

            Ok(RawDecodeOutput { pixels: buf, info })
        }
        OutputMode::Linear => {
            let byte_data: Vec<u8> = bytemuck::cast_slice::<f32, u8>(&final_rgb).to_vec();

            let buf = PixelBuffer::from_vec(
                byte_data,
                final_w as u32,
                final_h as u32,
                PixelDescriptor::RGBF32_LINEAR.with_primaries(config.target.to_color_primaries()),
            )
            .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

            Ok(RawDecodeOutput { pixels: buf, info })
        }
        OutputMode::CameraRaw => {
            let byte_data: Vec<u8> = bytemuck::cast_slice::<f32, u8>(&final_rgb).to_vec();

            let buf = PixelBuffer::from_vec(
                byte_data,
                final_w as u32,
                final_h as u32,
                PixelDescriptor::RGBF32_LINEAR.with_primaries(zenpixels::ColorPrimaries::Unknown),
            )
            .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

            Ok(RawDecodeOutput { pixels: buf, info })
        }
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

    // Apply color pipeline — skipped for CameraRaw
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
        color::apply_color_pipeline(&mut rgb, wb, raw.xyz_to_cam, config.target);

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
        apply_crop(&rgb, width, height, &raw.crops)
    } else {
        (rgb, width, height)
    };

    let is_dng = false; // Can't easily check without original data here

    // Apply EXIF orientation
    let raw_orient = orientation_to_u16(&raw.orientation);
    let (final_rgb, final_w, final_h, final_orient) = if config.apply_orientation && raw_orient > 1
    {
        let (data, w, h) = crate::orient::apply_orientation(cropped_rgb, out_w, out_h, raw_orient);
        (data, w, h, 1u16)
    } else {
        (cropped_rgb, out_w, out_h, raw_orient)
    };

    let crop_rect = if raw.crops.iter().any(|&c| c > 0) {
        Some([
            raw.crops[0] as u32,
            raw.crops[1] as u32,
            raw.crops[2] as u32,
            raw.crops[3] as u32,
        ])
    } else {
        None
    };

    let info = RawInfo {
        width: final_w as u32,
        height: final_h as u32,
        make: raw.clean_make,
        model: raw.clean_model,
        sensor_width: raw.width as u32,
        sensor_height: raw.height as u32,
        cfa_pattern: raw.cfa.to_string(),
        is_dng,
        orientation: final_orient,
        bit_depth: Some(bits_from_whitelevel(raw.whitelevels[0] as u32)),
        wb_coeffs: raw.wb_coeffs,
        color_matrix: raw.xyz_to_cam,
        black_levels: [
            raw.blacklevels[0] as f32,
            raw.blacklevels[1] as f32,
            raw.blacklevels[2] as f32,
            raw.blacklevels[3] as f32,
        ],
        white_levels: [
            raw.whitelevels[0] as f32,
            raw.whitelevels[1] as f32,
            raw.whitelevels[2] as f32,
            raw.whitelevels[3] as f32,
        ],
        crop_rect,
        active_area: None,
        baseline_exposure: None,
        sensor_layout: SensorLayout::LinearRaw,
    };

    match config.output {
        OutputMode::Develop => {
            let mut gamma_rgb = final_rgb;
            color::apply_srgb_gamma(&mut gamma_rgb);
            let u16_data = color::f32_to_u16(&gamma_rgb);

            let buf = PixelBuffer::from_vec(
                u16_data,
                final_w as u32,
                final_h as u32,
                PixelDescriptor::RGB16_SRGB.with_primaries(config.target.to_color_primaries()),
            )
            .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

            Ok(RawDecodeOutput { pixels: buf, info })
        }
        OutputMode::Linear => {
            let byte_data: Vec<u8> = bytemuck::cast_slice::<f32, u8>(&final_rgb).to_vec();

            let buf = PixelBuffer::from_vec(
                byte_data,
                final_w as u32,
                final_h as u32,
                PixelDescriptor::RGBF32_LINEAR.with_primaries(config.target.to_color_primaries()),
            )
            .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

            Ok(RawDecodeOutput { pixels: buf, info })
        }
        OutputMode::CameraRaw => {
            let byte_data: Vec<u8> = bytemuck::cast_slice::<f32, u8>(&final_rgb).to_vec();

            let buf = PixelBuffer::from_vec(
                byte_data,
                final_w as u32,
                final_h as u32,
                PixelDescriptor::RGBF32_LINEAR.with_primaries(zenpixels::ColorPrimaries::Unknown),
            )
            .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?;

            Ok(RawDecodeOutput { pixels: buf, info })
        }
    }
}

/// Normalize raw sensor data to f32 \[0, 1\] using black/white levels.
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
    // Look for DNGVersion tag (0xC612) in the first 4KB.
    // TIFF endianness determines byte order of the 2-byte tag id on disk:
    // little-endian TIFF stores it as [0x12, 0xC6]; big-endian as [0xC6, 0x12].
    let search_len = data.len().min(4096);
    let haystack = &data[..search_len];
    let needle: &[u8] = if data[0] == b'I' {
        &[0x12, 0xC6]
    } else {
        &[0xC6, 0x12]
    };
    memchr::memmem::find(haystack, needle).is_some()
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

/// Estimate sensor bit depth from the white level value.
///
/// Returns the number of bits needed to represent the white level
/// (e.g., white level 4095 → 12 bits, 16383 → 14 bits).
pub(crate) fn bits_from_whitelevel(wl: u32) -> u8 {
    if wl == 0 {
        return 16;
    }
    (32 - wl.leading_zeros()) as u8
}

/// Detect whether a byte slice looks like a supported RAW/DNG file.
///
/// Checks for TIFF-based RAW formats and known camera RAW magic bytes.
pub(crate) fn is_raw_file(data: &[u8]) -> bool {
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

    // Canon CR3 (ISO BMFF with "crx " major brand in ftyp box)
    if &data[4..8] == b"ftyp" && &data[8..12] == b"crx " {
        return true;
    }

    false
}
