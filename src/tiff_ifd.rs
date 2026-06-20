//! Minimal TIFF IFD parser for extracting SubIFDs and Apple-specific tags.
//!
//! kamadak-exif only reads IFD0 + EXIF IFD + GPS IFD. This module walks
//! the full IFD chain including SubIFDs (tag 330), which is needed for:
//! - Apple APPLEDNG raw tile metadata (SubIFD0)
//! - Apple semantic sky matte data (SubIFD1)
//! - DNG profile tags (ProfileToneCurve, ProfileName, etc.)
//! - Any tag not in IFD0/EXIF/GPS

use alloc::string::String;
use alloc::vec::Vec;

/// Byte order for TIFF data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ByteOrder {
    #[default]
    BigEndian,
    LittleEndian,
}

/// A raw TIFF IFD entry.
#[derive(Clone, Debug)]
pub struct IfdEntry {
    /// Tag identifier.
    pub tag: u16,
    /// TIFF data type (1=BYTE, 2=ASCII, 3=SHORT, 4=LONG, 5=RATIONAL, etc.)
    pub dtype: u16,
    /// Number of values.
    pub count: u32,
    /// Raw 4-byte value/offset field.
    pub value_offset: [u8; 4],
}

/// A parsed TIFF IFD.
#[derive(Clone, Debug)]
pub struct Ifd {
    /// Offset of this IFD in the file.
    pub offset: usize,
    /// IFD entries.
    pub entries: Vec<IfdEntry>,
    /// Offset of the next IFD (0 if none).
    pub next_ifd_offset: u32,
}

/// Parsed TIFF file structure with IFD chain and SubIFDs.
#[derive(Clone, Debug)]
pub struct TiffStructure {
    pub byte_order: ByteOrder,
    /// IFD0 (and any chained IFDs via next-IFD pointer).
    pub ifds: Vec<Ifd>,
    /// SubIFDs referenced by tag 330 from IFD0.
    pub sub_ifds: Vec<Ifd>,
}

// ── TIFF type sizes ──────────────────────────────────────────────────

/// Size in bytes per element for each TIFF data type.
fn tiff_type_size(dtype: u16) -> usize {
    match dtype {
        1 | 6 | 7 => 1, // BYTE, SBYTE, UNDEFINED
        2 => 1,         // ASCII
        3 | 8 => 2,     // SHORT, SSHORT
        4 | 9 => 4,     // LONG, SLONG
        5 | 10 => 8,    // RATIONAL, SRATIONAL
        11 => 4,        // FLOAT
        12 => 8,        // DOUBLE
        16 => 8,        // LONG8 (BigTIFF)
        _ => 0,
    }
}

// ── Tag constants ────────────────────────────────────────────────────

/// Standard TIFF/DNG/Apple tag IDs.
pub mod tags {
    // Standard TIFF
    pub const IMAGE_WIDTH: u16 = 0x0100;
    pub const IMAGE_LENGTH: u16 = 0x0101;
    pub const BITS_PER_SAMPLE: u16 = 0x0102;
    pub const COMPRESSION: u16 = 0x0103;
    pub const PHOTOMETRIC: u16 = 0x0106;
    pub const STRIP_OFFSETS: u16 = 0x0111;
    pub const SAMPLES_PER_PIXEL: u16 = 0x0115;
    pub const ROWS_PER_STRIP: u16 = 0x0116;
    pub const STRIP_BYTE_COUNTS: u16 = 0x0117;
    pub const TILE_WIDTH: u16 = 0x0142;
    pub const TILE_LENGTH: u16 = 0x0143;
    pub const TILE_OFFSETS: u16 = 0x0144;
    pub const TILE_BYTE_COUNTS: u16 = 0x0145;
    pub const SUB_IFDS: u16 = 0x014A;
    pub const EXIF_IFD: u16 = 0x8769;
    pub const GPS_IFD: u16 = 0x8825;
    pub const MAKER_NOTE: u16 = 0x927C;

