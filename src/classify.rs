//! File format classification for RAW, DNG, and Apple container formats.
//!
//! Detects the specific container format from file header bytes, distinguishing
//! between standard DNG, Apple APPLEDNG (iPhone 15/16 ProRAW), Apple AMPF
//! (iPhone 17 Pro processed JPEG + gain map), and other RAW formats.

/// Detected file format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FileFormat {
    /// Apple APPLEDNG container (iPhone 15/16 Pro ProRAW).
    ///
    /// Big-endian TIFF/DNG with "APPLEDNG" signature at bytes 8-15.
    /// Contains linear raw data (10-bit LJPEG) + embedded camera-rendered
    /// JPEG preview + HDR gain map + semantic sky matte.
    AppleDng,

    /// Apple AMPF container (iPhone 17 Pro "ProRAW").
    ///
    /// JPEG wrapper with AMPF marker. Contains a processed SDR JPEG +
    /// HDR gain map. **NOT raw data** despite `.DNG` extension.
    AppleAmpf,

    /// Standard Adobe DNG (Digital Negative).
    ///
    /// Has DNGVersion tag but not Apple-specific container signature.
    Dng,

    /// TIFF-based RAW format (CR2, NEF, ARW, PEF, ORF, etc.).
    ///
    /// Valid TIFF header but no DNG version tag detected in quick probe.
    TiffRaw,

    /// Canon CR3 (ISO BMFF container).
    Cr3,

    /// Fujifilm RAF.
    Raf,

    /// Panasonic/Leica RW2.
    Rw2,

    /// Olympus ORF (TIFF variant).
    Orf,

    /// Plain JPEG (not AMPF).
    Jpeg,

    /// Unknown or unsupported format.
    #[default]
    Unknown,
}

impl FileFormat {
    /// Whether this format contains decodable raw sensor data.
    #[must_use]
    pub fn is_raw(self) -> bool {
        matches!(
            self,
            Self::AppleDng
                | Self::Dng
                | Self::TiffRaw
                | Self::Cr3
                | Self::Raf
                | Self::Rw2
                | Self::Orf
        )
    }

    /// Whether this is an Apple-specific format.
    #[must_use]
    pub fn is_apple(self) -> bool {
        matches!(self, Self::AppleDng | Self::AppleAmpf)
    }

    /// Whether this format may contain an HDR gain map.
    #[must_use]
    pub fn has_gain_map(self) -> bool {
        matches!(self, Self::AppleDng | Self::AppleAmpf)
    }
}

