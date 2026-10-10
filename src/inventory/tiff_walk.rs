//! Crate-agnostic TIFF structure walk for structural inventories.
//!
//! Kept identical in `zentiff` (zenextras) and `zenraw`: change both copies
//! together. The walk parses a TIFF header, every IFD reachable from the IFD
//! chain and from pointer entries, out-of-line values and the extents
//! (strips, tiles, free space, JPEG streams) the directories reference. A
//! crate supplies [`Rules`] saying what its decode path does with each piece;
//! the walk never decodes pixels.
//!
//! TIFF parts can interleave in any order, so a TIFF's parts are siblings in
//! file order: header, IFDs (each with one child `Field` per entry),
//! out-of-line values (`Field`, same tag as their entry) and extents.
//! Overlapping parts are split around the parts placed before them, with a
//! note in both details; nothing overlapping is ever emitted.

use alloc::borrow::Cow;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use zencodec::inventory::{
    Disposition, Inventory, InventoryError, Part, PartId, PartKind, PartTag,
};

use super::tags::tag_name;

// ── Limits ─────────────────────────────────────────────────────────────

/// IFDs walked per TIFF. Real files carry a handful; multi-page scans a few
/// thousand.
const MAX_IFDS: usize = 4096;
/// Sub-IFD pointers read from one pointer entry.
const MAX_SUB_POINTERS: usize = 1024;
/// Notes appended to one part's detail before the rest are only counted.
const MAX_NOTES: usize = 4;
/// Pieces one overlapping part may be split into.
const MAX_PIECES: usize = 64;
/// Overlap comparisons per placement; a crafted file with millions of
/// mutually overlapping values stops being split past this.
const MAX_OVERLAP_STEPS: usize = 1 << 22;

// ── Tags the walk itself interprets ────────────────────────────────────

pub(super) const STRIP_OFFSETS: u16 = 273;
pub(super) const STRIP_BYTE_COUNTS: u16 = 279;
pub(super) const FREE_OFFSETS: u16 = 288;
pub(super) const FREE_BYTE_COUNTS: u16 = 289;
pub(super) const TILE_OFFSETS: u16 = 324;
pub(super) const TILE_BYTE_COUNTS: u16 = 325;
pub(super) const SUB_IFDS: u16 = 330;
pub(super) const JPEG_IF_OFFSET: u16 = 513;
pub(super) const JPEG_IF_LENGTH: u16 = 514;
pub(super) const EXIF_IFD: u16 = 34665;
pub(super) const GPS_IFD: u16 = 34853;
pub(super) const INTEROP_IFD: u16 = 40965;

// ── Byte access ────────────────────────────────────────────────────────

pub(super) fn get(d: &[u8], at: u64, n: u64) -> Option<&[u8]> {
    let s = usize::try_from(at).ok()?;
    let e = s.checked_add(usize::try_from(n).ok()?)?;
    d.get(s..e)
}

/// Byte order and offset width.
#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    pub(super) le: bool,
    pub(super) big: bool,
}

impl Layout {
    pub(super) fn u16(self, d: &[u8], at: u64) -> Option<u16> {
        let b: [u8; 2] = get(d, at, 2)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    pub(super) fn u32(self, d: &[u8], at: u64) -> Option<u32> {
        let b: [u8; 4] = get(d, at, 4)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    pub(super) fn u64(self, d: &[u8], at: u64) -> Option<u64> {
        let b: [u8; 8] = get(d, at, 8)?.try_into().ok()?;
        Some(if self.le {
            u64::from_le_bytes(b)
        } else {
            u64::from_be_bytes(b)
        })
    }

    /// An offset field: 4 bytes, 8 in BigTIFF.
    pub(super) fn offset(self, d: &[u8], at: u64) -> Option<u64> {
        if self.big {
            self.u64(d, at)
        } else {
            self.u32(d, at).map(u64::from)
        }
    }

    pub(super) fn header_len(self) -> u64 {
        if self.big { 16 } else { 8 }
    }

    pub(super) fn count_len(self) -> u64 {
        if self.big { 8 } else { 2 }
    }

    pub(super) fn entry_len(self) -> u64 {
        if self.big { 20 } else { 12 }
    }

    /// Bytes a value may occupy inside its entry.
    pub(super) fn inline_cap(self) -> u64 {
        if self.big { 8 } else { 4 }
    }
}

/// Size of one value of a TIFF field type, `None` for a type TIFF does not
/// define.
pub(super) fn type_size(typ: u16) -> Option<u64> {
    Some(match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return None,
    })
}

pub(super) fn type_name(typ: u16) -> Cow<'static, str> {
    Cow::Borrowed(match typ {
        1 => "BYTE",
        2 => "ASCII",
        3 => "SHORT",
        4 => "LONG",
        5 => "RATIONAL",
        6 => "SBYTE",
        7 => "UNDEFINED",
        8 => "SSHORT",
        9 => "SLONG",
        10 => "SRATIONAL",
        11 => "FLOAT",
        12 => "DOUBLE",
        13 => "IFD",
        16 => "LONG8",
        17 => "SLONG8",
        18 => "IFD8",
        other => return Cow::Owned(format!("type {other}")),
    })
}

// ── Parsed structure ───────────────────────────────────────────────────

/// Which directory an IFD is, by how the walk reached it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// The n-th IFD of the main chain (page n).
    Page(u32),
    /// Reached through a SubIFDs-style pointer, or chained after one.
    Sub,
    /// The EXIF IFD (34665), or chained after it.
    Exif,
    /// The GPS IFD (34853), or chained after it.
    Gps,
    /// The Interoperability IFD (40965), or chained after it.
    Interop,
    /// A maker-note IFD inside a MakerNote (37500) value, walked by
    /// [`walk_ifd`] with the vendor's offset base.
    #[allow(dead_code)] // not every crate walks maker notes
    MakerNote,
}

