//! Apple-specific metadata extraction for APPLEDNG and AMPF files.
//!
//! Parses Apple MakerNote, HDR gain map metadata, semantic segmentation
//! mattes, and other Apple-proprietary structures embedded in DNG/JPEG.
//!
//! Feature-gated behind `apple`.

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

use crate::tiff_ifd::{
    self, ByteOrder, TiffStructure, read_ascii_value, read_float_values, read_rational_values,
    read_u16_value, read_u32_value, tags,
};

// ── Apple MakerNote ──────────────────────────────────────────────────

/// Known Apple MakerNote tag IDs.
///
/// These are embedded inside the MakerNote blob, which has its own
/// IFD structure prefixed by `Apple iOS\0` + version + byte order.
pub mod makernote_tags {
    pub const MAKERNOTE_VERSION: u16 = 0x0001;
    pub const AE_STABLE: u16 = 0x0002;
    pub const AE_TARGET: u16 = 0x0003;
    pub const AE_AVERAGE: u16 = 0x0004;
    pub const AF_STABLE: u16 = 0x0005;
    pub const ACCELERATION_VECTOR: u16 = 0x0006;
    pub const HDR_IMAGE_TYPE: u16 = 0x0007;
    pub const BURST_UUID: u16 = 0x000B;
    pub const SEMANTIC_COMPONENTS: u16 = 0x000C;
    pub const MEDIA_GROUP_UUID: u16 = 0x0011;
    pub const IMAGE_CAPTURE_TYPE: u16 = 0x0014;
    pub const IMAGE_UNIQUE_ID: u16 = 0x0015;
    pub const LIVE_PHOTO_ID: u16 = 0x0017;
    pub const OIS_MODE: u16 = 0x0020;
    pub const SIGNAL_TO_NOISE_RATIO: u16 = 0x002D;
    pub const LENS_ID: u16 = 0x002E;
    pub const PHOTO_TRANSCODING_GAIN: u16 = 0x002F;
    pub const AF_PERFORMANCE: u16 = 0x0038;
    pub const AF_MEASURED_DEPTH: u16 = 0x003D;
    pub const AF_CONFIDENCE: u16 = 0x003F;
}

/// A single raw MakerNote tag value.
#[derive(Clone, Debug)]
pub(crate) struct MakerNoteTag {
    pub tag: u16,
    pub dtype: u16,
    pub count: u32,
    /// Raw bytes of the value data.
    pub data: Vec<u8>,
}

/// Parsed Apple MakerNote.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub(crate) struct AppleMakerNote {
    /// MakerNote header version (e.g., 14 for iPhone 16 Pro).
    pub version: u16,
    /// Byte order of the MakerNote IFD.
    pub byte_order: ByteOrder,
    /// All raw tags from the MakerNote (including unknown ones).
    pub tags: Vec<MakerNoteTag>,

    // ── Parsed known fields ──
    pub image_capture_type: Option<i32>,
    pub hdr_image_type: Option<i32>,
    pub signal_to_noise_ratio: Option<f64>,
    pub af_stable: Option<bool>,
    pub ae_stable: Option<bool>,
    pub acceleration_vector: Option<Vec<f64>>,
    pub burst_uuid: Option<String>,
    pub image_unique_id: Option<String>,
    pub live_photo_id: Option<String>,
}