    // DNG-specific
    pub const DNG_VERSION: u16 = 0xC612;
    pub const DNG_BACKWARD_VERSION: u16 = 0xC613;
    pub const UNIQUE_CAMERA_MODEL: u16 = 0xC614;
    pub const COLOR_MATRIX_1: u16 = 0xC621;
    pub const COLOR_MATRIX_2: u16 = 0xC622;
    pub const ANALOG_BALANCE: u16 = 0xC627;
    pub const AS_SHOT_NEUTRAL: u16 = 0xC628;
    pub const AS_SHOT_WHITE_XY: u16 = 0xC629;
    pub const BASELINE_EXPOSURE: u16 = 0xC62A;
    pub const BASELINE_NOISE: u16 = 0xC62B;
    pub const BASELINE_SHARPNESS: u16 = 0xC62C;
    pub const LINEAR_RESPONSE_LIMIT: u16 = 0xC62E;
    pub const CALIBRATION_ILLUMINANT_1: u16 = 0xC65A;
    pub const CALIBRATION_ILLUMINANT_2: u16 = 0xC65B;
    pub const FORWARD_MATRIX_1: u16 = 0xC714;
    pub const FORWARD_MATRIX_2: u16 = 0xC715;
    pub const PROFILE_HUE_SAT_MAP_ENCODING: u16 = 0xC6F7;
    pub const PROFILE_NAME: u16 = 0xC6F8;
    pub const PROFILE_TONE_CURVE: u16 = 0xC6FC;
    pub const NOISE_PROFILE: u16 = 0xC761;
    pub const DEFAULT_USER_CROP: u16 = 0xC7B5;

    // DNG 1.6+ (ProfileGainTableMap — used by Apple ProRAW for Smart HDR)
    pub const PROFILE_GAIN_TABLE_MAP: u16 = 0xCD2D; // 52525
    pub const PROFILE_GAIN_TABLE_MAP2: u16 = 0xCD40; // 52544, DNG 1.7+

    // Apple-specific (SubIFD tags for semantic segmentation)
    pub const APPLE_UNKNOWN_C7A6: u16 = 0xC7A6;
    pub const APPLE_AUX_TYPE: u16 = 0xCD2E; // 52526 — semantic mask type URN
    /// Custom PhotometricInterpretation value used by Apple for semantic data.
    pub const APPLE_SEMANTIC_PHOTOMETRIC: u16 = 0xCD2F; // 52527

    // PhotometricInterpretation values
    pub const PHOTOMETRIC_LINEAR_RAW: u16 = 34892;
}

// ── Parsing ──────────────────────────────────────────────────────────

impl TiffStructure {
    /// Parse a TIFF file's IFD structure from raw bytes.
    ///
    /// Reads IFD0 and its chain, plus SubIFDs from tag 330.
    /// Does NOT read image data — just the metadata structure.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }

        let byte_order = match (data[0], data[1]) {
            (b'M', b'M') => ByteOrder::BigEndian,
            (b'I', b'I') => ByteOrder::LittleEndian,
            _ => return None,
        };

        let magic = read_u16(data, 2, byte_order);
        if magic != 42 {
            return None;
        }

        let ifd0_offset = read_u32(data, 4, byte_order) as usize;

        let mut ifds = Vec::new();
        let mut sub_ifds = Vec::new();

        // Walk IFD chain
        let mut current_offset = ifd0_offset;
        let mut safety = 0;
        while current_offset != 0 && safety < 20 {
            safety += 1;
            let Some(ifd) = parse_ifd(data, current_offset, byte_order) else {
                break;
            };

            // Extract SubIFD offsets from tag 330
            for entry in &ifd.entries {
                if entry.tag == tags::SUB_IFDS {
                    let offsets = read_long_values(data, entry, byte_order);
                    for off in offsets {
                        if let Some(sub_ifd) = parse_ifd(data, off as usize, byte_order) {
                            sub_ifds.push(sub_ifd);
                        }
                    }
                }
                // Also parse EXIF IFD (tag 0x8769) as a SubIFD
                if entry.tag == tags::EXIF_IFD {
                    let offsets = read_long_values(data, entry, byte_order);
                    for off in offsets {
                        if let Some(exif_ifd) = parse_ifd(data, off as usize, byte_order) {
                            sub_ifds.push(exif_ifd);
                        }
                    }
                }
            }

            let next = ifd.next_ifd_offset;
            ifds.push(ifd);
            current_offset = next as usize;
        }