/// One IFD entry.
#[derive(Clone, Debug)]
pub(super) struct Entry {
    /// Absolute offset of the entry.
    pub(super) at: u64,
    pub(super) tag: u16,
    pub(super) typ: u16,
    pub(super) count: u64,
    /// Absolute offset of the 4-byte (BigTIFF: 8-byte) value-or-offset field.
    pub(super) field: u64,
    /// Remarks the walk attached to this entry.
    pub(super) notes: Vec<String>,
}

/// Where an entry's value lives.
pub(super) enum Loc {
    /// The field type is not one TIFF defines.
    UnknownType,
    /// `count * size` overflows, or the offset does.
    Overflow,
    /// No value bytes.
    Empty,
    /// Inside the entry.
    Inline,
    /// Out of line: absolute offset, byte count.
    At(u64, u64),
}

/// One directory.
#[derive(Debug)]
pub(super) struct Ifd {
    pub(super) at: u64,
    /// End of the bytes it covers, clipped to the TIFF's limit.
    pub(super) end: u64,
    pub(super) kind: Kind,
    pub(super) name: String,
    /// Whether the decode path reads this directory.
    pub(super) followed: bool,
    pub(super) entries: Vec<Entry>,
    /// Raw next-IFD pointer (relative to the TIFF's base).
    pub(super) next: u64,
    /// Why the directory could not be read whole.
    pub(super) problem: Option<String>,
    /// Remarks about its next pointer.
    pub(super) notes: Vec<String>,
    /// The IFD and entry whose pointer led here (`None` for the IFD chain).
    pub(super) parent: Option<(usize, usize)>,
}

/// One parsed TIFF.
pub(super) struct Walk<'a> {
    /// The input up to the TIFF's limit; positions are absolute.
    pub(super) d: &'a [u8],
    /// Offsets stored in the TIFF are relative to this.
    pub(super) base: u64,
    pub(super) lay: Layout,
    /// The TIFF version number from the header (42, 43, or a raw variant).
    pub(super) magic: u16,
    pub(super) header: Range<u64>,
    /// Whether the header parsed completely.
    pub(super) header_ok: bool,
    /// Bytes after the header that belong to the file format (CR2), with
    /// their disposition and detail.
    pub(super) header_ext: Option<(Range<u64>, Disposition, String)>,
    /// Why the header or IFD0 could not be read.
    pub(super) fatal: Option<String>,
    pub(super) ifds: Vec<Ifd>,
}

impl Walk<'_> {
    pub(super) fn limit(&self) -> u64 {
        self.d.len() as u64
    }

    /// The absolute position of a TIFF-relative offset.
    pub(super) fn abs(&self, off: u64) -> Option<u64> {
        self.base.checked_add(off)
    }

    pub(super) fn loc(&self, e: &Entry) -> Loc {
        let Some(size) = type_size(e.typ) else {
            return Loc::UnknownType;
        };
        let Some(total) = size.checked_mul(e.count) else {
            return Loc::Overflow;
        };
        if total == 0 {
            Loc::Empty
        } else if total <= self.lay.inline_cap() {
            Loc::Inline
        } else {
            match self.lay.offset(self.d, e.field).and_then(|o| self.abs(o)) {
                Some(off) => Loc::At(off, total),
                None => Loc::Overflow,
            }
        }
    }

    /// The value's bytes, when they lie inside the TIFF.
    pub(super) fn bytes(&self, e: &Entry) -> Option<&[u8]> {
        match self.loc(e) {
            Loc::Inline => {
                let total = type_size(e.typ)?.checked_mul(e.count)?;
                get(self.d, e.field, total)
            }
            Loc::At(off, total) => get(self.d, off, total),
            Loc::Empty => Some(&[]),
            _ => None,
        }
    }

    /// Unsigned integer values (BYTE, UNDEFINED, SHORT, LONG, IFD, LONG8,
    /// IFD8), at most `max` of them; other types yield nothing.
    pub(super) fn uints(&self, e: &Entry, max: usize) -> Vec<u64> {
        let mut out = Vec::new();
        let size = match e.typ {
            1 | 7 => 1,
            3 => 2,
            4 | 13 => 4,
            16 | 18 => 8,
            _ => return out,
        };
        let Some(bytes) = self.bytes(e) else {
            return out;
        };
        for chunk in bytes.chunks_exact(size).take(max) {
            let v = match size {
                1 => u64::from(chunk[0]),
                2 => u64::from(self.lay.u16(chunk, 0).unwrap_or(0)),
                4 => u64::from(self.lay.u32(chunk, 0).unwrap_or(0)),
                _ => self.lay.u64(chunk, 0).unwrap_or(0),
            };
            out.push(v);
        }
        out
    }

    /// The single unsigned value of a one-element integer entry.
    #[allow(dead_code)] // not every crate's rules need it
    pub(super) fn single_uint(&self, e: &Entry) -> Option<u64> {
        if e.count != 1 {
            return None;
        }
        self.uints(e, 1).first().copied()
    }

    /// The last entry with `tag` (decoders keeping a map keep the last of
    /// duplicate tags).
    pub(super) fn find<'i>(&self, ifd: &'i Ifd, tag: u16) -> Option<&'i Entry> {
        ifd.entries.iter().rev().find(|e| e.tag == tag)
    }
}