/// Parse Apple MakerNote from raw MakerNote bytes.
///
/// Apple MakerNote format:
/// - `Apple iOS\0` (10 bytes)
/// - Version: u16 (big-endian)
/// - Byte order: `MM` or `II` (2 bytes)
/// - TIFF magic: 42 (u16)
/// - IFD offset from start of byte-order marker (u32)
/// - IFD entries follow standard TIFF IFD format
///
/// All offsets within the MakerNote are relative to the byte-order marker
/// (offset 12 from the start of the MakerNote blob).
pub(crate) fn parse_apple_makernote(makernote_bytes: &[u8]) -> Option<AppleMakerNote> {
    // Check "Apple iOS\0" prefix
    if makernote_bytes.len() < 20 || &makernote_bytes[..10] != b"Apple iOS\0" {
        return None;
    }

    // Version (big-endian u16 at offset 10)
    let version = u16::from_be_bytes([makernote_bytes[10], makernote_bytes[11]]);

    // Byte order at offset 12
    let bo_offset = 12;
    let byte_order = match (makernote_bytes[bo_offset], makernote_bytes[bo_offset + 1]) {
        (b'M', b'M') => ByteOrder::BigEndian,
        (b'I', b'I') => ByteOrder::LittleEndian,
        _ => return None,
    };

    // The IFD data starts at the byte-order marker (offset 12)
    // All offsets in the IFD are relative to this point
    let tiff_data = &makernote_bytes[bo_offset..];

    if tiff_data.len() < 8 {
        return None;
    }

    // Apple MakerNote doesn't use standard TIFF magic (42).
    // The u16 at offset 2 is the entry count or a custom version number.
    // The IFD starts immediately after the byte-order marker.
    //
    // Layout: MM + count(u16) + IFD entries...
    // OR:     MM + magic(u16=0x002A) + ifd_offset(u32) (standard TIFF)
    let maybe_magic = tiff_ifd_read_u16(tiff_data, 2, byte_order);

    let ifd_offset = if maybe_magic == 42 {
        // Standard TIFF structure
        tiff_ifd_read_u32(tiff_data, 4, byte_order) as usize
    } else {
        // Apple custom: bytes 2-3 are the entry count, IFD starts at offset 2
        // (the entry_count is read again below)
        2
    };
    if ifd_offset + 2 > tiff_data.len() {
        return None;
    }

    let entry_count = tiff_ifd_read_u16(tiff_data, ifd_offset, byte_order) as usize;
    let entries_start = ifd_offset + 2;

    let mut result = AppleMakerNote {
        version,
        byte_order,
        ..Default::default()
    };

    for i in 0..entry_count {
        let e = entries_start + i * 12;
        if e + 12 > tiff_data.len() {
            break;
        }

        let tag = tiff_ifd_read_u16(tiff_data, e, byte_order);
        let dtype = tiff_ifd_read_u16(tiff_data, e + 2, byte_order);
        let count = tiff_ifd_read_u32(tiff_data, e + 4, byte_order);

        // Read raw value data
        let type_size = match dtype {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => 1,
        };
        let total_size = type_size * count as usize;
        let value_data = if total_size <= 4 {
            tiff_data[e + 8..e + 12][..total_size.min(4)].to_vec()
        } else {
            let offset = tiff_ifd_read_u32(tiff_data, e + 8, byte_order) as usize;
            if offset + total_size <= tiff_data.len() {
                tiff_data[offset..offset + total_size].to_vec()
            } else {
                Vec::new()
            }
        };

        result.tags.push(MakerNoteTag {
            tag,
            dtype,
            count,
            data: value_data.clone(),
        });

        // Parse known tags
        match tag {
            makernote_tags::IMAGE_CAPTURE_TYPE => {
                result.image_capture_type = read_makernote_i32(&value_data, dtype, byte_order);
            }
            makernote_tags::HDR_IMAGE_TYPE => {
                result.hdr_image_type = read_makernote_i32(&value_data, dtype, byte_order);
            }
            makernote_tags::AF_STABLE => {
                result.af_stable =
                    read_makernote_i32(&value_data, dtype, byte_order).map(|v| v != 0);
            }
            makernote_tags::AE_STABLE => {
                // AE_STABLE is often UNDEFINED (bplist), just check if present
                result.ae_stable = Some(!value_data.is_empty());
            }
            makernote_tags::SIGNAL_TO_NOISE_RATIO
                if dtype == 5 && value_data.len() >= 8 =>
            {
                let num = tiff_ifd_read_u32(&value_data, 0, byte_order);
                let den = tiff_ifd_read_u32(&value_data, 4, byte_order);
                result.signal_to_noise_ratio = Some(num as f64 / den.max(1) as f64);
            }
            makernote_tags::ACCELERATION_VECTOR
                if (dtype == 10 || dtype == 5) && count >= 3 =>
            {
                let mut accel = Vec::new();
                for j in 0..count as usize {
                    let off = j * 8;
                    if off + 8 > value_data.len() {
                        break;
                    }
                    let num = tiff_ifd_read_u32(&value_data, off, byte_order) as i32;
                    let den = tiff_ifd_read_u32(&value_data, off + 4, byte_order) as i32;
                    accel.push(num as f64 / den.max(1) as f64);
                }
                result.acceleration_vector = Some(accel);
            }
            makernote_tags::BURST_UUID
            | makernote_tags::IMAGE_UNIQUE_ID
            | makernote_tags::LIVE_PHOTO_ID
                if dtype == 2 =>
            {
                let s = String::from_utf8_lossy(&value_data)
                    .trim_end_matches('\0')
                    .to_string();
                match tag {
                    makernote_tags::BURST_UUID => result.burst_uuid = Some(s),
                    makernote_tags::IMAGE_UNIQUE_ID => result.image_unique_id = Some(s),
                    makernote_tags::LIVE_PHOTO_ID => result.live_photo_id = Some(s),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    Some(result)
}

fn read_makernote_i32(data: &[u8], dtype: u16, byte_order: ByteOrder) -> Option<i32> {
    match dtype {
        3 => {
            // SHORT
            if data.len() >= 2 {
                Some(tiff_ifd_read_u16(data, 0, byte_order) as i32)
            } else {
                None
            }
        }
        4 => {
            // LONG
            if data.len() >= 4 {
                Some(tiff_ifd_read_u32(data, 0, byte_order) as i32)
            } else {
                None
            }
        }
        9 => {
            // SLONG
            if data.len() >= 4 {
                Some(tiff_ifd_read_u32(data, 0, byte_order) as i32)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ── HDR Gain Map ─────────────────────────────────────────────────────

/// HDR gain map metadata extracted from Apple files.
///
/// Both APPLEDNG and AMPF files can contain an HDR gain map as a second
/// image in an MPF (Multi-Picture Format) structure. The gain map is a
/// grayscale JPEG that encodes per-pixel HDR headroom.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct GainMapInfo {
    /// Raw JPEG bytes of the gain map image.
    pub jpeg_data: Vec<u8>,
    /// Gain map width in pixels.
    pub width: u32,
    /// Gain map height in pixels.
    pub height: u32,

    // ── Apple HDR gain map namespace ──
    /// `HDRGainMap:HDRGainMapHeadroom` — max brightness in stops.
    pub headroom: Option<f64>,
    /// `HDRGainMap:HDRGainMapVersion`.
    pub version: Option<u32>,

    // ── Adobe/ISO 21496 HDR tone map namespace ──
    /// `HDRToneMap:GainMapMax`.
    pub gain_map_max: Option<f64>,
    /// `HDRToneMap:GainMapMin`.
    pub gain_map_min: Option<f64>,
    /// `HDRToneMap:Gamma`.
    pub gamma: Option<f64>,
    /// `HDRToneMap:OffsetHDR`.
    pub offset_hdr: Option<f64>,
    /// `HDRToneMap:OffsetSDR`.
    pub offset_sdr: Option<f64>,
    /// `HDRToneMap:BaseRenditionIsHDR`.
    pub base_rendition_is_hdr: Option<bool>,
    /// `HDRToneMap:BaseHeadroom`.
    pub base_headroom: Option<f64>,
    /// `HDRToneMap:AlternateHeadroom`.
    pub alternate_headroom: Option<f64>,
}

/// Extract the HDR gain map from an Apple AMPF or APPLEDNG file.
///
/// For AMPF: scans the MPF structure in the JPEG to find the second image.
/// For APPLEDNG: extracts the gain map from the embedded preview JPEG's MPF.
pub fn extract_gain_map(data: &[u8]) -> Option<GainMapInfo> {
    if data.len() < 4 {
        return None;
    }

    if data[0] == 0xFF && data[1] == 0xD8 {
        // JPEG-based (AMPF or embedded preview)
        extract_gain_map_from_jpeg(data)
    } else if (data[0] == b'M' && data[1] == b'M') || (data[0] == b'I' && data[1] == b'I') {
        // TIFF-based (APPLEDNG) — extract the preview JPEG first, then get gain map from it
        let preview = crate::exif::extract_dng_preview(data)?;
        extract_gain_map_from_jpeg(&preview)
    } else {
        None
    }
}

/// Extract gain map from a JPEG that contains MPF (Multi-Picture Format) data.
fn extract_gain_map_from_jpeg(jpeg_data: &[u8]) -> Option<GainMapInfo> {
    // Find MPF APP2 marker: FF E2 <len> "MPF\0"
    let mpf_offset = find_mpf_segment(jpeg_data)?;
    let mpf_entries = parse_mpf_entries(jpeg_data, mpf_offset)?;

    // The gain map is typically the second image
    if mpf_entries.len() < 2 {
        return None;
    }

    let (gm_offset, gm_size) = mpf_entries[1];
    if gm_offset + gm_size > jpeg_data.len() {
        return None;
    }

    let gm_jpeg = &jpeg_data[gm_offset..gm_offset + gm_size];

    // Verify it's a valid JPEG
    if gm_jpeg.len() < 2 || gm_jpeg[0] != 0xFF || gm_jpeg[1] != 0xD8 {
        return None;
    }

    // Get dimensions from SOF
    let (width, height) = jpeg_sof_dimensions(gm_jpeg).unwrap_or((0, 0));

    // Extract XMP from gain map JPEG for HDR metadata
    let mut info = GainMapInfo {
        jpeg_data: gm_jpeg.to_vec(),
        width,
        height,
        headroom: None,
        version: None,
        gain_map_max: None,
        gain_map_min: None,
        gamma: None,
        offset_hdr: None,
        offset_sdr: None,
        base_rendition_is_hdr: None,
        base_headroom: None,
        alternate_headroom: None,
    };

    // Parse XMP in the gain map JPEG
    if let Some(xmp) = crate::xmp::extract_xmp(gm_jpeg) {
        parse_gain_map_xmp(&xmp, &mut info);
    }

    Some(info)
}

/// Parse gain map XMP metadata from Apple/Adobe namespaces.
fn parse_gain_map_xmp(xmp: &str, info: &mut GainMapInfo) {
    use crate::xmp::get_xmp_property;

    // Apple HDR Gain Map namespace
    info.headroom =
        get_xmp_property(xmp, "HDRGainMap", "HDRGainMapHeadroom").and_then(|s| s.parse().ok());
    info.version =
        get_xmp_property(xmp, "HDRGainMap", "HDRGainMapVersion").and_then(|s| s.parse().ok());

    // Adobe/ISO 21496 HDR Tone Map namespace
    info.gain_map_max =
        get_xmp_property(xmp, "HDRToneMap", "GainMapMax").and_then(|s| s.parse().ok());
    info.gain_map_min =
        get_xmp_property(xmp, "HDRToneMap", "GainMapMin").and_then(|s| s.parse().ok());
    info.gamma = get_xmp_property(xmp, "HDRToneMap", "Gamma").and_then(|s| s.parse().ok());
    info.offset_hdr = get_xmp_property(xmp, "HDRToneMap", "OffsetHDR").and_then(|s| s.parse().ok());
    info.offset_sdr = get_xmp_property(xmp, "HDRToneMap", "OffsetSDR").and_then(|s| s.parse().ok());
    info.base_rendition_is_hdr = get_xmp_property(xmp, "HDRToneMap", "BaseRenditionIsHDR")
        .map(|s| s == "True" || s == "true" || s == "1");
    info.base_headroom =
        get_xmp_property(xmp, "HDRToneMap", "BaseHeadroom").and_then(|s| s.parse().ok());
    info.alternate_headroom =
        get_xmp_property(xmp, "HDRToneMap", "AlternateHeadroom").and_then(|s| s.parse().ok());
}

// ── Semantic Segmentation Mattes ─────────────────────────────────────

/// Apple semantic segmentation matte (e.g., sky, skin, hair, teeth).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub(crate) struct SemanticMatte {
    /// URN type, e.g. `urn:com:apple:photo:2020:aux:semanticskymatte`.
    pub matte_type: String,
    /// Short type name extracted from URN (e.g., "semanticskymatte").
    pub short_type: String,
    /// Image width.
    pub width: u32,
    /// Image height.
    pub height: u32,
    /// Compression type.
    pub compression: u16,
    /// PhotometricInterpretation.
    pub photometric: u16,
    /// Raw compressed image data.
    pub data: Vec<u8>,
}

/// Extract semantic segmentation mattes from an APPLEDNG file.
///
/// APPLEDNG SubIFDs may contain semantic data with tag 0xCD2E (AppleAuxType)
/// containing a URN like `urn:com:apple:photo:2020:aux:semanticskymatte`.
pub(crate) fn extract_semantic_mattes(file_data: &[u8]) -> Vec<SemanticMatte> {
    let Some(tiff) = TiffStructure::parse(file_data) else {
        return Vec::new();
    };

    let mut mattes = Vec::new();

    for sub_ifd in &tiff.sub_ifds {
        // Look for AppleAuxType tag
        let aux_type_entry = sub_ifd
            .entries
            .iter()
            .find(|e| e.tag == tags::APPLE_AUX_TYPE);
        let Some(aux_entry) = aux_type_entry else {
            continue;
        };

        let Some(aux_type) = read_ascii_value(file_data, aux_entry, tiff.byte_order) else {
            continue;
        };

        // Only process semantic matte entries
        if !aux_type.contains("aux:semantic") {
            continue;
        }

        // Extract short type name from URN
        let short_type = aux_type.rsplit(':').next().unwrap_or(&aux_type).to_string();

        // Get image dimensions
        let width = sub_ifd
            .entries
            .iter()
            .find(|e| e.tag == tags::IMAGE_WIDTH)
            .and_then(|e| read_u32_value(file_data, e, tiff.byte_order))
            .unwrap_or(0);
        let height = sub_ifd
            .entries
            .iter()
            .find(|e| e.tag == tags::IMAGE_LENGTH)
            .and_then(|e| read_u32_value(file_data, e, tiff.byte_order))
            .unwrap_or(0);
        let compression = sub_ifd
            .entries
            .iter()
            .find(|e| e.tag == tags::COMPRESSION)
            .and_then(|e| read_u16_value(file_data, e, tiff.byte_order))
            .unwrap_or(0);
        let photometric = sub_ifd
            .entries
            .iter()
            .find(|e| e.tag == tags::PHOTOMETRIC)
            .and_then(|e| read_u16_value(file_data, e, tiff.byte_order))
            .unwrap_or(0);

        // Get image data (from StripOffsets/StripByteCounts or TileOffsets/TileByteCounts)
        let data = extract_ifd_image_data(file_data, sub_ifd, tiff.byte_order);

        mattes.push(SemanticMatte {
            matte_type: aux_type,
            short_type,
            width,
            height,
            compression,
            photometric,
            data,
        });
    }

    mattes
}

fn extract_ifd_image_data(file_data: &[u8], ifd: &tiff_ifd::Ifd, byte_order: ByteOrder) -> Vec<u8> {
    // Try strip-based first
    let strip_offsets = ifd.entries.iter().find(|e| e.tag == tags::STRIP_OFFSETS);
    let strip_counts = ifd
        .entries
        .iter()
        .find(|e| e.tag == tags::STRIP_BYTE_COUNTS);

    if let (Some(off_entry), Some(cnt_entry)) = (strip_offsets, strip_counts) {
        let offsets = tiff_ifd::read_long_values(file_data, off_entry, byte_order);
        let counts = tiff_ifd::read_long_values(file_data, cnt_entry, byte_order);

        let total: usize = counts.iter().map(|&c| c as usize).sum();
        let mut data = Vec::with_capacity(total);

        for (off, cnt) in offsets.iter().zip(counts.iter()) {
            let start = *off as usize;
            let end = start + *cnt as usize;
            if end <= file_data.len() {
                data.extend_from_slice(&file_data[start..end]);
            }
        }
        return data;
    }

    // Try tile-based
    let tile_offsets = ifd.entries.iter().find(|e| e.tag == tags::TILE_OFFSETS);
    let tile_counts = ifd.entries.iter().find(|e| e.tag == tags::TILE_BYTE_COUNTS);

    if let (Some(off_entry), Some(cnt_entry)) = (tile_offsets, tile_counts) {
        let offsets = tiff_ifd::read_long_values(file_data, off_entry, byte_order);
        let counts = tiff_ifd::read_long_values(file_data, cnt_entry, byte_order);

        let total: usize = counts.iter().map(|&c| c as usize).sum();
        let mut data = Vec::with_capacity(total);

        for (off, cnt) in offsets.iter().zip(counts.iter()) {
            let start = *off as usize;
            let end = start + *cnt as usize;
            if end <= file_data.len() {
                data.extend_from_slice(&file_data[start..end]);
            }
        }
        return data;
    }

    Vec::new()
}

// ── DNG Profile ──────────────────────────────────────────────────────

/// DNG embedded color profile metadata.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct DngProfile {
    /// Profile name (tag 0xC6F8).
    pub name: Option<String>,
    /// Profile tone curve — 257 (x, y) control points (tag 0xC6FC, 514 floats).
    pub tone_curve: Option<Vec<f32>>,
    /// Noise profile coefficients (tag 0xC761).
    pub noise_profile: Option<Vec<f64>>,
    /// Profile HueSatMap encoding (tag 0xC6F7).
    pub hue_sat_map_encoding: Option<f64>,
    /// Default user crop [top, left, bottom, right] as fractions (tag 0xC7B5).
    pub default_user_crop: Option<[f64; 4]>,
    /// Baseline noise (tag 0xC62B).
    pub baseline_noise: Option<f64>,
    /// Baseline sharpness (tag 0xC62C).
    pub baseline_sharpness: Option<f64>,
    /// Linear response limit (tag 0xC62E).
    pub linear_response_limit: Option<f64>,
}

/// Extract DNG profile data from a TIFF/DNG file using our IFD parser.
///
/// These tags are in IFD0 and not always accessible through kamadak-exif
/// (e.g., ProfileToneCurve is 514 FLOAT values).
pub fn extract_dng_profile(data: &[u8]) -> Option<DngProfile> {
    let tiff = TiffStructure::parse(data)?;

    let mut profile = DngProfile::default();

    if let Some(entry) = tiff.ifd0_entry(tags::PROFILE_NAME) {
        profile.name = read_ascii_value(data, entry, tiff.byte_order);
    }

    if let Some(entry) = tiff.ifd0_entry(tags::PROFILE_TONE_CURVE) {
        let floats = read_float_values(data, entry, tiff.byte_order);
        if !floats.is_empty() {
            profile.tone_curve = Some(floats);
        }
    }

    if let Some(entry) = tiff.ifd0_entry(tags::NOISE_PROFILE) {
        let vals = tiff_ifd::read_double_values(data, entry, tiff.byte_order);
        if !vals.is_empty() {
            profile.noise_profile = Some(vals);
        } else {
            // Some DNGs store noise profile as RATIONAL
            let rats = read_rational_values(data, entry, tiff.byte_order);
            if !rats.is_empty() {
                profile.noise_profile = Some(rats);
            }
        }
    }

    if let Some(entry) = tiff.ifd0_entry(tags::PROFILE_HUE_SAT_MAP_ENCODING) {
        let vals = read_rational_values(data, entry, tiff.byte_order);
        if let Some(&v) = vals.first() {
            profile.hue_sat_map_encoding = Some(v);
        }
    }

    if let Some(entry) = tiff.ifd0_entry(tags::DEFAULT_USER_CROP) {
        let vals = read_rational_values(data, entry, tiff.byte_order);
        if vals.len() >= 4 {
            profile.default_user_crop = Some([vals[0], vals[1], vals[2], vals[3]]);
        }
    }

    if let Some(entry) = tiff.ifd0_entry(tags::BASELINE_NOISE) {
        let vals = read_rational_values(data, entry, tiff.byte_order);
        if let Some(&v) = vals.first() {
            profile.baseline_noise = Some(v);
        }
    }

    if let Some(entry) = tiff.ifd0_entry(tags::BASELINE_SHARPNESS) {
        let vals = read_rational_values(data, entry, tiff.byte_order);
        if let Some(&v) = vals.first() {
            profile.baseline_sharpness = Some(v);
        }
    }

    if let Some(entry) = tiff.ifd0_entry(tags::LINEAR_RESPONSE_LIMIT) {
        let vals = read_rational_values(data, entry, tiff.byte_order);
        if let Some(&v) = vals.first() {
            profile.linear_response_limit = Some(v);
        }
    }

    Some(profile)
}

// ── ProfileGainTableMap (DNG 1.6 Smart HDR) ─────────────────────────

/// DNG 1.6 ProfileGainTableMap — spatially-varying, tonally-dependent gain.
///
/// Apple ProRAW uses this to encode Smart HDR: per-pixel exposure adjustments
/// that recover highlights and boost shadows. The map is a 3D lookup table
/// with spatial (V×H grid) and tonal (N weight entries) dimensions.
///
/// The gain is applied to raw pixel values before color rendering.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ProfileGainTableMap {
    /// Grid dimensions (rows, cols).
    pub grid_rows: u32,
    pub grid_cols: u32,
    /// Spacing between grid points in normalized image coords.
    pub spacing_v: f64,
    pub spacing_h: f64,
    /// Origin of grid in normalized image coords.
    pub origin_v: f64,
    pub origin_h: f64,
    /// Number of tonal table entries per grid point.
    pub tonal_points: u32,
    /// Input weight coefficients: [R, G, B, min(RGB), max(RGB)].
    pub input_weights: [f32; 5],
    /// The gain table: `grid_rows × grid_cols × tonal_points` f32 values.
    /// Layout: row-major, then col-major, then tonal index.
    pub table: Vec<f32>,
}

impl ProfileGainTableMap {
    /// Apply the gain map to linear RGB pixel data in-place.
    ///
    /// `width` and `height` are the image dimensions in pixels.
    /// `baseline_exposure` is used to scale the weight input.
    pub fn apply(&self, pixels: &mut [f32], width: u32, height: u32, baseline_exposure: f64) {
        let bl_scale = 2.0f32.powf(baseline_exposure as f32);
        let npix = (width as usize) * (height as usize);
        if pixels.len() < npix * 3 {
            return;
        }

        let max_row = (self.grid_rows - 1) as f32;
        let max_col = (self.grid_cols - 1) as f32;
        let max_tone = (self.tonal_points - 1) as f32;
        let gc = self.grid_cols as usize;
        let tp = self.tonal_points as usize;

        for y in 0..height as usize {
            for x in 0..width as usize {
                let idx = (y * width as usize + x) * 3;
                let r = pixels[idx];
                let g = pixels[idx + 1];
                let b = pixels[idx + 2];

                // Compute weight from linear RGB
                let mn = r.min(g).min(b);
                let mx = r.max(g).max(b);
                let weight = (self.input_weights[0] * r
                    + self.input_weights[1] * g
                    + self.input_weights[2] * b
                    + self.input_weights[3] * mn
                    + self.input_weights[4] * mx)
                    * bl_scale;
                let weight = weight.clamp(0.0, 1.0);

                // Spatial position in grid
                let v_img = y as f32 / height as f32;
                let h_img = x as f32 / width as f32;
                let gy =
                    ((v_img - self.origin_v as f32) / self.spacing_v as f32).clamp(0.0, max_row);
                let gx =
                    ((h_img - self.origin_h as f32) / self.spacing_h as f32).clamp(0.0, max_col);

                // Grid indices for bilinear spatial interpolation
                let gy0 = gy as usize;
                let gy1 = (gy0 + 1).min(self.grid_rows as usize - 1);
                let gx0 = gx as usize;
                let gx1 = (gx0 + 1).min(self.grid_cols as usize - 1);
                let fy = gy - gy0 as f32;
                let fx = gx - gx0 as f32;

                // Tonal index
                let tone = weight * max_tone;
                let t0 = tone as usize;
                let t1 = (t0 + 1).min(tp - 1);
                let ft = tone - t0 as f32;

                // Trilinear interpolation: spatial (2D) × tonal (1D)
                let lookup = |row: usize, col: usize, t: usize| -> f32 {
                    self.table[(row * gc + col) * tp + t]
                };

                let g00 = lookup(gy0, gx0, t0) * (1.0 - ft) + lookup(gy0, gx0, t1) * ft;
                let g01 = lookup(gy0, gx1, t0) * (1.0 - ft) + lookup(gy0, gx1, t1) * ft;
                let g10 = lookup(gy1, gx0, t0) * (1.0 - ft) + lookup(gy1, gx0, t1) * ft;
                let g11 = lookup(gy1, gx1, t0) * (1.0 - ft) + lookup(gy1, gx1, t1) * ft;

                let g0 = g00 * (1.0 - fx) + g01 * fx;
                let g1 = g10 * (1.0 - fx) + g11 * fx;
                let gain = g0 * (1.0 - fy) + g1 * fy;

                pixels[idx] *= gain;
                pixels[idx + 1] *= gain;
                pixels[idx + 2] *= gain;
            }
        }
    }

    /// Get gain statistics (min, max, mean).
    pub fn stats(&self) -> (f32, f32, f32) {
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        let mut sum = 0.0f64;
        for &v in &self.table {
            min = min.min(v);
            max = max.max(v);
            sum += v as f64;
        }
        let mean = if self.table.is_empty() {
            0.0
        } else {
            (sum / self.table.len() as f64) as f32
        };
        (min, max, mean)
    }
}

/// Extract ProfileGainTableMap from an APPLEDNG file.
///
/// The map is stored in SubIFD\[0\] (the raw image SubIFD) as tag 0xCD2D.
/// Format: V1 — big-endian header + f32 gain table.
pub fn extract_profile_gain_table_map(data: &[u8]) -> Option<ProfileGainTableMap> {
    let tiff = TiffStructure::parse(data)?;

    // Find tag 0xCD2D in SubIFDs (it's in the raw image SubIFD, not IFD0)
    let (_, entry) = tiff.find_entry(tags::PROFILE_GAIN_TABLE_MAP)?;

    // The entry is UNDEFINED (type 7) — raw bytes
    let raw = tiff_ifd::read_entry_bytes(data, entry, tiff.byte_order)?;
    if raw.len() < 64 {
        return None;
    }

    // Parse header (always big-endian per DNG spec)
    let grid_rows = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let grid_cols = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
    let spacing_v = f64::from_be_bytes(raw[8..16].try_into().ok()?);
    let spacing_h = f64::from_be_bytes(raw[16..24].try_into().ok()?);
    let origin_v = f64::from_be_bytes(raw[24..32].try_into().ok()?);
    let origin_h = f64::from_be_bytes(raw[32..40].try_into().ok()?);
    let tonal_points = u32::from_be_bytes([raw[40], raw[41], raw[42], raw[43]]);

    // Input weights: 5 × f32
    let mut input_weights = [0.0f32; 5];
    for (i, weight) in input_weights.iter_mut().enumerate() {
        let off = 44 + i * 4;
        *weight = f32::from_be_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]);
    }

    // Gain table: grid_rows × grid_cols × tonal_points × f32
    let table_start = 64;
    let table_len = grid_rows as usize * grid_cols as usize * tonal_points as usize;
    let table_bytes = table_len * 4;

    if table_start + table_bytes > raw.len() {
        return None;
    }

    let mut table = Vec::with_capacity(table_len);
    for i in 0..table_len {
        let off = table_start + i * 4;
        table.push(f32::from_be_bytes([
            raw[off],
            raw[off + 1],
            raw[off + 2],
            raw[off + 3],
        ]));
    }

    Some(ProfileGainTableMap {
        grid_rows,
        grid_cols,
        spacing_v,
        spacing_h,
        origin_v,
        origin_h,
        tonal_points,
        input_weights,
        table,
    })
}

// ── Comprehensive Apple metadata extraction ──────────────────────────

/// Complete Apple-specific metadata extracted from an APPLEDNG or AMPF file.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub(crate) struct AppleMetadata {
    /// Apple MakerNote data.
    pub makernote: Option<AppleMakerNote>,
    /// DNG profile data (APPLEDNG only).
    pub dng_profile: Option<DngProfile>,
    /// ProfileGainTableMap — Smart HDR spatially-varying gain (APPLEDNG only).
    pub gain_table_map: Option<ProfileGainTableMap>,
    /// HDR gain map info (from embedded preview MPF).
    pub gain_map: Option<GainMapInfo>,
    /// Semantic segmentation mattes (APPLEDNG only).
    pub semantic_mattes: Vec<SemanticMatte>,
    /// File format detected.
    pub format: crate::classify::FileFormat,

    // ── Apple XMP namespaces ──
    /// `apdi:AuxiliaryImageType` — type of auxiliary image.
    pub auxiliary_image_type: Option<String>,
    /// `apdi:NativeFormat` — native pixel format.
    pub native_format: Option<String>,
    /// `apdi:StoredFormat` — stored pixel format.
    pub stored_format: Option<String>,
}

/// Extract all Apple-specific metadata from a file.
///
/// Handles both APPLEDNG (TIFF-based) and AMPF (JPEG-based) files.
/// Returns `None` if the file is not an Apple format.
///
/// For AMPF (JPEG) files, MakerNote is extracted via kamadak-exif.
/// For APPLEDNG (TIFF) files, MakerNote is extracted via our own TIFF parser
/// since kamadak-exif's TIFF codepath doesn't always expose it.
pub(crate) fn extract_apple_metadata(data: &[u8]) -> Option<AppleMetadata> {
    let format = crate::classify::classify(data);
    if !format.is_apple() {
        return None;
    }

    let mut meta = AppleMetadata {
        format,
        ..Default::default()
    };

    // Extract MakerNote via our own TIFF parser (more reliable than kamadak-exif
    // for APPLEDNG files where the EXIF IFD is inside a big-endian TIFF)
    {
        let mn_bytes = extract_makernote_bytes(data);
        if let Some(bytes) = mn_bytes {
            meta.makernote = parse_apple_makernote(&bytes);
        }
    }

    match format {
        crate::classify::FileFormat::AppleDng => {
            // DNG profile
            meta.dng_profile = extract_dng_profile(data);
            // ProfileGainTableMap (Smart HDR)
            meta.gain_table_map = extract_profile_gain_table_map(data);
            // Semantic mattes
            meta.semantic_mattes = extract_semantic_mattes(data);
            // Gain map (from embedded preview JPEG)
            meta.gain_map = extract_gain_map(data);
        }
        crate::classify::FileFormat::AppleAmpf => {
            // Gain map (directly from AMPF JPEG)
            meta.gain_map = extract_gain_map(data);
        }
        _ => {}
    }

    // Apple XMP namespaces — scan main file XMP packets
    #[cfg(feature = "xmp")]
    {
        for pkt in crate::xmp::extract_xmp_packets(data) {
            extract_apdi_from_xmp(&pkt.xml, &mut meta);
        }
    }

    // Also check gain map XMP for apdi properties (APPLEDNG stores apdi there)
    #[cfg(feature = "xmp")]
    if let Some(gm) = &meta.gain_map
        && let Some(xmp) = crate::xmp::extract_xmp(&gm.jpeg_data)
    {
        extract_apdi_from_xmp(&xmp, &mut meta);
    }

    Some(meta)
}

/// Extract Apple pixel data info (apdi:) properties from an XMP string.
#[cfg(feature = "xmp")]
fn extract_apdi_from_xmp(xmp: &str, meta: &mut AppleMetadata) {
    if meta.auxiliary_image_type.is_none() {
        meta.auxiliary_image_type = crate::xmp::get_xmp_property(xmp, "apdi", "AuxiliaryImageType");
    }
    if meta.native_format.is_none() {
        meta.native_format = crate::xmp::get_xmp_property(xmp, "apdi", "NativeFormat");
    }
    if meta.stored_format.is_none() {
        meta.stored_format = crate::xmp::get_xmp_property(xmp, "apdi", "StoredFormat");
    }
}

// ── MakerNote byte extraction ────────────────────────────────────────

/// Extract raw MakerNote bytes from a TIFF or JPEG file.
///
/// For TIFF: uses our TIFF IFD parser to walk into the EXIF IFD and find tag 0x927C.
/// For JPEG: uses kamadak-exif.
fn extract_makernote_bytes(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 {
        return None;
    }

    // TIFF-based (APPLEDNG, standard DNG)
    if (data[0] == b'M' && data[1] == b'M') || (data[0] == b'I' && data[1] == b'I') {
        let tiff = TiffStructure::parse(data)?;
        // MakerNote is tag 0x927C in the EXIF IFD (which we parse as a SubIFD)
        if let Some((_, entry)) = tiff.find_entry(tiff_ifd::tags::MAKER_NOTE) {
            let bytes = tiff_ifd::read_entry_bytes(data, entry, tiff.byte_order)?;
            return Some(bytes.to_vec());
        }
        return None;
    }

    // JPEG-based (AMPF)
    #[cfg(feature = "exif")]
    if data[0] == 0xFF && data[1] == 0xD8 {
        let exif_data = exif::Reader::new()
            .read_from_container(&mut std::io::Cursor::new(data))
            .ok()?;
        if let Some(field) = exif_data.get_field(exif::Tag::MakerNote, exif::In::PRIMARY)
            && let exif::Value::Undefined(ref bytes, _) = field.value
        {
            return Some(bytes.clone());
        }
    }

    None
}

// ── MPF (Multi-Picture Format) parsing ───────────────────────────────

/// Find the MPF APP2 segment in a JPEG. Returns offset of the MPF data (after "MPF\0").
fn find_mpf_segment(data: &[u8]) -> Option<usize> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }

    let mut pos = 2;
    while pos + 4 < data.len() {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }

        let marker = data[pos + 1];

        // SOS (Start of Scan) — stop searching
        if marker == 0xDA {
            break;
        }

        // Skip markers without length
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            pos += 2;
            continue;
        }

        if pos + 4 > data.len() {
            break;
        }

        let seg_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;

        // APP2 with "MPF\0" signature
        if marker == 0xE2
            && seg_len >= 6
            && pos + 4 + 4 <= data.len()
            && &data[pos + 4..pos + 8] == b"MPF\0"
        {
            return Some(pos + 8); // Return offset after "MPF\0"
        }

        pos += 2 + seg_len;
    }

    None
}

