//! darktable-cli backend for scene-referred RAW decoding.
//!
//! Shells out to `darktable-cli` for high-quality RAW processing with access to
//! 900+ cameras, advanced demosaicing, highlight recovery, and lens correction.
//!
//! Output is linear scene-referred f32 in Rec.709 primaries (sRGB gamut, linear
//! transfer) by default, matching [`zenpixels::PixelDescriptor::RGBF32_LINEAR`].
//!
//! # Requirements
//!
//! - `darktable-cli` must be in `$PATH` (darktable 4.0+ recommended, 5.0+ preferred)
//! - Feature-gated behind `darktable`

extern crate std;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use std::io::Read;
use std::path::Path;
use std::process::Command;

use whereat::at;
use zenpixels::{PixelBuffer, PixelDescriptor};

use crate::decode::{RawDecodeOutput, RawInfo};
use crate::error::{IntoBufferError, RawError, Result};

/// darktable output ICC profile type.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub enum DtColorProfile {
    /// Linear Rec.709 / sRGB primaries, linear transfer.
    /// Matches `PixelDescriptor::RGBF32_LINEAR`.
    #[default]
    LinearRec709,
    /// Linear Rec.2020 wide gamut, linear transfer.
    LinearRec2020,
    /// sRGB with standard transfer function.
    Srgb,
}

impl DtColorProfile {
    fn icc_type_arg(&self) -> &'static str {
        match self {
            Self::LinearRec709 => "LIN_REC709",
            Self::LinearRec2020 => "LIN_REC2020",
            Self::Srgb => "SRGB",
        }
    }
}

/// Configuration for darktable-cli backend.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DtConfig {
    /// Path to darktable-cli binary. Default: search `$PATH`.
    pub cli_path: Option<String>,
    /// Output color profile. Default: linear Rec.709.
    pub color_profile: DtColorProfile,
    /// Optional XMP sidecar content to apply.
    pub xmp: Option<String>,
    /// Maximum pixel count before rejecting.
    pub max_pixels: u64,
    /// Subprocess timeout in seconds. Default: 120.
    pub timeout_secs: u64,
}

impl Default for DtConfig {
    fn default() -> Self {
        Self {
            cli_path: None,
            color_profile: DtColorProfile::default(),
            xmp: None,
            max_pixels: 200_000_000,
            timeout_secs: 120,
        }
    }
}

impl DtConfig {
    /// Create with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the darktable-cli binary path.
    #[must_use]
    pub fn with_cli_path(mut self, path: impl Into<String>) -> Self {
        self.cli_path = Some(path.into());
        self
    }

    /// Set the output color profile.
    #[must_use]
    pub fn with_color_profile(mut self, profile: DtColorProfile) -> Self {
        self.color_profile = profile;
        self
    }

    /// Set XMP sidecar content to apply during processing.
    #[must_use]
    pub fn with_xmp(mut self, xmp: impl Into<String>) -> Self {
        self.xmp = Some(xmp.into());
        self
    }

    /// Set maximum allowed pixel count.
    #[must_use]
    pub fn with_max_pixels(mut self, max: u64) -> Self {
        self.max_pixels = max;
        self
    }

    /// Set subprocess timeout in seconds (default: 120).
    #[must_use]
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

/// Check if darktable-cli is available.
pub fn is_available() -> bool {
    find_cli(None).is_some()
}

/// Get the darktable version string, if available.
pub fn version() -> Option<String> {
    let cli = find_cli(None)?;
    let output = Command::new(&cli).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Some(stderr.trim().to_string())
    } else {
        Some(text.to_string())
    }
}