/// Parse the directory at absolute offset `at`. `None` when `at` is outside
/// the TIFF.
fn parse_ifd(
    d: &[u8],
    lay: Layout,
    at: u64,
    kind: Kind,
    name: String,
    followed: bool,
) -> Option<Ifd> {
    let len = d.len() as u64;
    if at >= len {
        return None;
    }
    let mut ifd = Ifd {
        at,
        end: len,
        kind,
        name,
        followed,
        entries: Vec::new(),
        next: 0,
        problem: None,
        notes: Vec::new(),
        parent: None,
    };
    let count = if lay.big {
        lay.u64(d, at)
    } else {
        lay.u16(d, at).map(u64::from)
    };
    let Some(count) = count else {
        ifd.problem = Some("entry count runs past the end of the data".into());
        return Some(ifd);
    };
    let declared_end = count
        .checked_mul(lay.entry_len())
        .and_then(|n| n.checked_add(at))
        .and_then(|n| n.checked_add(lay.count_len()))
        .and_then(|n| n.checked_add(lay.inline_cap()));
    let mut e_at = at + lay.count_len();
    let mut i = 0u64;
    while i < count {
        let Some(e_end) = e_at.checked_add(lay.entry_len()) else {
            break;
        };
        if e_end > len {
            break;
        }
        let (Some(tag), Some(typ)) = (lay.u16(d, e_at), lay.u16(d, e_at + 2)) else {
            break;
        };
        let (count_v, field) = if lay.big {
            (lay.u64(d, e_at + 4), e_at + 12)
        } else {
            (lay.u32(d, e_at + 4).map(u64::from), e_at + 8)
        };
        let Some(count_v) = count_v else { break };
        ifd.entries.push(Entry {
            at: e_at,
            tag,
            typ,
            count: count_v,
            field,
            notes: Vec::new(),
        });
        e_at = e_end;
        i += 1;
    }
    match declared_end {
        Some(end) if end <= len => {
            ifd.end = end;
            ifd.next = lay.offset(d, end - lay.inline_cap()).unwrap_or(0);
        }
        _ => {
            ifd.problem = Some(format!(
                "declares {count} entries but runs past the end of the data"
            ));
        }
    }
    Some(ifd)
}

// ── What a decode path does ────────────────────────────────────────────

/// A disposition with an optional remark for the detail.
pub(super) struct Fate {
    pub(super) d: Disposition,
    pub(super) note: Option<Cow<'static, str>>,
}

impl Fate {
    pub(super) fn new(d: Disposition) -> Self {
        Self { d, note: None }
    }

    pub(super) fn because(d: Disposition, note: impl Into<Cow<'static, str>>) -> Self {
        Self {
            d,
            note: Some(note.into()),
        }
    }
}

/// How long the extents of an [`ExtentRule`] are.
#[allow(dead_code)] // not every crate's rules use every variant
pub(super) enum Count {
    /// Byte counts in this tag, one per offset.
    Tag(u16),
    /// One extent from the first offset to the end of the TIFF.
    ToEnd,
    /// One extent of this many bytes from the first offset.
    Bytes(u64),
}

/// An offset tag with the lengths of the extents it locates.
pub(super) struct ExtentRule {
    pub(super) offsets: u16,
    pub(super) counts: Count,
    pub(super) what: &'static str,
    pub(super) kind: PartKind,
    pub(super) fate: Fate,
}

/// What one decode path does with a TIFF's directories, entries and extents.
pub(super) trait Rules {
    /// Accept a TIFF version number other than 42 and 43 (ORF, RW2).
    fn magic(&self, _magic: u16) -> bool {
        false
    }

    /// Bytes after the classic 8-byte header that the file format defines
    /// (CR2), and what the decode path does with them.
    fn header_extension(&self, _w: &Walk<'_>) -> Option<(u64, Fate)> {
        None
    }

    /// For a value the decode path consumes only in part: how many leading
    /// bytes it uses, and the fate of the rest (an ASCII value's bytes after
    /// its NUL, for example).
    fn value_tail(&self, _w: &Walk<'_>, _ifd: &Ifd, _index: usize) -> Option<(u64, Fate)> {
        None
    }

    /// The bytes each of `chunks` strips (or tiles) holds when the decoder
    /// reads them exactly, so a longer declared count leaves a tail it never
    /// reads. `None` when that cannot be told cheaply (compressed data).
    fn chunk_sizes(&self, w: &Walk<'_>, ifd: &Ifd, tiles: bool, chunks: usize) -> Option<Vec<u64>> {
        uncompressed_sizes(w, ifd, tiles, chunks)
    }

    /// The directory pointer entry `e` in a `kind` IFD leads to.
    fn pointer(&self, kind: Kind, e: &Entry) -> Option<(Kind, &'static str)> {
        match (e.tag, kind) {
            (SUB_IFDS, Kind::Page(_) | Kind::Sub) => Some((Kind::Sub, "SubIFD")),
            (EXIF_IFD, Kind::Page(_) | Kind::Sub) => Some((Kind::Exif, "EXIF IFD")),
            (GPS_IFD, Kind::Page(_) | Kind::Sub) => Some((Kind::Gps, "GPS IFD")),
            (INTEROP_IFD, Kind::Exif) => Some((Kind::Interop, "Interop IFD")),
            _ => None,
        }
    }

    /// The TIFF-relative offsets a pointer entry holds.
    fn pointer_offsets(&self, w: &Walk<'_>, e: &Entry) -> Vec<u64> {
        w.uints(e, MAX_SUB_POINTERS)
    }

