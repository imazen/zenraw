//! XMP (Extensible Metadata Platform) extraction and generation.
//!
//! Extracts embedded XMP packets from RAW/DNG files by scanning for
//! the standard `<?xpacket begin` / `<?xpacket end` markers. This
//! works across all file formats since XMP packets are self-identifying.
//!
//! Feature-gated behind `xmp`.

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;

/// Raw XMP packet extracted from a file.
#[derive(Clone, Debug)]
pub struct XmpPacket {
    /// The raw XMP XML string (everything between xpacket markers, inclusive).
    pub xml: String,
    /// Byte offset where the packet was found in the file.
    pub offset: usize,
}

/// Extract all XMP packets from file data.
///
/// Scans for `<?xpacket begin` markers and extracts the complete XML
/// including the xpacket processing instructions. Most files contain
/// exactly one XMP packet, but some (like PDFs) may contain multiple.
pub fn extract_xmp_packets(data: &[u8]) -> Vec<XmpPacket> {
    let mut packets = Vec::new();
    let begin_marker = b"<?xpacket begin";
    let end_marker = b"<?xpacket end";

    let mut search_from = 0;
    while search_from < data.len() {
        // Find start marker
        let Some(start) = find_bytes(&data[search_from..], begin_marker) else {
            break;
        };
        let abs_start = search_from + start;

        // Find end marker (must include the closing "?>")
        let Some(end_marker_pos) = find_bytes(&data[abs_start..], end_marker) else {
            break;
        };
        let abs_end_marker = abs_start + end_marker_pos;

        // Find the closing "?>" after the end marker
        let remaining = &data[abs_end_marker..];
        let Some(close_pos) = find_bytes(remaining, b"?>") else {
            break;
        };
        let abs_end = abs_end_marker + close_pos + 2; // include "?>"

        // Extract the XMP XML
        if let Ok(xml) = core::str::from_utf8(&data[abs_start..abs_end]) {
            packets.push(XmpPacket {
                xml: String::from(xml),
                offset: abs_start,
            });
        }

        search_from = abs_end;
    }

    packets
}

/// Extract the first XMP packet as a string, or None if not found.
///
/// Tries `<?xpacket>` markers first, then falls back to `<x:xmpmeta>` blocks
/// (some embedded XMP, like Apple gain map metadata, omits xpacket wrappers).
pub fn extract_xmp(data: &[u8]) -> Option<String> {
    let packets = extract_xmp_packets(data);
    if let Some(first) = packets.into_iter().next() {
        return Some(first.xml);
    }
    // Fallback: look for <x:xmpmeta> ... </x:xmpmeta> blocks
    extract_xmpmeta_block(data)
}

/// Extract XMP from `<x:xmpmeta>` blocks (without xpacket wrappers).
fn extract_xmpmeta_block(data: &[u8]) -> Option<String> {
    let open = b"<x:xmpmeta";
    let close = b"</x:xmpmeta>";

    let start = find_bytes(data, open)?;
    let after_open = start;
    let end_pos = find_bytes(&data[after_open..], close)?;
    let abs_end = after_open + end_pos + close.len();

    core::str::from_utf8(&data[start..abs_end])
        .ok()
        .map(String::from)
}

/// Extract a specific XMP property value by namespace prefix and name.
///
/// This is a simple text-based extraction that works for common simple
/// properties without requiring a full XML parser. For complex XMP
/// structures, use the raw XML from [`extract_xmp`].
///
/// # Example
/// ```ignore
/// let rating = get_xmp_property(&xmp_xml, "xmp", "Rating");
/// let creator = get_xmp_property(&xmp_xml, "dc", "creator");
/// ```
pub fn get_xmp_property(xmp_xml: &str, ns_prefix: &str, name: &str) -> Option<String> {
    // Try attribute form: ns:Name="value"
    let attr_pattern = format!("{ns_prefix}:{name}=\"");
    if let Some(pos) = xmp_xml.find(&attr_pattern) {
        let value_start = pos + attr_pattern.len();
        if let Some(end) = xmp_xml[value_start..].find('"') {
            return Some(String::from(&xmp_xml[value_start..value_start + end]));
        }
    }

    // Try element form: <ns:Name>value</ns:Name>
    let open_tag = format!("<{ns_prefix}:{name}>");
    let close_tag = format!("</{ns_prefix}:{name}>");
    if let Some(open_pos) = xmp_xml.find(&open_tag) {
        let value_start = open_pos + open_tag.len();
        if let Some(close_pos) = xmp_xml[value_start..].find(&close_tag) {
            let value = &xmp_xml[value_start..value_start + close_pos];
            // Trim whitespace and nested tags for simple values
            let trimmed = value.trim();
            if !trimmed.contains('<') {
                return Some(String::from(trimmed));
            }
        }
    }

    None
}