/// Decode a RAW file from disk using darktable-cli.
///
/// Produces scene-referred linear f32 output by default (PFM interchange).
pub fn decode_file(path: &Path, config: &DtConfig) -> Result<RawDecodeOutput> {
    let cli = find_cli(config.cli_path.as_deref()).ok_or_else(|| {
        at!(RawError::Unsupported(
            "darktable-cli not found in PATH".into()
        ))
    })?;

    // Validate input exists
    if !path.exists() {
        return Err(at!(RawError::InvalidInput(format!(
            "file not found: {}",
            path.display()
        ))));
    }

    // Create unique temp directory per invocation to avoid parallel conflicts
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let tmp_dir = std::env::temp_dir().join(format!("zenraw_dt_{pid}_{id}"));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| at!(RawError::Decode(e.to_string())))?;

    let out_path = tmp_dir.join("output.pfm");

    // Write XMP sidecar if provided
    let xmp_path = if let Some(ref xmp) = config.xmp {
        let p = tmp_dir.join("sidecar.xmp");
        std::fs::write(&p, xmp).map_err(|e| at!(RawError::Decode(e.to_string())))?;
        Some(p)
    } else {
        None
    };

    // Build command
    let mut cmd = Command::new(&cli);
    cmd.arg(path);

    if let Some(ref xmp) = xmp_path {
        cmd.arg(xmp);
    }

    cmd.arg(&out_path);

    // Use isolated config to avoid user presets interfering
    let dt_config = tmp_dir.join("dt_config");
    std::fs::create_dir_all(&dt_config).map_err(|e| at!(RawError::Decode(e.to_string())))?;

    cmd.args([
        "--apply-custom-presets",
        "false",
        "--out-ext",
        "pfm",
        "--icc-type",
        config.color_profile.icc_type_arg(),
        "--core",
        "--library",
        ":memory:",
        "--configdir",
    ]);
    cmd.arg(&dt_config);
    // Disable auto-workflow (sigmoid/filmic) for pure linear output
    cmd.args(["--conf", "plugins/darkroom/workflow=none"]);

    // Run darktable-cli with timeout to prevent indefinite blocking
    let result = (|| -> Result<(Vec<f32>, u32, u32)> {
        let mut child = cmd.spawn().map_err(|e| {
            at!(RawError::Decode(format!(
                "failed to run darktable-cli: {e}"
            )))
        })?;

        let timeout = std::time::Duration::from_secs(config.timeout_secs);
        let start = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() >= timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(at!(RawError::Decode(format!(
                            "darktable-cli timed out after {}s",
                            config.timeout_secs
                        ))));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(at!(RawError::Decode(format!(
                        "failed to wait on darktable-cli: {e}"
                    ))));
                }
            }
        };

        if !status.success() {
            return Err(at!(RawError::Decode(format!(
                "darktable-cli exited with {status}"
            ))));
        }

        // Parse PFM output
        let pfm_data = std::fs::read(&out_path)
            .map_err(|e| at!(RawError::Decode(format!("failed to read PFM output: {e}"))))?;

        parse_pfm(&pfm_data)
    })();

    // Clean up entire temp directory on ALL paths (best effort)
    let _ = std::fs::remove_dir_all(&tmp_dir);

    let (pixels_f32, width, height) = result?;

    // Check limits
    let total = width as u64 * height as u64;
    if total > config.max_pixels {
        return Err(at!(RawError::LimitExceeded(format!(
            "image {width}x{height} = {total} pixels exceeds limit of {}",
            config.max_pixels
        ))));
    }

    // Build PixelBuffer
    let descriptor = match config.color_profile {
        DtColorProfile::Srgb => PixelDescriptor::RGB8_SRGB,
        _ => PixelDescriptor::RGBF32_LINEAR,
    };

    // For sRGB, convert f32 to u8
    let buf = if matches!(config.color_profile, DtColorProfile::Srgb) {
        let u8_data: Vec<u8> = pixels_f32
            .iter()
            .map(|&v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect();
        PixelBuffer::from_vec(u8_data, width, height, descriptor)
            .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?
    } else {
        let byte_data: Vec<u8> = pixels_f32.iter().flat_map(|&v| v.to_ne_bytes()).collect();
        PixelBuffer::from_vec(byte_data, width, height, descriptor)
            .map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))?
    };

    // Extract camera info from filename (darktable doesn't emit it in PFM)
    let filename = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_dng = ext.eq_ignore_ascii_case("dng");

    Ok(RawDecodeOutput {
        pixels: buf,
        info: RawInfo {
            width,
            height,
            make: String::new(),
            model: filename,
            sensor_width: width,
            sensor_height: height,
            cfa_pattern: String::new(),
            is_dng,
            orientation: 1,
            bit_depth: None, // darktable outputs processed pixels, not raw sensor data
            wb_coeffs: [1.0, 1.0, 1.0, 1.0],
            color_matrix: [[0.0; 3]; 4],
            black_levels: [0.0; 4],
            white_levels: [0.0; 4],
            crop_rect: None,
            active_area: None,
            baseline_exposure: None,
            sensor_layout: crate::decode::SensorLayout::Unknown,
        },
    })
}