    /// Whether the decode path reads the directory pointer entry `e` of
    /// `parent` leads to.
    fn follows(&self, w: &Walk<'_>, parent: &Ifd, e: &Entry) -> bool;

    /// Whether the decode path reads the directory `ifd`'s next pointer
    /// leads to.
    fn follows_next(&self, ifd: &Ifd) -> bool;

    fn ifd(&self, w: &Walk<'_>, ifd: &Ifd) -> Fate;

    /// The entry (and, unless [`value`](Rules::value) says otherwise, its
    /// out-of-line value).
    fn entry(&self, w: &Walk<'_>, ifd: &Ifd, index: usize) -> Fate;

    /// The entry's out-of-line value.
    fn value(&self, w: &Walk<'_>, ifd: &Ifd, index: usize) -> Fate {
        self.entry(w, ifd, index)
    }

    /// Strips and tiles.
    fn image_data(&self, w: &Walk<'_>, ifd: &Ifd) -> Fate;

    /// A JPEG stream located by JPEGInterchangeFormat (513/514).
    fn jpeg_stream(&self, w: &Walk<'_>, ifd: &Ifd) -> Fate;

    /// Extents `ifd` locates. The default covers strips, tiles, free space
    /// and JPEGInterchangeFormat streams of image IFDs.
    fn extents(&self, w: &Walk<'_>, ifd: &Ifd) -> Vec<ExtentRule> {
        if !matches!(ifd.kind, Kind::Page(_) | Kind::Sub) {
            return Vec::new();
        }
        alloc::vec![
            ExtentRule {
                offsets: STRIP_OFFSETS,
                counts: Count::Tag(STRIP_BYTE_COUNTS),
                what: "strip",
                kind: PartKind::Extent,
                fate: self.image_data(w, ifd),
            },
            ExtentRule {
                offsets: TILE_OFFSETS,
                counts: Count::Tag(TILE_BYTE_COUNTS),
                what: "tile",
                kind: PartKind::Extent,
                fate: self.image_data(w, ifd),
            },
            ExtentRule {
                offsets: FREE_OFFSETS,
                counts: Count::Tag(FREE_BYTE_COUNTS),
                what: "free space",
                kind: PartKind::Extent,
                fate: Fate::because(Disposition::Padding, "declared free space"),
            },
            ExtentRule {
                offsets: JPEG_IF_OFFSET,
                counts: Count::Tag(JPEG_IF_LENGTH),
                what: "JPEG stream",
                kind: PartKind::EmbeddedImage,
                fate: self.jpeg_stream(w, ifd),
            },
        ]
    }
}

/// The bytes each uncompressed (Compression 1) strip or tile holds, from
/// the IFD's layout tags; `None` for compressed or subsampled data.
pub(super) fn uncompressed_sizes(
    w: &Walk<'_>,
    ifd: &Ifd,
    tiles: bool,
    chunks: usize,
) -> Option<Vec<u64>> {
    let one = |tag: u16| {
        w.find(ifd, tag)
            .and_then(|e| w.uints(e, 1).first().copied())
    };
    if one(259).unwrap_or(1) != 1 || one(262) == Some(6) {
        return None;
    }
    let width = one(256)?;
    let height = one(257)?;
    let spp = one(277).unwrap_or(1).max(1);
    let bps = w
        .find(ifd, 258)
        .map(|e| w.uints(e, 64))
        .unwrap_or_else(|| alloc::vec![1]);
    let planar = one(284) == Some(2);
    let bits = if planar {
        *bps.first()?
    } else if bps.len() as u64 == spp {
        bps.iter().try_fold(0u64, |a, &b| a.checked_add(b))?
    } else {
        bps.first()?.checked_mul(spp)?
    };
    let row_bytes = |px: u64| px.checked_mul(bits).map(|b| b.div_ceil(8));
    if tiles {
        let per = row_bytes(one(322)?)?.checked_mul(one(323)?)?;
        return Some(alloc::vec![per; chunks]);
    }
    let rps = one(278).unwrap_or(height).clamp(1, height.max(1));
    let per_plane = height.div_ceil(rps).max(1);
    let row = row_bytes(width)?;
    let mut out = Vec::with_capacity(chunks.min(1 << 20));
    for k in 0..chunks as u64 {
        let rows = rps.min(height.saturating_sub((k % per_plane) * rps));
        out.push(row.checked_mul(rows)?);
    }
    Some(out)
}

/// Parse the TIFF whose header is at `base`, with every position below
/// `limit`, following pointers the way `rules` describes.
pub(super) fn walk<'a>(data: &'a [u8], base: u64, limit: u64, rules: &dyn Rules) -> Walk<'a> {
    let limit = limit.min(data.len() as u64);
    let d = &data[..usize::try_from(limit).unwrap_or(data.len())];
    let mut w = Walk {
        d,
        base,
        lay: Layout {
            le: true,
            big: false,
        },
        magic: 0,
        header: base..base,
        header_ok: false,
        header_ext: None,
        fatal: None,
        ifds: Vec::new(),
    };
    let rest = limit.saturating_sub(base);
    let lay = match get(d, base, 2) {
        Some(b"II") => Layout {
            le: true,
            big: false,
        },
        Some(b"MM") => Layout {
            le: false,
            big: false,
        },
        _ => {
            w.header = base..base + rest.min(2);
            w.fatal = Some("not a TIFF byte-order mark".into());
            return w;
        }
    };
    let (lay, ifd0) = match lay.u16(d, base + 2) {
        Some(43) => {
            let big = Layout { big: true, ..lay };
            // Bytesize of offsets (8) and a reserved zero.
            if big.u16(d, base + 4).is_some_and(|v| v != 8)
                || big.u16(d, base + 6).is_some_and(|v| v != 0)
            {
                w.header = base..base + rest.min(8);
                w.fatal = Some("BigTIFF header with an offset size other than 8".into());
                return w;
            }
            w.magic = 43;
            (big, big.u64(d, base + 8))
        }
        Some(m) if m == 42 || rules.magic(m) => {
            w.magic = m;
            (lay, lay.u32(d, base + 4).map(u64::from))
        }
        Some(m) => {
            w.header = base..base + rest.min(4);
            w.fatal = Some(format!("not a TIFF version number ({m})"));
            return w;
        }
        None => {
            w.header = base..limit;
            w.fatal = Some("truncated TIFF header".into());
            return w;
        }
    };
    w.lay = lay;
    let Some(ifd0) = ifd0 else {
        w.header = base..limit;
        w.fatal = Some("truncated TIFF header".into());
        return w;
    };
    w.header = base..base + lay.header_len();
    w.header_ok = true;
    if let Some((extra, fate)) = rules.header_extension(&w)
        && w.header.end + extra <= limit
    {
        let r = w.header.end..w.header.end + extra;
        let detail = fate.note.map(|n| n.into_owned()).unwrap_or_default();
        w.header_ext = Some((r, fate.d, detail));
    }

    let first = w.abs(ifd0);
    run_queue(&mut w, first, Kind::Page(0), "IFD0".into(), rules);
    w
}

/// Walk the directories starting at the absolute offset `at`, which has no
/// TIFF header of its own (a maker-note IFD): offsets inside are relative to
/// `base`, and `lay` gives the byte order. Every position stays below
/// `limit`.
#[allow(clippy::too_many_arguments, dead_code)] // not every crate walks maker notes
pub(super) fn walk_ifd<'a>(
    data: &'a [u8],
    base: u64,
    limit: u64,
    lay: Layout,
    at: u64,
    kind: Kind,
    name: String,
    rules: &dyn Rules,
) -> Walk<'a> {
    let limit = limit.min(data.len() as u64);
    let d = &data[..usize::try_from(limit).unwrap_or(data.len())];
    let mut w = Walk {
        d,
        base,
        lay,
        magic: 0,
        header: at..at,
        header_ok: true,
        header_ext: None,
        fatal: None,
        ifds: Vec::new(),
    };
    run_queue(&mut w, Some(at), kind, name, rules);
    w
}

/// Where a pending directory's pointer came from.
#[derive(Clone, Copy)]
enum From {
    Header,
    Entry(usize, usize),
    Next(usize),
}

struct Pending {
    at: Option<u64>,
    kind: Kind,
    name: String,
    followed: bool,
    from: From,
}

/// Walk every directory reachable from the first one, breadth first.
fn run_queue(w: &mut Walk<'_>, first: Option<u64>, kind: Kind, name: String, rules: &dyn Rules) {
    let d = w.d;
    let lay = w.lay;
    let mut queue = VecDeque::new();
    queue.push_back(Pending {
        at: first,
        kind,
        name,
        followed: true,
        from: From::Header,
    });
    let mut visited = BTreeSet::new();
    while let Some(p) = queue.pop_front() {
        let note = |w: &mut Walk<'_>, text: String| match p.from {
            From::Header => w.fatal = Some(text),
            From::Entry(i, e) => w.ifds[i].entries[e].notes.push(text),
            From::Next(i) => w.ifds[i].notes.push(text),
        };
        let Some(at) = p.at else {
            note(w, format!("{} offset overflows", p.name));
            continue;
        };
        if w.ifds.len() >= MAX_IFDS {
            note(w, format!("not walked: more than {MAX_IFDS} IFDs"));
            continue;
        }
        if !visited.insert(at) {
            note(
                w,
                format!(
                    "{} at {at} was already walked (cycle or shared IFD)",
                    p.name
                ),
            );
            continue;
        }
        let Some(mut ifd) = parse_ifd(d, lay, at, p.kind, p.name.clone(), p.followed) else {
            note(
                w,
                format!("{} offset {at} is past the end of the data", p.name),
            );
            continue;
        };
        if let From::Entry(i, e) = p.from {
            ifd.parent = Some((i, e));
        }
        if p.kind == Kind::Page(0) && ifd.problem.is_some() {
            w.fatal = Some(format!(
                "IFD0 {}",
                ifd.problem.as_deref().unwrap_or_default()
            ));
        }
        let idx = w.ifds.len();
        let readable = ifd.problem.is_none();
        let next = ifd.next;
        let followed_next = rules.follows_next(&ifd);
        let kind = ifd.kind;
        let base_name = ifd.name.clone();
        w.ifds.push(ifd);
        if !readable {
            continue;
        }
        for ei in 0..w.ifds[idx].entries.len() {
            let e = &w.ifds[idx].entries[ei];
            let Some((child_kind, label)) = rules.pointer(kind, e) else {
                continue;
            };
            let followed = rules.follows(w, &w.ifds[idx], e);
            let ptrs = rules.pointer_offsets(w, e);
            if ptrs.is_empty() {
                let text = format!("{label} pointer of type {} not read", type_name(e.typ));
                w.ifds[idx].entries[ei].notes.push(text);
            }
            let numbered = ptrs.len() > 1 || child_kind == Kind::Sub;
            for (k, off) in ptrs.into_iter().enumerate() {
                let name = match (numbered, base_name.len() > 48) {
                    (true, false) => format!("{label} {k} of {base_name}"),
                    (true, true) => format!("{label} {k} (nested)"),
                    (false, false) => format!("{label} of {base_name}"),
                    (false, true) => format!("{label} (nested)"),
                };
                queue.push_back(Pending {
                    at: w.abs(off),
                    kind: child_kind,
                    name,
                    followed,
                    from: From::Entry(idx, ei),
                });
            }
        }
        if next != 0 {
            let (kind, name) = match kind {
                Kind::Page(n) => (
                    Kind::Page(n.saturating_add(1)),
                    format!("IFD{}", n.saturating_add(1)),
                ),
                other if base_name.len() > 48 => (other, "IFD chained after a sub-IFD".into()),
                other => (other, format!("IFD chained after {base_name}")),
            };
            queue.push_back(Pending {
                at: w.abs(next),
                kind,
                name,
                followed: followed_next,
                from: From::Next(idx),
            });
        }
    }
}