/// Common XMP properties extracted from a RAW/DNG file.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct XmpMetadata {
    /// Raw XMP XML packet.
    pub raw_xml: Option<String>,

    // ── Dublin Core (dc:) ──
    pub creator: Option<String>,
    pub description: Option<String>,
    pub rights: Option<String>,
    pub title: Option<String>,

    // ── XMP Basic (xmp:) ──
    pub rating: Option<i32>,
    pub label: Option<String>,
    pub create_date: Option<String>,
    pub modify_date: Option<String>,
    pub creator_tool: Option<String>,

    // ── Photoshop (photoshop:) ──
    pub color_mode: Option<String>,

    // ── Camera Raw (crs:) ──
    pub white_balance: Option<String>,
    pub temperature: Option<i32>,
    pub tint: Option<i32>,
    pub exposure: Option<String>,
    pub contrast: Option<String>,
    pub shadows: Option<String>,
    pub highlights: Option<String>,
    pub saturation: Option<String>,
    pub sharpness: Option<String>,

    // ── EXIF (exif:) ──
    pub exif_image_width: Option<u32>,
    pub exif_image_height: Option<u32>,

    // ── TIFF (tiff:) ──
    pub tiff_make: Option<String>,
    pub tiff_model: Option<String>,
    pub tiff_orientation: Option<u16>,
}

/// Extract common XMP metadata properties from file data.
pub fn read_xmp_metadata(data: &[u8]) -> Option<XmpMetadata> {
    let xml = extract_xmp(data)?;

    let mut m = XmpMetadata {
        raw_xml: Some(xml.clone()),
        ..Default::default()
    };

    // Dublin Core
    m.creator = get_xmp_property(&xml, "dc", "creator");
    m.description = get_xmp_property(&xml, "dc", "description");
    m.rights = get_xmp_property(&xml, "dc", "rights");
    m.title = get_xmp_property(&xml, "dc", "title");

    // XMP Basic
    m.rating = get_xmp_property(&xml, "xmp", "Rating").and_then(|s| s.parse().ok());
    m.label = get_xmp_property(&xml, "xmp", "Label");
    m.create_date = get_xmp_property(&xml, "xmp", "CreateDate");
    m.modify_date = get_xmp_property(&xml, "xmp", "ModifyDate");
    m.creator_tool = get_xmp_property(&xml, "xmp", "CreatorTool");

    // Photoshop
    m.color_mode = get_xmp_property(&xml, "photoshop", "ColorMode");

    // Camera Raw
    m.white_balance = get_xmp_property(&xml, "crs", "WhiteBalance");
    m.temperature = get_xmp_property(&xml, "crs", "Temperature").and_then(|s| s.parse().ok());
    m.tint = get_xmp_property(&xml, "crs", "Tint").and_then(|s| s.parse().ok());
    m.exposure = get_xmp_property(&xml, "crs", "Exposure2012")
        .or_else(|| get_xmp_property(&xml, "crs", "Exposure"));
    m.contrast = get_xmp_property(&xml, "crs", "Contrast2012")
        .or_else(|| get_xmp_property(&xml, "crs", "Contrast"));
    m.shadows = get_xmp_property(&xml, "crs", "Shadows2012")
        .or_else(|| get_xmp_property(&xml, "crs", "Shadows"));
    m.highlights = get_xmp_property(&xml, "crs", "Highlights2012")
        .or_else(|| get_xmp_property(&xml, "crs", "Highlights"));
    m.saturation = get_xmp_property(&xml, "crs", "Saturation");
    m.sharpness = get_xmp_property(&xml, "crs", "Sharpness");

    // EXIF namespace
    m.exif_image_width =
        get_xmp_property(&xml, "exif", "PixelXDimension").and_then(|s| s.parse().ok());
    m.exif_image_height =
        get_xmp_property(&xml, "exif", "PixelYDimension").and_then(|s| s.parse().ok());

    // TIFF namespace
    m.tiff_make = get_xmp_property(&xml, "tiff", "Make");
    m.tiff_model = get_xmp_property(&xml, "tiff", "Model");
    m.tiff_orientation = get_xmp_property(&xml, "tiff", "Orientation").and_then(|s| s.parse().ok());

    Some(m)
}