        Some(TiffStructure {
            byte_order,
            ifds,
            sub_ifds,
        })
    }

    /// Find an entry by tag in IFD0.
    pub fn ifd0_entry(&self, tag: u16) -> Option<&IfdEntry> {
        self.ifds.first()?.entries.iter().find(|e| e.tag == tag)
    }

    /// Find an entry by tag across all IFDs (IFD0 chain + SubIFDs).
    pub fn find_entry(&self, tag: u16) -> Option<(&Ifd, &IfdEntry)> {
        for ifd in &self.ifds {
            if let Some(entry) = ifd.entries.iter().find(|e| e.tag == tag) {
                return Some((ifd, entry));
            }
        }
        for ifd in &self.sub_ifds {
            if let Some(entry) = ifd.entries.iter().find(|e| e.tag == tag) {
                return Some((ifd, entry));
            }
        }
        None
    }

    /// Get all entries with a given tag across all IFDs.
    pub fn find_all_entries(&self, tag: u16) -> Vec<(&Ifd, &IfdEntry)> {
        let mut results = Vec::new();
        for ifd in self.ifds.iter().chain(self.sub_ifds.iter()) {
            for entry in &ifd.entries {
                if entry.tag == tag {
                    results.push((ifd, entry));
                }
            }
        }
        results
    }
}

impl IfdEntry {
    /// Total byte size of this entry's value data.
    ///
    /// Uses checked arithmetic to prevent overflow on 32-bit platforms.
    /// Returns `usize::MAX` on overflow, which causes subsequent bounds
    /// checks to fail safely.
    pub fn data_size(&self) -> usize {
        tiff_type_size(self.dtype).saturating_mul(self.count as usize)
    }

    /// Whether the value fits inline in the 4-byte value/offset field.
    pub fn is_inline(&self) -> bool {
        self.data_size() <= 4
    }
}

// ── Value readers ────────────────────────────────────────────────────

/// Read value data bytes for an IFD entry.
pub fn read_entry_bytes<'a>(
    data: &'a [u8],
    entry: &'a IfdEntry,
    byte_order: ByteOrder,
) -> Option<&'a [u8]> {
    let size = entry.data_size();
    if size == 0 {
        return None;
    }
    if size <= 4 {
        // Inline value
        Some(&entry.value_offset[..size])
    } else {
        // Offset to data
        let offset = u32::from_be_bytes(entry.value_offset) as usize;
        let offset = if byte_order == ByteOrder::LittleEndian {
            u32::from_le_bytes(entry.value_offset) as usize
        } else {
            offset
        };
        // `offset` is an attacker-controlled u32; `offset + size` can overflow
        // usize on 32-bit targets and wrap past the bounds check. checked_add.
        if offset.checked_add(size).is_none_or(|end| end > data.len()) {
            return None;
        }
        Some(&data[offset..offset + size])
    }
}

/// Read a single u32 value from an IFD entry.
pub fn read_u32_value(data: &[u8], entry: &IfdEntry, byte_order: ByteOrder) -> Option<u32> {
    match entry.dtype {
        3 => {
            // SHORT
            let bytes = read_entry_bytes(data, entry, byte_order)?;
            Some(read_u16(bytes, 0, byte_order) as u32)
        }
        4 => {
            // LONG
            let bytes = read_entry_bytes(data, entry, byte_order)?;
            Some(read_u32(bytes, 0, byte_order))
        }
        _ => None,
    }
}

/// Read a u16 value from an IFD entry.
pub fn read_u16_value(data: &[u8], entry: &IfdEntry, byte_order: ByteOrder) -> Option<u16> {
    if entry.dtype != 3 {
        return None;
    }
    let bytes = read_entry_bytes(data, entry, byte_order)?;
    Some(read_u16(bytes, 0, byte_order))
}

/// Read multiple LONG (u32) values from an IFD entry.
pub fn read_long_values(data: &[u8], entry: &IfdEntry, byte_order: ByteOrder) -> Vec<u32> {
    let mut values = Vec::new();
    let bytes = match read_entry_bytes(data, entry, byte_order) {
        Some(b) => b,
        None => return values,
    };
    match entry.dtype {
        3 => {
            // SHORT values — cap iteration count by actual data length
            let max_count = bytes.len() / 2;
            let count = (entry.count as usize).min(max_count);
            for i in 0..count {
                values.push(read_u16(bytes, i * 2, byte_order) as u32);
            }
        }
        4 => {
            // LONG values — cap iteration count by actual data length
            let max_count = bytes.len() / 4;
            let count = (entry.count as usize).min(max_count);
            for i in 0..count {
                values.push(read_u32(bytes, i * 4, byte_order));
            }
        }
        _ => {}
    }
    values
}