// ── Parts before overlap resolution ────────────────────────────────────

/// A part before overlap resolution, with its own children.
pub(super) struct Cand {
    pub(super) range: Range<u64>,
    pub(super) kind: PartKind,
    pub(super) tag: PartTag,
    pub(super) label: Option<String>,
    pub(super) disp: Disposition,
    pub(super) detail: String,
    pub(super) children: Vec<Cand>,
    /// For a container whose children account for its contents: the range
    /// they tile; uncovered bytes become `Unreferenced` gaps on emission.
    pub(super) body: Option<Range<u64>>,
    notes: usize,
}

impl Cand {
    pub(super) fn new(
        range: Range<u64>,
        kind: PartKind,
        tag: PartTag,
        disp: Disposition,
        detail: String,
    ) -> Self {
        Self {
            range,
            kind,
            tag,
            label: None,
            disp,
            detail,
            children: Vec::new(),
            body: None,
            notes: 0,
        }
    }

    pub(super) fn note(&mut self, text: &str) {
        self.notes += 1;
        if self.notes <= MAX_NOTES {
            push_note(&mut self.detail, text);
        } else if self.notes == MAX_NOTES + 1 {
            push_note(&mut self.detail, "further overlaps not listed");
        }
    }
}

pub(super) fn push_note(detail: &mut String, text: &str) {
    if !detail.is_empty() {
        detail.push_str("; ");
    }
    detail.push_str(text);
}