/// Parse MPF entry table. Returns Vec of (absolute_offset, size) for each image.
fn parse_mpf_entries(jpeg_data: &[u8], mpf_data_offset: usize) -> Option<Vec<(usize, usize)>> {
    let mpf_data = &jpeg_data[mpf_data_offset..];
    if mpf_data.len() < 8 {
        return None;
    }

    // MPF uses its own TIFF-like structure
    let byte_order = match (mpf_data[0], mpf_data[1]) {
        (b'M', b'M') => ByteOrder::BigEndian,
        (b'I', b'I') => ByteOrder::LittleEndian,
        _ => return None,
    };

    // Verify "42" magic at offset 2
    let magic = tiff_ifd_read_u16(mpf_data, 2, byte_order);
    if magic != 42 {
        return None;
    }

    // IFD offset (relative to start of MPF TIFF structure)
    let ifd_offset = tiff_ifd_read_u32(mpf_data, 4, byte_order) as usize;
    if ifd_offset + 2 > mpf_data.len() {
        return None;
    }

    let entry_count = tiff_ifd_read_u16(mpf_data, ifd_offset, byte_order) as usize;
    let entries_start = ifd_offset + 2;

    // Find MP Entry tag (0xB002) — contains image offsets and sizes
    let mut mp_entry_offset = 0usize;
    let mut mp_entry_count = 0u32;

    for i in 0..entry_count {
        let e = entries_start + i * 12;
        if e + 12 > mpf_data.len() {
            break;
        }
        let tag = tiff_ifd_read_u16(mpf_data, e, byte_order);
        if tag == 0xB002 {
            // MP Entry — type 7 (UNDEFINED), each entry is 16 bytes
            mp_entry_count = tiff_ifd_read_u32(mpf_data, e + 4, byte_order);
            let total_size = mp_entry_count as usize;
            if total_size <= 4 {
                mp_entry_offset = e + 8;
            } else {
                mp_entry_offset = tiff_ifd_read_u32(mpf_data, e + 8, byte_order) as usize;
            }
            break;
        }
    }

    if mp_entry_count == 0 {
        return None;
    }

    // Each MP Entry is 16 bytes: attributes(4) + size(4) + offset(4) + dep_img1(2) + dep_img2(2)
    let num_images = mp_entry_count / 16;
    let mut entries = Vec::new();

    for i in 0..num_images as usize {
        let e = mp_entry_offset + i * 16;
        if e + 16 > mpf_data.len() {
            break;
        }

        let size = tiff_ifd_read_u32(mpf_data, e + 4, byte_order) as usize;
        let offset = tiff_ifd_read_u32(mpf_data, e + 8, byte_order) as usize;

        // First image offset is 0 = start of the JPEG file
        // Subsequent offsets are relative to the start of the MPF TIFF header
        let abs_offset = if i == 0 || offset == 0 {
            0
        } else {
            mpf_data_offset + offset
        };

        let actual_size = if i == 0 && size == 0 {
            // First image size=0 means the entire first JPEG
            // We don't know the exact size, so we'll skip it
            0
        } else {
            size
        };

        entries.push((abs_offset, actual_size));
    }

    Some(entries)
}