/// Read an ASCII string from an IFD entry.
pub fn read_ascii_value(data: &[u8], entry: &IfdEntry, byte_order: ByteOrder) -> Option<String> {
    if entry.dtype != 2 {
        return None;
    }
    let bytes = read_entry_bytes(data, entry, byte_order)?;
    // Strip trailing NUL
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end]).ok().map(String::from)
}

/// Read RATIONAL values as f64 pairs (numerator/denominator).
pub fn read_rational_values(data: &[u8], entry: &IfdEntry, byte_order: ByteOrder) -> Vec<f64> {
    let mut values = Vec::new();
    if entry.dtype != 5 && entry.dtype != 10 {
        return values;
    }
    let bytes = match read_entry_bytes(data, entry, byte_order) {
        Some(b) => b,
        None => return values,
    };
    // Cap iteration count by actual data length (8 bytes per rational)
    let max_count = bytes.len() / 8;
    let count = (entry.count as usize).min(max_count);
    let signed = entry.dtype == 10;
    for i in 0..count {
        let off = i * 8;
        let num = read_u32(bytes, off, byte_order);
        let den = read_u32(bytes, off + 4, byte_order);
        let val = if signed {
            (num as i32) as f64 / (den as i32).max(1) as f64
        } else {
            num as f64 / den.max(1) as f64
        };
        values.push(val);
    }
    values
}

/// Read FLOAT (32-bit) values from an IFD entry.
pub fn read_float_values(data: &[u8], entry: &IfdEntry, byte_order: ByteOrder) -> Vec<f32> {
    let mut values = Vec::new();
    if entry.dtype != 11 {
        return values;
    }
    let bytes = match read_entry_bytes(data, entry, byte_order) {
        Some(b) => b,
        None => return values,
    };
    // Cap iteration count by actual data length (4 bytes per float)
    let max_count = bytes.len() / 4;
    let count = (entry.count as usize).min(max_count);
    for i in 0..count {
        let off = i * 4;
        let bits = read_u32(bytes, off, byte_order);
        values.push(f32::from_bits(bits));
    }
    values
}

/// Read DOUBLE (64-bit) values from an IFD entry.
pub fn read_double_values(data: &[u8], entry: &IfdEntry, byte_order: ByteOrder) -> Vec<f64> {
    let mut values = Vec::new();
    if entry.dtype != 12 {
        return values;
    }
    let bytes = match read_entry_bytes(data, entry, byte_order) {
        Some(b) => b,
        None => return values,
    };
    // Cap iteration count by actual data length (8 bytes per double)
    let max_count = bytes.len() / 8;
    let count = (entry.count as usize).min(max_count);
    for i in 0..count {
        let off = i * 8;
        let hi = read_u32(bytes, off, byte_order) as u64;
        let lo = read_u32(bytes, off + 4, byte_order) as u64;
        let bits = if byte_order == ByteOrder::BigEndian {
            (hi << 32) | lo
        } else {
            (lo << 32) | hi
        };
        values.push(f64::from_bits(bits));
    }
    values
}

// ── Low-level readers ────────────────────────────────────────────────

fn parse_ifd(data: &[u8], offset: usize, byte_order: ByteOrder) -> Option<Ifd> {
    // `offset` is an attacker-controlled u32 (IFD pointer); checked_add so it
    // cannot overflow usize on 32-bit targets and wrap past this bound.
    if offset == 0 || offset.checked_add(2).is_none_or(|end| end > data.len()) {
        return None;
    }

    let entry_count = read_u16(data, offset, byte_order) as usize;
    let entries_start = offset + 2;
    let entries_end = entries_start + entry_count * 12;

    if entries_end + 4 > data.len() {
        return None;
    }

    let mut entries = Vec::with_capacity(entry_count);
    for i in 0..entry_count {
        let e = entries_start + i * 12;
        let tag = read_u16(data, e, byte_order);
        let dtype = read_u16(data, e + 2, byte_order);
        let count = read_u32(data, e + 4, byte_order);
        let mut value_offset = [0u8; 4];
        value_offset.copy_from_slice(&data[e + 8..e + 12]);

        entries.push(IfdEntry {
            tag,
            dtype,
            count,
            value_offset,
        });
    }

    let next_ifd_offset = read_u32(data, entries_end, byte_order);

    Some(Ifd {
        offset,
        entries,
        next_ifd_offset,
    })
}