fn describe(c: &Cand) -> String {
    let head = c.detail.split(';').next().unwrap_or("");
    format!(
        "{} {}..{} ({head})",
        c.kind.name(),
        c.range.start,
        c.range.end
    )
}

/// Non-overlapping sibling parts. Each insertion is split around the parts
/// already placed.
#[derive(Default)]
pub(super) struct Placed {
    parts: Vec<Cand>,
    /// start → (end, index into `parts`)
    by_start: BTreeMap<u64, (u64, usize)>,
    steps: usize,
}

impl Placed {
    pub(super) fn insert(&mut self, mut c: Cand) {
        if c.range.start >= c.range.end {
            return;
        }
        let mut hits: Vec<(u64, u64, usize)> = Vec::new();
        for (&s, &(e, i)) in self.by_start.range(..c.range.end).rev() {
            self.steps += 1;
            if e <= c.range.start || self.steps > MAX_OVERLAP_STEPS {
                break;
            }
            hits.push((s, e, i));
        }
        if hits.is_empty() {
            let i = self.parts.len();
            self.by_start.insert(c.range.start, (c.range.end, i));
            self.parts.push(c);
            return;
        }
        hits.reverse();
        let desc = describe(&c);
        let mut pieces = Vec::new();
        let mut cursor = c.range.start;
        for &(s, e, i) in &hits {
            if s > cursor {
                pieces.push(cursor..s);
            }
            cursor = cursor.max(e);
            self.parts[i].note(&format!("overlaps {desc}"));
        }
        if cursor < c.range.end {
            pieces.push(cursor..c.range.end);
        }
        let total = pieces.len();
        let mut children = core::mem::take(&mut c.children);
        for (k, r) in pieces.into_iter().enumerate().take(MAX_PIECES) {
            let mut piece = Cand::new(r.clone(), c.kind, c.tag.clone(), c.disp, c.detail.clone());
            piece.label = c.label.clone();
            piece.notes = c.notes;
            // A split container no longer holds its whole body.
            piece.body = None;
            let (inside, rest): (Vec<Cand>, Vec<Cand>) = children
                .into_iter()
                .partition(|ch| ch.range.start >= r.start && ch.range.end <= r.end);
            piece.children = inside;
            children = rest;
            piece.note(&format!(
                "overlaps {} other part(s); piece {} of {total}",
                hits.len(),
                k + 1
            ));
            let i = self.parts.len();
            self.by_start.insert(r.start, (r.end, i));
            self.parts.push(piece);
        }
        if !children.is_empty()
            && let Some(&(_, _, i)) = hits.first()
        {
            self.parts[i].note(&format!(
                "{} parts of {desc} lie inside it and are not listed",
                children.len()
            ));
        }
    }

    /// The placed parts in file order.
    pub(super) fn into_sorted(self) -> Vec<Cand> {
        let mut parts = self.parts;
        parts.sort_by_key(|c| c.range.start);
        parts
    }
}

/// A placed TIFF.
pub(super) struct Placement {
    /// Parts in file order.
    pub(super) parts: Vec<Cand>,
    /// Furthest byte any part or claim reaches, clipped to the TIFF.
    pub(super) logical_end: u64,
    /// Whether IFD0 was read whole.
    pub(super) ifd0_ok: bool,
}