/// Extract width and height from a JPEG SOF marker.
fn jpeg_sof_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }

    let mut pos = 2;
    while pos + 4 < data.len() {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }

        let marker = data[pos + 1];

        // SOF markers (SOF0-SOF3, SOF5-SOF7, SOF9-SOF11, SOF13-SOF15)
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF)
            && pos + 9 <= data.len()
        {
            let height = u16::from_be_bytes([data[pos + 5], data[pos + 6]]) as u32;
            let width = u16::from_be_bytes([data[pos + 7], data[pos + 8]]) as u32;
            return Some((width, height));
        }

        // SOS — stop searching
        if marker == 0xDA {
            break;
        }

        // Skip markers without length
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            pos += 2;
            continue;
        }

        if pos + 4 > data.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        pos += 2 + seg_len;
    }

    None
}

// ── Local TIFF read helpers ──────────────────────────────────────────
// (Duplicated from tiff_ifd to avoid making those pub(crate))

fn tiff_ifd_read_u16(data: &[u8], offset: usize, byte_order: ByteOrder) -> u16 {
    match byte_order {
        ByteOrder::BigEndian => u16::from_be_bytes([data[offset], data[offset + 1]]),
        ByteOrder::LittleEndian => u16::from_le_bytes([data[offset], data[offset + 1]]),
    }
}