// ── Internal helpers ─────────────────────────────────────────────────

/// Find a byte pattern in a slice, returning the offset.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_xmp_from_dng() {
        let dirs = ["/mnt/v/input/fivek/dng/"];
        for dir in &dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.filter_map(|e| e.ok()).take(3) {
                let path = entry.path();
                if !path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
                {
                    continue;
                }

                let data = std::fs::read(&path).unwrap();
                let packets = extract_xmp_packets(&data);
                let name = path.file_name().unwrap().to_str().unwrap();

                eprintln!("{name}: found {} XMP packet(s)", packets.len());
                assert!(!packets.is_empty(), "DNG should contain XMP: {name}");

                for (i, pkt) in packets.iter().enumerate() {
                    eprintln!("  Packet {i}: offset={}, len={}", pkt.offset, pkt.xml.len());
                    assert!(pkt.xml.contains("<?xpacket begin"));
                    assert!(pkt.xml.contains("<?xpacket end"));
                }

                // Test metadata extraction
                let meta = read_xmp_metadata(&data);
                assert!(meta.is_some(), "should parse XMP metadata from {name}");
                let meta = meta.unwrap();
                eprintln!("  tiff:Make = {:?}", meta.tiff_make);
                eprintln!("  tiff:Model = {:?}", meta.tiff_model);
                eprintln!("  xmp:CreatorTool = {:?}", meta.creator_tool);
                eprintln!("  crs:WhiteBalance = {:?}", meta.white_balance);
                eprintln!("  xmp:Rating = {:?}", meta.rating);

                return; // One successful test is enough
            }
        }
        eprintln!("Skipping: no DNG files found for XMP test");
    }

    #[test]
    fn extract_xmp_property_attribute_form() {
        let xmp = r#"<?xpacket begin="..." ?><x:xmpmeta><rdf:RDF>
            <rdf:Description xmp:Rating="5" xmp:Label="Red" tiff:Make="Nikon" />
        </rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;

        assert_eq!(get_xmp_property(xmp, "xmp", "Rating"), Some("5".into()));
        assert_eq!(get_xmp_property(xmp, "xmp", "Label"), Some("Red".into()));
        assert_eq!(get_xmp_property(xmp, "tiff", "Make"), Some("Nikon".into()));
        assert_eq!(get_xmp_property(xmp, "xmp", "Missing"), None);
    }

    #[test]
    fn extract_xmp_property_element_form() {
        let xmp = r#"<?xpacket begin="..." ?>
        <x:xmpmeta>
          <rdf:RDF>
            <rdf:Description>
              <xmp:CreatorTool>Adobe Camera Raw 9.0</xmp:CreatorTool>
              <dc:description>Test photo</dc:description>
            </rdf:Description>
          </rdf:RDF>
        </x:xmpmeta>
        <?xpacket end="w"?>"#;

        assert_eq!(
            get_xmp_property(xmp, "xmp", "CreatorTool"),
            Some("Adobe Camera Raw 9.0".into())
        );
        assert_eq!(
            get_xmp_property(xmp, "dc", "description"),
            Some("Test photo".into())
        );
    }

    #[test]
    fn no_xmp_returns_none() {
        let data = b"This is not a RAW file";
        assert!(extract_xmp(data).is_none());
        assert!(extract_xmp_packets(data).is_empty());
    }

    #[test]
    fn xmp_from_raw_samples() {
        let dir = "/mnt/v/input/raw-samples/";
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("Skipping: raw-samples not found");
            return;
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };

            let packets = extract_xmp_packets(&data);
            if packets.is_empty() {
                eprintln!("{name}: no XMP found");
            } else {
                eprintln!("{name}: {} XMP packet(s)", packets.len());
                if let Some(meta) = read_xmp_metadata(&data) {
                    eprintln!("  Make={:?} Model={:?}", meta.tiff_make, meta.tiff_model);
                }
            }
        }
    }
}
