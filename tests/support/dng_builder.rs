#![allow(dead_code)]

// TIFF field types.
pub const BYTE: u16 = 1;
pub const ASCII: u16 = 2;
pub const SHORT: u16 = 3;
pub const LONG: u16 = 4;

/// One IFD entry, with its value either inline (≤ 4 bytes) or in the heap blob.
pub struct Entry {
    pub tag: u16,
    pub kind: u16,
    pub count: u32,
    /// Raw value bytes, already little-endian.
    pub value: Vec<u8>,
}

pub fn short(tag: u16, v: u16) -> Entry {
    Entry {
        tag,
        kind: SHORT,
        count: 1,
        value: v.to_le_bytes().to_vec(),
    }
}

pub fn long(tag: u16, v: u32) -> Entry {
    Entry {
        tag,
        kind: LONG,
        count: 1,
        value: v.to_le_bytes().to_vec(),
    }
}

pub fn ascii(tag: u16, s: &str) -> Entry {
    let mut value = s.as_bytes().to_vec();
    value.push(0);
    Entry {
        tag,
        kind: ASCII,
        count: value.len() as u32,
        value,
    }
}

pub fn bytes(tag: u16, v: &[u8]) -> Entry {
    Entry {
        tag,
        kind: BYTE,
        count: v.len() as u32,
        value: v.to_vec(),
    }
}

/// Serialise a little-endian, single-IFD TIFF.
///
/// Layout: 8-byte header, IFD at offset 8, then the heap (out-of-line entry
/// values), then `strip` (the pixel data `StripOffsets` points at).
pub fn build_tiff(mut entries: Vec<Entry>, strip: &[u8], strip_offset_tag: u16) -> Vec<u8> {
    entries.sort_by_key(|e| e.tag);

    let ifd_start = 8usize;
    let ifd_len = 2 + entries.len() * 12 + 4;
    let heap_start = ifd_start + ifd_len;

    // Lay the heap out first so we know where the strip lands.
    let mut heap = Vec::new();
    let mut placements = Vec::with_capacity(entries.len());
    for e in &entries {
        if e.value.len() <= 4 {
            placements.push(None);
        } else {
            if heap.len() % 2 == 1 {
                heap.push(0); // keep values word-aligned
            }
            placements.push(Some(heap_start + heap.len()));
            heap.extend_from_slice(&e.value);
        }
    }
    let strip_offset = (heap_start + heap.len()) as u32;

    let mut out = Vec::new();
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&(ifd_start as u32).to_le_bytes());

    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (e, place) in entries.iter().zip(&placements) {
        out.extend_from_slice(&e.tag.to_le_bytes());
        out.extend_from_slice(&e.kind.to_le_bytes());
        out.extend_from_slice(&e.count.to_le_bytes());
        let field: [u8; 4] = match place {
            Some(off) => (*off as u32).to_le_bytes(),
            None if e.tag == strip_offset_tag => strip_offset.to_le_bytes(),
            None => {
                let mut f = [0u8; 4];
                f[..e.value.len()].copy_from_slice(&e.value);
                f
            }
        };
        out.extend_from_slice(&field);
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // no next IFD

    assert_eq!(out.len(), heap_start, "IFD length miscomputed");
    out.extend_from_slice(&heap);
    assert_eq!(out.len(), strip_offset as usize, "strip offset miscomputed");
    out.extend_from_slice(strip);
    out
}