fn read_u16(data: &[u8], offset: usize, byte_order: ByteOrder) -> u16 {
    match byte_order {
        ByteOrder::BigEndian => u16::from_be_bytes([data[offset], data[offset + 1]]),
        ByteOrder::LittleEndian => u16::from_le_bytes([data[offset], data[offset + 1]]),
    }
}

fn read_u32(data: &[u8], offset: usize, byte_order: ByteOrder) -> u32 {
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
    fn parse_appledng_structure() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };

        let tiff = TiffStructure::parse(&data).expect("should parse TIFF");
        assert_eq!(tiff.byte_order, ByteOrder::BigEndian);
        assert!(!tiff.ifds.is_empty(), "should have IFD0");

        let ifd0 = &tiff.ifds[0];
        eprintln!("IFD0: {} entries", ifd0.entries.len());
        for e in &ifd0.entries {
            eprintln!("  tag=0x{:04X} type={} count={}", e.tag, e.dtype, e.count);
        }

        eprintln!("SubIFDs: {}", tiff.sub_ifds.len());
        for (i, sub) in tiff.sub_ifds.iter().enumerate() {
            eprintln!("  SubIFD{}: {} entries", i, sub.entries.len());
            for e in &sub.entries {
                eprintln!("    tag=0x{:04X} type={} count={}", e.tag, e.dtype, e.count);
            }
        }

        // APPLEDNG should have SubIFDs
        assert!(
            tiff.sub_ifds.len() >= 2,
            "APPLEDNG should have >=2 SubIFDs (raw + semantic matte)"
        );

        // Check for DNG version
        let dng_ver = tiff.ifd0_entry(tags::DNG_VERSION);
        assert!(dng_ver.is_some(), "should have DNGVersion tag");

        // Check for ProfileName
        let profile_name = tiff.ifd0_entry(tags::PROFILE_NAME);
        if let Some(entry) = profile_name {
            let name = read_ascii_value(&data, entry, tiff.byte_order);
            eprintln!("ProfileName: {:?}", name);
        }

        // Check for Apple AuxType in SubIFDs
        let aux_entries = tiff.find_all_entries(tags::APPLE_AUX_TYPE);
        eprintln!("Apple AuxType entries: {}", aux_entries.len());
        for (_ifd, entry) in &aux_entries {
            let val = read_ascii_value(&data, entry, tiff.byte_order);
            eprintln!("  AuxType: {:?}", val);
        }
    }

    #[test]
    fn parse_android_dng_structure() {
        let path = "/mnt/v/heic/android/20260220_093521.dng";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: Android DNG not found");
            return;
        };

        let tiff = TiffStructure::parse(&data).expect("should parse TIFF");
        eprintln!("Android DNG byte order: {:?}", tiff.byte_order);
        eprintln!(
            "IFDs: {}, SubIFDs: {}",
            tiff.ifds.len(),
            tiff.sub_ifds.len()
        );

        // Should have DNG version
        assert!(tiff.ifd0_entry(tags::DNG_VERSION).is_some());
    }

    #[test]
    fn parse_fivek_dng_structure() {
        let dir = "/mnt/v/input/fivek/dng/";
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("Skipping: FiveK DNG dir not found");
            return;
        };
        for entry in entries.filter_map(|e| e.ok()).take(1) {
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
            {
                continue;
            }
            let data = std::fs::read(&path).unwrap();
            let tiff = TiffStructure::parse(&data).expect("should parse TIFF");
            eprintln!(
                "FiveK DNG: byte_order={:?}, IFDs={}, SubIFDs={}",
                tiff.byte_order,
                tiff.ifds.len(),
                tiff.sub_ifds.len()
            );
        }
    }
}