/// Turn a walk into non-overlapping parts. `first` are placed right after
/// the header, ahead of the TIFF's own parts.
pub(super) fn place(w: &mut Walk<'_>, rules: &dyn Rules, first: Vec<Cand>) -> Placement {
    let limit = w.limit();
    let mut placed = Placed::default();
    let mut logical_end = w.header.end;
    let ifd0_ok = w
        .ifds
        .first()
        .is_some_and(|i| i.kind == Kind::Page(0) && i.problem.is_none());
    if !ifd0_ok {
        logical_end = limit;
    }
    if w.header.end > w.header.start {
        let mut c = if w.header_ok {
            let what = if w.lay.big {
                "BigTIFF header"
            } else {
                "TIFF header"
            };
            Cand::new(
                w.header.clone(),
                PartKind::Header,
                PartTag::None,
                Disposition::Structure,
                what.into(),
            )
        } else {
            let why = w.fatal.clone().unwrap_or_default();
            Cand::new(
                w.header.clone(),
                PartKind::Header,
                PartTag::None,
                Disposition::Malformed,
                why,
            )
        };
        if w.header_ok
            && let Some(why) = &w.fatal
        {
            c.note(why);
        }
        placed.insert(c);
    }
    if let Some((r, d, detail)) = w.header_ext.clone() {
        placed.insert(Cand::new(r, PartKind::Header, PartTag::None, d, detail));
    }
    for c in first {
        logical_end = logical_end.max(c.range.end);
        placed.insert(c);
    }

    // Values and extents first, so pointer problems land on entries before
    // the entries become parts.
    let mut values: Vec<Cand> = Vec::new();
    for i in 0..w.ifds.len() {
        let ifd = &w.ifds[i];
        if ifd.problem.is_some() {
            continue;
        }
        let mut notes: Vec<(usize, String)> = Vec::new();
        for (ei, e) in ifd.entries.iter().enumerate() {
            let Loc::At(off, size) = w.loc(e) else {
                continue;
            };
            let fate = rules.value(w, ifd, ei);
            let mut detail = format!("{} value, {}", tag_name(ifd.kind, e.tag), ifd.name);
            if let Some(n) = &fate.note {
                push_note(&mut detail, n);
            }
            let end = off.saturating_add(size);
            if off >= limit {
                notes.push((ei, format!("value at {off} is past the end of the data")));
                logical_end = limit;
                continue;
            }
            let mut disp = fate.d;
            if end > limit {
                push_note(&mut detail, "truncated by the end of the data");
                disp = Disposition::Malformed;
                logical_end = limit;
            }
            logical_end = logical_end.max(end.min(limit));
            let mut c = Cand::new(
                off..end.min(limit),
                PartKind::Field,
                PartTag::Code(u32::from(e.tag)),
                disp,
                detail,
            );
            if end <= limit
                && let Some((keep, tail)) = rules.value_tail(w, ifd, ei)
                && keep < size
            {
                let mut t = String::new();
                if let Some(n) = &tail.note {
                    push_note(&mut t, n);
                }
                c.children.push(Cand::new(
                    off + keep..end,
                    PartKind::Field,
                    PartTag::Code(u32::from(e.tag)),
                    tail.d,
                    t,
                ));
            }
            values.push(c);
        }
        for rule in rules.extents(w, ifd) {
            let Some(oe) = w.find(ifd, rule.offsets) else {
                continue;
            };
            let max = usize::try_from(limit / 4 + 1).unwrap_or(usize::MAX);
            let offs = w.uints(oe, max);
            let cnts = match rule.counts {
                Count::Tag(ct) => match w.find(ifd, ct) {
                    Some(ce) => w.uints(ce, max),
                    None => continue,
                },
                Count::ToEnd => offs
                    .first()
                    .and_then(|&o| w.abs(o))
                    .map(|a| alloc::vec![limit.saturating_sub(a)])
                    .unwrap_or_default(),
                Count::Bytes(n) => alloc::vec![n],
            };
            let oi = ifd
                .entries
                .iter()
                .rposition(|e| e.tag == rule.offsets)
                .unwrap_or(0);
            let pixels = rule.fate.d == Disposition::ImageData
                && matches!(rule.offsets, STRIP_OFFSETS | TILE_OFFSETS);
            let sizes = if pixels {
                rules.chunk_sizes(w, ifd, rule.offsets == TILE_OFFSETS, offs.len())
            } else {
                None
            };
            let emit = |r: Range<u64>,
                        first: usize,
                        last: usize,
                        values: &mut Vec<Cand>,
                        notes: &mut Vec<(usize, String)>| {
                let which = if first == last {
                    format!("{} {first}", rule.what)
                } else {
                    format!("{}s {first}..={last}", rule.what)
                };
                if r.start >= limit {
                    notes.push((
                        oi,
                        format!("{which} at {} is past the end of the data", r.start),
                    ));
                    return;
                }
                let mut detail = format!("{}, {which}", ifd.name);
                if let Some(n) = &rule.fate.note {
                    push_note(&mut detail, n);
                }
                if pixels && sizes.is_none() {
                    push_note(
                        &mut detail,
                        "bytes after the end of the coded data are not split out",
                    );
                }
                let mut disp = rule.fate.d;
                if r.end > limit {
                    push_note(&mut detail, "truncated by the end of the data");
                    disp = Disposition::Malformed;
                }
                values.push(Cand::new(
                    r.start..r.end.min(limit),
                    rule.kind,
                    PartTag::Code(u32::from(rule.offsets)),
                    disp,
                    detail,
                ));
            };
            // Merge index-consecutive extents that are also byte-contiguous.
            let mut run: Option<(Range<u64>, usize, usize)> = None;
            for (k, (&o, &n)) in offs.iter().zip(cnts.iter()).enumerate() {
                let Some(start) = w.abs(o) else { continue };
                if n == 0 {
                    continue;
                }
                let end = start.saturating_add(n);
                logical_end = logical_end.max(end.min(limit));
                if end > limit {
                    logical_end = limit;
                }
                // A strip declared longer than the rows the decoder reads: list
                // it alone, with the unread tail as a child.
                let used = sizes.as_ref().and_then(|s| s.get(k).copied());
                if let Some(used) = used
                    && used > 0
                    && used < n
                    && end <= limit
                {
                    if let Some((r, first, last)) = run.take() {
                        emit(r, first, last, &mut values, &mut notes);
                    }
                    emit(start..end, k, k, &mut values, &mut notes);
                    if let Some(c) = values.last_mut() {
                        c.children.push(Cand::new(
                            start + used..end,
                            rule.kind,
                            PartTag::Code(u32::from(rule.offsets)),
                            Disposition::Dropped,
                            format!("after the {used} bytes of rows the decoder reads"),
                        ));
                    }
                    continue;
                }
                run = match run {
                    Some((r, first, last)) if r.end == start && last + 1 == k => {
                        Some((r.start..end, first, k))
                    }
                    other => {
                        if let Some((r, first, last)) = other {
                            emit(r, first, last, &mut values, &mut notes);
                        }
                        Some((start..end, k, k))
                    }
                };
            }
            if let Some((r, first, last)) = run {
                emit(r, first, last, &mut values, &mut notes);
            }
        }
        for (ei, text) in notes {
            if let Some(e) = w.ifds[i].entries.get_mut(ei) {
                e.notes.push(text);
            }
        }
    }

    // Directories with their entries.
    for ifd in &w.ifds {
        let fate = match &ifd.problem {
            Some(p) => Fate::because(Disposition::Malformed, p.clone()),
            None => rules.ifd(w, ifd),
        };
        let mut detail = format!("{}, {} entries", ifd.name, ifd.entries.len());
        if let Some(n) = &fate.note {
            push_note(&mut detail, n);
        }
        for n in &ifd.notes {
            push_note(&mut detail, &format!("next IFD: {n}"));
        }
        let mut c = Cand::new(
            ifd.at..ifd.end,
            PartKind::Ifd,
            PartTag::None,
            fate.d,
            detail,
        );
        logical_end = logical_end.max(ifd.end);
        if ifd.problem.is_some() {
            logical_end = limit;
        }
        for (ei, e) in ifd.entries.iter().enumerate() {
            let fate = if ifd.problem.is_some() {
                Fate::because(Disposition::Malformed, "in an unreadable IFD")
            } else {
                rules.entry(w, ifd, ei)
            };
            let mut detail = format!(
                "{} {}[{}]",
                tag_name(ifd.kind, e.tag),
                type_name(e.typ),
                e.count
            );
            if let Some(n) = &fate.note {
                push_note(&mut detail, n);
            }
            for n in &e.notes {
                push_note(&mut detail, n);
            }
            c.children.push(Cand::new(
                e.at..e.at + w.lay.entry_len(),
                PartKind::Field,
                PartTag::Code(u32::from(e.tag)),
                fate.d,
                detail,
            ));
        }
        placed.insert(c);
    }
    for v in values {
        placed.insert(v);
    }
    Placement {
        parts: placed.into_sorted(),
        logical_end,
        ifd0_ok,
    }
}