/// Decode RAW bytes using darktable-cli (writes to temp file, decodes, cleans up).
pub fn decode_bytes(data: &[u8], config: &DtConfig) -> Result<RawDecodeOutput> {
    // Detect extension from magic bytes
    let ext = detect_extension(data);

    // Use unique-per-invocation temp dir to avoid race conditions
    use std::sync::atomic::{AtomicU64, Ordering};
    static BYTES_COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = BYTES_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let tmp_dir = std::env::temp_dir().join(format!("zenraw_dt_bytes_{pid}_{id}"));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| at!(RawError::Decode(e.to_string())))?;

    let input_path = tmp_dir.join(format!("input.{ext}"));
    std::fs::write(&input_path, data).map_err(|e| at!(RawError::Decode(e.to_string())))?;

    let result = decode_file(&input_path, config);

    // Clean up entire temp directory (best effort)
    let _ = std::fs::remove_dir_all(&tmp_dir);

    result
}

// ── PFM parser ──────────────────────────────────────────────────────────

/// Parse a PFM (Portable Float Map) file.
///
/// Format: "PF\n<width> <height>\n<scale>\n<binary f32 RGB data>"
/// Scale > 0 = big-endian, scale < 0 = little-endian.
fn parse_pfm(data: &[u8]) -> Result<(Vec<f32>, u32, u32)> {
    let mut cursor = std::io::Cursor::new(data);
    let mut header = String::new();

    // Read magic line
    read_line(&mut cursor, &mut header)?;
    let magic = header.trim();
    if magic != "PF" {
        return Err(at!(RawError::InvalidInput(format!(
            "not a color PFM file (magic: {magic:?})"
        ))));
    }

    // Read dimensions
    header.clear();
    read_line(&mut cursor, &mut header)?;
    let dims: Vec<&str> = header.split_whitespace().collect();
    if dims.len() != 2 {
        return Err(at!(RawError::InvalidInput(
            "invalid PFM dimensions line".into()
        )));
    }
    let width: u32 = dims[0]
        .parse()
        .map_err(|_| at!(RawError::InvalidInput("invalid PFM width".into())))?;
    let height: u32 = dims[1]
        .parse()
        .map_err(|_| at!(RawError::InvalidInput("invalid PFM height".into())))?;

    // Read scale
    header.clear();
    read_line(&mut cursor, &mut header)?;
    let scale: f64 = header
        .trim()
        .parse()
        .map_err(|_| at!(RawError::InvalidInput("invalid PFM scale".into())))?;
    let is_little_endian = scale < 0.0;
    let _abs_scale = scale.abs();

    // Read binary pixel data
    let pos = cursor.position() as usize;
    let pixel_data = &data[pos..];

    // Validate dimensions with checked arithmetic BEFORE allocating
    let total = (width as u64)
        .checked_mul(height as u64)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| {
            at!(RawError::LimitExceeded(format!(
                "PFM dimensions overflow: {width}x{height}"
            )))
        })?;
    let expected = total.checked_mul(4).ok_or_else(|| {
        at!(RawError::LimitExceeded(format!(
            "PFM byte count overflow: {width}x{height}x3x4"
        )))
    })?;

    // Reject unreasonably large images before allocation (200M pixels max)
    if total / 3 > 200_000_000 {
        return Err(at!(RawError::LimitExceeded(format!(
            "PFM dimensions {width}x{height} = {} pixels exceeds 200M limit",
            total / 3
        ))));
    }

    let total = total as usize;
    let expected = expected as usize;

    if pixel_data.len() < expected {
        return Err(at!(RawError::InvalidInput(format!(
            "PFM data too short: expected {expected} bytes, got {}",
            pixel_data.len()
        ))));
    }

    // Parse f32 values — PFM stores rows bottom-to-top
    let mut pixels = Vec::with_capacity(total);

    let row_bytes = width as usize * 3 * 4;
    for row in (0..height as usize).rev() {
        let row_start = row * row_bytes;
        for i in 0..width as usize * 3 {
            let offset = row_start + i * 4;
            let bytes = [
                pixel_data[offset],
                pixel_data[offset + 1],
                pixel_data[offset + 2],
                pixel_data[offset + 3],
            ];
            let val = if is_little_endian {
                f32::from_le_bytes(bytes)
            } else {
                f32::from_be_bytes(bytes)
            };
            pixels.push(val);
        }
    }

    Ok((pixels, width, height))
}

/// Read a line from a cursor (until \n).
fn read_line(cursor: &mut std::io::Cursor<&[u8]>, buf: &mut String) -> Result<()> {
    let mut byte = [0u8; 1];
    loop {
        if cursor
            .read(&mut byte)
            .map_err(|e| at!(RawError::Decode(e.to_string())))?
            == 0
        {
            return Err(at!(RawError::InvalidInput(
                "unexpected EOF in PFM header".into()
            )));
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0] as char);
    }
    Ok(())
}