impl core::fmt::Display for FileFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AppleDng => write!(f, "Apple DNG (ProRAW)"),
            Self::AppleAmpf => write!(f, "Apple AMPF (processed JPEG + gain map)"),
            Self::Dng => write!(f, "DNG"),
            Self::TiffRaw => write!(f, "TIFF-based RAW"),
            Self::Cr3 => write!(f, "Canon CR3"),
            Self::Raf => write!(f, "Fujifilm RAF"),
            Self::Rw2 => write!(f, "Panasonic RW2"),
            Self::Orf => write!(f, "Olympus ORF"),
            Self::Jpeg => write!(f, "JPEG"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Classify a file's format from its header bytes.
///
/// Only needs the first ~64 bytes for most formats, but benefits from
/// more data for DNG version detection (scans first 1KB for DNG tag).
#[must_use]
pub fn classify(data: &[u8]) -> FileFormat {
    if data.len() < 4 {
        return FileFormat::Unknown;
    }

    // JPEG-based formats (check before TIFF since AMPF starts with JPEG SOI)
    if data[0] == 0xFF && data[1] == 0xD8 {
        // Check for AMPF marker in first 64 bytes
        let search_len = 64.min(data.len());
        if data[..search_len].windows(4).any(|w| w == b"AMPF") {
            return FileFormat::AppleAmpf;
        }
        return FileFormat::Jpeg;
    }

    // TIFF-based formats
    let is_tiff_le = data[0] == b'I' && data[1] == b'I' && data[2] == 42 && data[3] == 0;
    let is_tiff_be = data[0] == b'M' && data[1] == b'M' && data[2] == 0 && data[3] == 42;

    if is_tiff_be || is_tiff_le {
        // Check for APPLEDNG signature at bytes 8-15
        if data.len() >= 16 && &data[8..16] == b"APPLEDNG" {
            return FileFormat::AppleDng;
        }

        // Probe for DNG version tag (0xC612) in first IFD
        if has_dng_version_tag(data, is_tiff_be) {
            return FileFormat::Dng;
        }

        return FileFormat::TiffRaw;
    }

    // Olympus ORF (TIFF variant with different magic)
    // Two known variants: "IIRO" (0x52 0x4F) and "IIRS" (0x52 0x53)
    if data[0] == b'I' && data[1] == b'I' && data[2] == 0x52 && (data[3] == 0x4F || data[3] == 0x53)
    {
        return FileFormat::Orf;
    }

    // Fuji RAF
    if data.len() >= 8 && &data[..8] == b"FUJIFILM" {
        return FileFormat::Raf;
    }

    // Panasonic RW2
    if data[0] == b'I' && data[1] == b'I' && data[2] == 0x55 && data[3] == 0x00 {
        return FileFormat::Rw2;
    }

    // Canon CR3 (ISO BMFF with "crx " major brand)
    if data.len() >= 12 && &data[4..8] == b"ftyp" && &data[8..12] == b"crx " {
        return FileFormat::Cr3;
    }

    FileFormat::Unknown
}

/// Check if a TIFF file contains the DNGVersion tag (0xC612) in IFD0.
fn has_dng_version_tag(data: &[u8], big_endian: bool) -> bool {
    if data.len() < 8 {
        return false;
    }

    let read_u16 = if big_endian { read_u16_be } else { read_u16_le };
    let read_u32 = if big_endian { read_u32_be } else { read_u32_le };

    // Get IFD0 offset
    let ifd_offset = read_u32(data, 4) as usize;
    // Attacker-controlled u32 offset; checked_add so it cannot overflow usize
    // on 32-bit targets and wrap past this bound into an OOB read below.
    if ifd_offset.checked_add(2).is_none_or(|end| end > data.len()) {
        return false;
    }

    let entry_count = read_u16(data, ifd_offset) as usize;
    let entries_start = ifd_offset + 2;

    // Each IFD entry is 12 bytes
    for i in 0..entry_count {
        let entry_offset = entries_start + i * 12;
        if entry_offset + 2 > data.len() {
            break;
        }
        let tag = read_u16(data, entry_offset);
        if tag == 0xC612 {
            return true;
        }
        // Tags are sorted in TIFF, so if we pass 0xC612 we can stop
        if tag > 0xC612 {
            break;
        }
    }

    false
}

fn read_u16_be(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_appledng() {
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: APPLEDNG file not found");
            return;
        };
        assert_eq!(classify(&data), FileFormat::AppleDng);
        assert!(classify(&data).is_raw());
        assert!(classify(&data).is_apple());
        assert!(classify(&data).has_gain_map());
    }

    #[test]
    fn classify_ampf() {
        let path = "/mnt/v/heic/IMG_3269.DNG";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: AMPF file not found");
            return;
        };
        assert_eq!(classify(&data), FileFormat::AppleAmpf);
        assert!(!classify(&data).is_raw());
        assert!(classify(&data).is_apple());
        assert!(classify(&data).has_gain_map());
    }

    #[test]
    fn classify_standard_dng() {
        let dir = "/mnt/v/input/fivek/dng/";
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("Skipping: FiveK DNG dir not found");
            return;
        };
        for entry in entries.filter_map(|e| e.ok()).take(1) {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
            {
                let data = std::fs::read(&path).unwrap();
                assert_eq!(classify(&data), FileFormat::Dng, "file: {}", path.display());
                assert!(classify(&data).is_raw());
                assert!(!classify(&data).is_apple());
            }
        }
    }

    #[test]
    fn classify_android_dng() {
        let path = "/mnt/v/heic/android/20260220_093521.dng";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("Skipping: Android DNG not found");
            return;
        };
        let fmt = classify(&data);
        eprintln!("Android DNG classified as: {fmt}");
        assert!(fmt.is_raw());
        assert!(!fmt.is_apple());
    }

    #[test]
    fn classify_jpeg() {
        // JPEG SOI without AMPF
        let data = [0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x10, b'E', b'x', b'i', b'f'];
        assert_eq!(classify(&data), FileFormat::Jpeg);
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(classify(b"hello"), FileFormat::Unknown);
        assert_eq!(classify(&[]), FileFormat::Unknown);
    }

    #[test]
    fn format_display() {
        assert_eq!(FileFormat::AppleDng.to_string(), "Apple DNG (ProRAW)");
        assert_eq!(
            FileFormat::AppleAmpf.to_string(),
            "Apple AMPF (processed JPEG + gain map)"
        );
    }
}