fn tiff_ifd_read_u32(data: &[u8], offset: usize, byte_order: ByteOrder) -> u32 {
    match byte_order {
        ByteOrder::BigEndian => u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]),
        ByteOrder::LittleEndian => u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_appledng_makernote() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let meta = extract_apple_metadata(&data);
        assert!(meta.is_some(), "should extract Apple metadata");
        let meta = meta.unwrap();
        assert_eq!(meta.format, crate::classify::FileFormat::AppleDng);

        if let Some(mn) = &meta.makernote {
            eprintln!("MakerNote version: {}", mn.version);
            eprintln!("MakerNote byte_order: {:?}", mn.byte_order);
            eprintln!("MakerNote tags: {}", mn.tags.len());
            for t in &mn.tags {
                eprintln!(
                    "  tag=0x{:04X} type={} count={} data_len={}",
                    t.tag,
                    t.dtype,
                    t.count,
                    t.data.len()
                );
            }
            eprintln!("image_capture_type: {:?}", mn.image_capture_type);
            eprintln!("hdr_image_type: {:?}", mn.hdr_image_type);
            eprintln!("signal_to_noise_ratio: {:?}", mn.signal_to_noise_ratio);
            eprintln!("af_stable: {:?}", mn.af_stable);
            eprintln!("burst_uuid: {:?}", mn.burst_uuid);
            eprintln!("image_unique_id: {:?}", mn.image_unique_id);
        } else {
            eprintln!("No MakerNote extracted (exif feature may be disabled)");
        }
    }

    #[test]
    fn parse_ampf_makernote() {
        let path = "/mnt/v/heic/IMG_3269.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: AMPF file not found");
            return;
        };

        let meta = extract_apple_metadata(&data);
        assert!(meta.is_some(), "should extract Apple metadata from AMPF");
        let meta = meta.unwrap();
        assert_eq!(meta.format, crate::classify::FileFormat::AppleAmpf);

        if let Some(mn) = &meta.makernote {
            eprintln!("AMPF MakerNote version: {}", mn.version);
            eprintln!("AMPF MakerNote tags: {}", mn.tags.len());
        }
    }

    #[test]
    fn extract_profile_gain_table_map_test() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let pgtm = extract_profile_gain_table_map(&data);
        assert!(pgtm.is_some(), "should extract ProfileGainTableMap");
        let pgtm = pgtm.unwrap();
        eprintln!(
            "PGTM: {}x{} grid, {} tonal points, {} entries",
            pgtm.grid_rows,
            pgtm.grid_cols,
            pgtm.tonal_points,
            pgtm.table.len()
        );
        eprintln!(
            "  spacing: ({:.4}, {:.4}) origin: ({:.4}, {:.4})",
            pgtm.spacing_v, pgtm.spacing_h, pgtm.origin_v, pgtm.origin_h
        );
        eprintln!(
            "  weights: R={:.5} G={:.5} B={:.5} min={:.5} max={:.5}",
            pgtm.input_weights[0],
            pgtm.input_weights[1],
            pgtm.input_weights[2],
            pgtm.input_weights[3],
            pgtm.input_weights[4]
        );
        let (min, max, mean) = pgtm.stats();
        eprintln!("  gains: min={min:.4} max={max:.4} mean={mean:.4}");

        assert!(pgtm.grid_rows > 0);
        assert!(pgtm.grid_cols > 0);
        assert!(pgtm.tonal_points > 0);
        assert_eq!(
            pgtm.table.len(),
            (pgtm.grid_rows * pgtm.grid_cols * pgtm.tonal_points) as usize
        );
    }

    #[test]
    fn extract_appledng_gain_map() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let gm = extract_gain_map(&data);
        if let Some(gm) = &gm {
            eprintln!(
                "APPLEDNG gain map: {}x{}, {} bytes",
                gm.width,
                gm.height,
                gm.jpeg_data.len()
            );
            eprintln!("  headroom: {:?}", gm.headroom);
            eprintln!("  version: {:?}", gm.version);
            eprintln!("  gain_map_max: {:?}", gm.gain_map_max);
            eprintln!("  gamma: {:?}", gm.gamma);
            eprintln!("  offset_hdr: {:?}", gm.offset_hdr);
            eprintln!("  offset_sdr: {:?}", gm.offset_sdr);
        } else {
            eprintln!("No gain map found in APPLEDNG (may need MPF in preview)");
        }
    }

    #[test]
    fn extract_ampf_gain_map() {
        let path = "/mnt/v/heic/IMG_3269.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: AMPF file not found");
            return;
        };

        let gm = extract_gain_map(&data);
        if let Some(gm) = &gm {
            eprintln!(
                "AMPF gain map: {}x{}, {} bytes",
                gm.width,
                gm.height,
                gm.jpeg_data.len()
            );
            eprintln!("  headroom: {:?}", gm.headroom);
            eprintln!("  version: {:?}", gm.version);
            eprintln!("  gain_map_max: {:?}", gm.gain_map_max);
            eprintln!("  gamma: {:?}", gm.gamma);
        } else {
            eprintln!("No gain map found in AMPF");
        }
    }

    #[test]
    fn extract_semantic_mattes_appledng() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let mattes = extract_semantic_mattes(&data);
        eprintln!("Semantic mattes: {}", mattes.len());
        for m in &mattes {
            eprintln!(
                "  type={} ({}), {}x{}, compression={}, photometric={}, data={}B",
                m.short_type,
                m.matte_type,
                m.width,
                m.height,
                m.compression,
                m.photometric,
                m.data.len()
            );
        }
    }

    #[test]
    fn extract_dng_profile_appledng() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let profile = extract_dng_profile(&data);
        if let Some(p) = &profile {
            eprintln!("Profile name: {:?}", p.name);
            if let Some(tc) = &p.tone_curve {
                eprintln!(
                    "Tone curve: {} values (first 10: {:?})",
                    tc.len(),
                    &tc[..10.min(tc.len())]
                );
            }
            eprintln!("Noise profile: {:?}", p.noise_profile);
            eprintln!("Default user crop: {:?}", p.default_user_crop);
            eprintln!("Baseline noise: {:?}", p.baseline_noise);
            eprintln!("Baseline sharpness: {:?}", p.baseline_sharpness);
            eprintln!("Linear response limit: {:?}", p.linear_response_limit);
        } else {
            eprintln!("No DNG profile found");
        }
    }

    #[test]
    fn full_apple_metadata_appledng() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let meta = extract_apple_metadata(&data).expect("should parse");
        eprintln!("Format: {}", meta.format);
        eprintln!("Has MakerNote: {}", meta.makernote.is_some());
        eprintln!("Has DNG profile: {}", meta.dng_profile.is_some());
        eprintln!("Has gain map: {}", meta.gain_map.is_some());
        eprintln!("Semantic mattes: {}", meta.semantic_mattes.len());
        eprintln!("auxiliary_image_type: {:?}", meta.auxiliary_image_type);
        eprintln!("native_format: {:?}", meta.native_format);
    }
}