// ── Utilities ───────────────────────────────────────────────────────────

/// Find darktable-cli binary.
fn find_cli(custom: Option<&str>) -> Option<String> {
    if let Some(path) = custom
        && Path::new(path).exists()
    {
        return Some(path.to_string());
    }

    // Search PATH
    if let Ok(output) = Command::new("which").arg("darktable-cli").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(path);
        }
    }

    None
}

/// Detect RAW file extension from magic bytes.
fn detect_extension(data: &[u8]) -> &'static str {
    if data.len() < 12 {
        return "raw";
    }

    // Check for Fuji RAF
    if data.len() >= 8 && &data[..8] == b"FUJIFILM" {
        return "raf";
    }

    // Check for Panasonic RW2
    if data[0] == b'I' && data[1] == b'I' && data[2] == 0x55 && data[3] == 0x00 {
        return "rw2";
    }

    // TIFF-based — check if DNG
    let is_tiff = (data[0] == b'I' && data[1] == b'I' && data[2] == 42 && data[3] == 0)
        || (data[0] == b'M' && data[1] == b'M' && data[2] == 0 && data[3] == 42);

    if is_tiff {
        // Scan for DNGVersion tag (0xC612) in the first 4KB; endianness determines byte order.
        let search_len = data.len().min(4096);
        let haystack = &data[..search_len];
        let needle: &[u8] = if data[0] == b'I' {
            &[0x12, 0xC6]
        } else {
            &[0xC6, 0x12]
        };
        if memchr::memmem::find(haystack, needle).is_some() {
            return "dng";
        }
        // Generic TIFF-based RAW — could be CR2, NEF, ARW, etc.
        // darktable handles them all with .raw or the original extension
        return "tiff";
    }

    "raw"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pfm_round_trip() {
        // Build a minimal 2×2 PFM (little-endian, bottom-to-top)
        let width = 2u32;
        let height = 2u32;
        let mut pfm = Vec::new();
        pfm.extend_from_slice(b"PF\n");
        pfm.extend_from_slice(format!("{width} {height}\n").as_bytes());
        pfm.extend_from_slice(b"-1.0\n"); // negative scale = little-endian

        // Row 0 (bottom in PFM = top in output): R=0.1, G=0.2, B=0.3, ...
        // Row 1 (top in PFM = bottom in output): R=0.7, G=0.8, B=0.9, ...
        let bottom_row: [f32; 6] = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let top_row: [f32; 6] = [0.7, 0.8, 0.9, 1.0, 1.1, 1.2];

        // PFM stores bottom-to-top, so bottom row first in file
        for v in &bottom_row {
            pfm.extend_from_slice(&v.to_le_bytes());
        }
        for v in &top_row {
            pfm.extend_from_slice(&v.to_le_bytes());
        }

        let (pixels, w, h) = parse_pfm(&pfm).unwrap();
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(pixels.len(), 12);

        // After flipping, top row comes first in our output
        assert!((pixels[0] - 0.7).abs() < 1e-6); // top-left R
        assert!((pixels[6] - 0.1).abs() < 1e-6); // bottom-left R
    }

    #[test]
    fn pfm_big_endian() {
        let mut pfm = Vec::new();
        pfm.extend_from_slice(b"PF\n1 1\n1.0\n"); // positive = big-endian
        let val: f32 = 0.42;
        pfm.extend_from_slice(&val.to_be_bytes());
        pfm.extend_from_slice(&val.to_be_bytes());
        pfm.extend_from_slice(&val.to_be_bytes());

        let (pixels, w, h) = parse_pfm(&pfm).unwrap();
        assert_eq!(w, 1);
        assert_eq!(h, 1);
        assert!((pixels[0] - 0.42).abs() < 1e-6);
    }

    #[test]
    fn detect_extension_dng() {
        // TIFF LE header with DNG tag
        let mut data = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
        data.extend_from_slice(&[0x12, 0xC6]); // DNGVersion tag LE
        data.resize(4096, 0);
        assert_eq!(detect_extension(&data), "dng");
    }

    #[test]
    fn detect_extension_raf() {
        let data = b"FUJIFILM0000000000000000";
        assert_eq!(detect_extension(data), "raf");
    }

    #[test]
    fn detect_extension_tiff_raw() {
        let data = [b'M', b'M', 0, 42, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(detect_extension(&data), "tiff");
    }

    #[test]
    fn availability_check() {
        // Just ensure it doesn't panic
        let _ = is_available();
    }
}