// ── Emission ───────────────────────────────────────────────────────────

/// Push `c` (and its children) under `parent`.
pub(super) fn emit(
    inv: &mut Inventory,
    parent: Option<PartId>,
    c: Cand,
) -> Result<(), InventoryError> {
    let body = c
        .body
        .clone()
        .filter(|b| b.start >= c.range.start && b.end <= c.range.end);
    let mut p = Part::new(c.kind, c.tag, c.range, c.disp);
    if let Some(l) = c.label {
        p = p.with_label(l);
    }
    if !c.detail.is_empty() {
        p = p.with_detail(c.detail);
    }
    if let Some(b) = body.clone() {
        p = p.with_body(b);
    }
    let id = inv.push(parent, p)?;
    let mut children = c.children;
    children.sort_by_key(|ch| ch.range.start);
    for ch in children {
        emit(inv, Some(id), ch)?;
    }
    if body.is_some() {
        inv.fill_gaps(Some(id), Disposition::Unreferenced)?;
    }
    Ok(())
}

/// Push top-level `parts` (non-overlapping, in file order) with gaps between
/// them: before `logical_end` a gap is `gap` (a lone odd-offset byte between
/// parts is a word-alignment pad); the caller's trailing fill covers the
/// rest.
pub(super) fn emit_top(
    inv: &mut Inventory,
    data: &[u8],
    parts: Vec<Cand>,
    logical_end: u64,
    gap: (Disposition, Option<String>),
) -> Result<(), InventoryError> {
    let len = data.len() as u64;
    let mut cursor = 0u64;
    let push_gap = |inv: &mut Inventory, r: Range<u64>| -> Result<(), InventoryError> {
        let end = r.end.min(logical_end);
        if r.start >= end {
            return Ok(());
        }
        let mut part = Part::new(PartKind::Gap, PartTag::None, r.start..end, gap.0);
        if let Some(d) = &gap.1 {
            part = part.with_detail(d.clone());
        } else if gap.0 == Disposition::Unreferenced
            && end - r.start == 1
            && r.start % 2 == 1
            && end < logical_end
        {
            let byte = get(data, r.start, 1).map_or(0, |b| b[0]);
            part = Part::new(
                PartKind::Gap,
                PartTag::None,
                r.start..end,
                Disposition::Padding,
            )
            .with_detail(if byte == 0 {
                "word-alignment pad byte"
            } else {
                "word-alignment pad byte (non-zero)"
            });
        }
        inv.push(None, part)?;
        Ok(())
    };
    for c in parts {
        if c.range.start > cursor {
            push_gap(inv, cursor..c.range.start)?;
        }
        cursor = cursor.max(c.range.end);
        emit(inv, None, c)?;
    }
    if cursor < len {
        push_gap(inv, cursor..len)?;
    }
    Ok(())
}
