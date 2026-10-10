//! Structural inventory of a camera RAW file, for
//! [`DecodeJob::inventory`](zencodec::decode::DecodeJob::inventory).
//!
//! [`tiff_walk`] parses TIFF structure (shared with zentiff). This module
//! adds the RAW containers and supplies [`RawRules`]: what zenraw's zencodec
//! decode path does with each piece.
//!
//! - TIFF-based RAWs (DNG, CR2, NEF, ARW, ORF, RW2, ...): the TIFF walk with
//!   the ORF and RW2 version numbers and the 16-byte CR2 header.
//! - RAF: the header, the JPEG preview (with the EXIF TIFF inside its APP1),
//!   the CFA header records and the CFA section (a TIFF in current models).
//! - CR3 (ISOBMFF): every box, recursing into the container boxes.
//!
//! The zencodec path (`zencodec_impl`) decodes pixels with rawloader (or
//! rawler with the `rawler` feature) and reports only orientation and, with
//! the `xmp` feature, the first XMP packet a byte scan finds. rawloader walks
//! the IFD chain, SubIFDs, the EXIF IFD, Fuji/Kodak sub-IFDs and maker notes,
//! keeping only the tags it knows ([`RAWLOADER_TAGS`]); its camera decoders
//! look those up anywhere in that tree. No EXIF, GPS or other metadata
//! reaches `ImageInfo`, although the decoder advertises `with_exif`.

mod tags;
mod tiff_walk;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use zencodec::ImageFormat;
use zencodec::inventory::{
    Disposition, Inventory, InventoryError, MetadataKind, PartKind, PartTag,
};

use tags::tag_known;
use tiff_walk::{
    Cand, Count, Entry, ExtentRule, Fate, GPS_IFD, Ifd, Kind, Placed, Rules, STRIP_OFFSETS,
    TILE_OFFSETS, Walk, emit_top, get, place, uncompressed_sizes, walk,
};

/// Detail for CR3 parts in builds without rawler.
const CR3_NOT_DECODED: &str =
    "CR3 is decoded only by the `rawler` backend; this build's decode fails";

/// Whether the zencodec path decodes with rawler (`rawler` feature) instead
/// of rawloader. rawler keeps every tag of the IFDs it walks, so those
/// entries are all `Structure` in such builds.
const RAWLER: bool = cfg!(feature = "rawler");

const NEW_SUBFILE_TYPE: u16 = 254;
const COMPRESSION: u16 = 259;
const ORIENTATION: u16 = 274;
const PANA_OFFSETS: u16 = 0x0118;
const XMP: u16 = 700;
const CFA_PATTERN: u16 = 33422;
const MAKER_NOTE: u16 = 37500;
const CR2_ID: u16 = 0xC5D8;
const DNG_VERSION: u16 = 50706;
const RAF_RAW_SUB_IFD: u16 = 0xF000;
const RAF_OFFSETS: u16 = 0xF007;
const RAF_BYTE_COUNTS: u16 = 0xF008;
const KODAK_IFD: u16 = 0x8290;
const KDC_IFD: u16 = 0xFE00;
const DNG_PRIVATE_DATA: u16 = 0xC634;
const SONY_OFFSET: u16 = 0x7200;
const SONY_LENGTH: u16 = 0x7201;
/// Canon's metadata `uuid` in CR3 `moov` (holds CNCV, CCTP, CMT1-4, THMB).
const CANON_UUID: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];

/// rawloader 0.37's `Tag` enum (decoders/tiff.rs): the only tags its TIFF
/// parser keeps. Sorted.
const RAWLOADER_TAGS: &[u16] = &[
    0x0002, 0x0003, 0x000C, 0x0011, 0x0012, 0x0014, 0x001D, 0x0024, 0x0025, 0x0026, 0x0029, 0x0081,
    0x008C, 0x0096, 0x0097, 0x00A4, 0x00A7, 0x00FE, 0x0100, 0x0101, 0x0102, 0x0103, 0x0106, 0x010F,
    0x0110, 0x0111, 0x0112, 0x0115, 0x0117, 0x0118, 0x0123, 0x0131, 0x0142, 0x0143, 0x0144, 0x014A,
    0x0200, 0x0201, 0x0220, 0x02BC, 0x03FD, 0x0600, 0x090D, 0x0E80, 0x0F00, 0x1017, 0x1018, 0x2040,
    0x2FF0, 0x4001, 0x7010, 0x7200, 0x7201, 0x7221, 0x7303, 0x7313, 0x828E, 0x8290, 0x8606, 0x8769,
    0x927C, 0xA010, 0xA021, 0xA028, 0xC5D8, 0xC612, 0xC618, 0xC61A, 0xC61D, 0xC621, 0xC622, 0xC628,
    0xC634, 0xC640, 0xC68D, 0xC68E, 0xF000, 0xF001, 0xF002, 0xF003, 0xF007, 0xF00E, 0xFA2A, 0xFD00,
    0xFD01, 0xFD04, 0xFE00,
];

fn rawloader_knows(tag: u16) -> bool {
    RAWLOADER_TAGS.binary_search(&tag).is_ok()
}

/// Pointer tags rawloader's `TiffIFD::new` follows into sub-IFDs.
fn rawloader_follows(tag: u16) -> bool {
    matches!(tag, 330 | 34665 | RAF_RAW_SUB_IFD | KODAK_IFD | KDC_IFD)
}

/// Which TIFF of the file a walk covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// The file itself (TIFF-based RAWs).
    Main,
    /// The EXIF TIFF inside a RAF's JPEG preview.
    RafJpegExif,
    /// A RAF's CFA section.
    RafCfa,
    /// The TIFF in a CR3 `CMT1`..`CMT4` box (IFD0, EXIF, maker notes, GPS).
    Cr3Cmt(u8),
}

/// zenraw's zencodec decode path for one TIFF of the file.
struct RawRules {
    role: Role,
    /// Absolute offset of the IFD whose strips/tiles rawloader decodes.
    raw_ifd: Option<u64>,
    /// Absolute offset of the IFD whose Orientation entry rawloader reads
    /// (the first in its depth-first order).
    orientation_ifd: Option<u64>,
    /// Whether the build surfaces XMP (`xmp` feature).
    xmp: bool,
    /// Whether the file is a DNG (uncompressed strip sizes are then exact).
    dng: bool,
    /// Offsets of the MakerNote values rawloader parses as IFDs.
    parsed_notes: Vec<u64>,
}

impl RawRules {
    fn unread(&self, kind: Kind, tag: u16, why: &'static str) -> Fate {
        if tag_known(kind, tag) {
            Fate::because(Disposition::Skipped, why)
        } else {
            Fate::because(Disposition::Unknown, why)
        }
    }
}

impl Rules for RawRules {
    fn magic(&self, magic: u16) -> bool {
        // Olympus ORF ("IIRO"/"IIRS"/"MMOR") and Panasonic RW2 ("IIU\0").
        matches!(magic, 0x4F52 | 0x5352 | 0x0055)
    }

    fn header_extension(&self, w: &Walk<'_>) -> Option<(u64, Fate)> {
        if self.role != Role::Main || w.lay.big || get(w.d, w.base + 8, 2) != Some(b"CR") {
            return None;
        }
        let fate = if RAWLER {
            Fate::because(
                Disposition::Structure,
                "CR2 header extension: version and raw IFD offset",
            )
        } else {
            Fate::because(
                Disposition::Skipped,
                "CR2 header extension: version and raw IFD offset; rawloader finds the raw IFD by tag",
            )
        };
        Some((8, fate))
    }

    fn chunk_sizes(&self, w: &Walk<'_>, ifd: &Ifd, tiles: bool, chunks: usize) -> Option<Vec<u64>> {
        // Only DNG's uncompressed layout is standard; vendor RAWs pack rows
        // their own way.
        if self.dng {
            uncompressed_sizes(w, ifd, tiles, chunks)
        } else {
            None
        }
    }

    fn pointer(&self, kind: Kind, e: &Entry) -> Option<(Kind, &'static str)> {
        match (e.tag, kind) {
            (RAF_RAW_SUB_IFD | KODAK_IFD | KDC_IFD, Kind::Page(_) | Kind::Sub) => {
                Some((Kind::Sub, "sub-IFD"))
            }
            // Sony stores a pointer to its SR2Private IFD in DNGPrivateData
            // (a single LONG); DNGs store maker data bytes there instead.
            (DNG_PRIVATE_DATA, Kind::Page(_) | Kind::Sub)
                if (e.count == 1 && matches!(e.typ, 4 | 13))
                    || (e.count == 4 && matches!(e.typ, 1 | 7)) =>
            {
                Some((Kind::Sub, "SR2Private IFD"))
            }
            (330, Kind::Page(_) | Kind::Sub) => Some((Kind::Sub, "SubIFD")),
            (34665, Kind::Page(_) | Kind::Sub) => Some((Kind::Exif, "EXIF IFD")),
            (GPS_IFD, Kind::Page(_) | Kind::Sub) => Some((Kind::Gps, "GPS IFD")),
            (40965, Kind::Exif) => Some((Kind::Interop, "Interop IFD")),
            _ => None,
        }
    }

    fn pointer_offsets(&self, w: &Walk<'_>, e: &Entry) -> Vec<u64> {
        // ARW stores the SR2Private offset as BYTE[4]; rawloader reads it with
        // `get_force_u32` in the file's byte order.
        if e.tag == DNG_PRIVATE_DATA && e.count == 4 && matches!(e.typ, 1 | 7) {
            return w
                .bytes(e)
                .and_then(|b| w.lay.u32(b, 0))
                .map(|v| alloc::vec![u64::from(v)])
                .unwrap_or_default();
        }
        w.uints(e, 1024)
    }

    fn follows(&self, _w: &Walk<'_>, parent: &Ifd, e: &Entry) -> bool {
        // The ARW decoder reads the SR2Private IFD itself (arw.rs
        // `get_wb`), outside rawloader's tree walk.
        parent.followed && (rawloader_follows(e.tag) || e.tag == DNG_PRIVATE_DATA)
    }

    fn follows_next(&self, ifd: &Ifd) -> bool {
        // rawloader's `TiffIFD::new_root` walks the main chain up to 100 IFDs,
        // rawler's `IFD::new_root` up to 10; sub-IFD next pointers are read
        // but never followed.
        let max = if RAWLER { 10 } else { 100 };
        matches!(ifd.kind, Kind::Page(n) if n < max)
    }

    fn ifd(&self, _w: &Walk<'_>, ifd: &Ifd) -> Fate {
        if matches!(self.role, Role::Cr3Cmt(_)) && !RAWLER {
            return Fate::because(Disposition::Skipped, CR3_NOT_DECODED);
        }
        if ifd.followed && ifd.kind == Kind::MakerNote {
            return Fate::because(
                Disposition::Structure,
                if RAWLER {
                    "maker-note IFD, parsed"
                } else {
                    "maker-note IFD rawloader parses (TiffIFD::new_makernote)"
                },
            );
        }
        if ifd.followed {
            return Fate::because(Disposition::Structure, "walked by rawloader");
        }
        let why = match ifd.kind {
            Kind::Gps => "rawloader does not walk GPS IFDs; zenraw reports no GPS",
            Kind::Interop => "rawloader does not walk Interop IFDs",
            Kind::Page(_) => "past the backend's IFD chain limit",
            _ => "not walked by rawloader",
        };
        Fate::because(Disposition::Skipped, why)
    }

    fn entry(&self, w: &Walk<'_>, ifd: &Ifd, index: usize) -> Fate {
        let Some(e) = ifd.entries.get(index) else {
            return Fate::new(Disposition::Malformed);
        };
        if !ifd.followed {
            return self.unread(ifd.kind, e.tag, "in a directory the decoder does not walk");
        }
        if let Role::Cr3Cmt(n) = self.role {
            return cr3_cmt_entry(n, e.tag, ifd.kind);
        }
        // rawler leaves every image's orientation Normal (rawimage.rs
        // `orientation: Orientation::Normal, // TODO fixme`), so only
        // rawloader builds report the tag.
        if e.tag == ORIENTATION && self.orientation_ifd == Some(ifd.at) && !RAWLER {
            return Fate::new(Disposition::Metadata(MetadataKind::Orientation));
        }
        if e.tag == ORIENTATION && RAWLER {
            return Fate::because(
                Disposition::Dropped,
                "parsed by rawler, which reports every image as Normal orientation (zenraw#22)",
            );
        }
        if RAWLER {
            return Fate::because(
                Disposition::Structure,
                "parsed by rawler, which keeps every tag of the IFDs it walks; its decoders read what they need",
            );
        }
        match e.tag {
            // rawloader parses the entry (Xmp is one of its tags) but never
            // reads the value; zenraw's XMP comes from a byte scan.
            XMP if self.xmp => Fate::because(
                Disposition::Dropped,
                "parsed by rawloader, value unused; zenraw finds XMP by scanning the file's bytes",
            ),
            XMP => Fate::because(
                Disposition::Dropped,
                "parsed by rawloader, value unused; XMP is surfaced only with the `xmp` feature",
            ),
            MAKER_NOTE => match w.loc(e) {
                tiff_walk::Loc::At(off, _) if !self.parsed_notes.contains(&off) => Fate::because(
                    Disposition::Dropped,
                    "rawloader cannot parse this maker note as an IFD (signature it does not know), so nothing reads it",
                ),
                _ => Fate::because(
                    Disposition::Structure,
                    "parsed by rawloader as a maker-note IFD; camera decoders read white balance, levels and keys from it",
                ),
            },
            GPS_IFD => Fate::because(
                Disposition::Skipped,
                "GPS IFD pointer; rawloader does not follow it",
            ),
            tag if rawloader_knows(tag) => Fate::because(
                Disposition::Structure,
                "parsed by rawloader (raw layout, camera parameters); its decoders may read only part of the value (StripOffsets[0], for example), which is not split out",
            ),
            tag if ifd.kind == Kind::Exif => self.unread(
                ifd.kind,
                tag,
                "rawloader skips tags it does not know; zenraw reports no EXIF through zencodec",
            ),
            tag => self.unread(ifd.kind, tag, "rawloader skips tags it does not know"),
        }
    }

    fn value(&self, w: &Walk<'_>, ifd: &Ifd, index: usize) -> Fate {
        match ifd.entries.get(index) {
            Some(e) if e.tag == XMP && ifd.followed && RAWLER => Fate::because(
                Disposition::Dropped,
                "rawler reads it but nothing uses it; zenraw's XMP comes from a byte scan",
            ),
            Some(e) if e.tag == XMP && ifd.followed => Fate::because(
                Disposition::Skipped,
                if self.xmp {
                    "rawloader never reads it; bytes outside the first XMP packet zenraw's scan finds"
                } else {
                    "rawloader never reads it; XMP is surfaced only with the `xmp` feature"
                },
            ),
            _ => self.entry(w, ifd, index),
        }
    }

    fn image_data(&self, _w: &Walk<'_>, ifd: &Ifd) -> Fate {
        if self.raw_ifd == Some(ifd.at) {
            Fate::because(Disposition::ImageData, "raw sensor data rawloader decodes")
        } else {
            Fate::because(
                Disposition::Skipped,
                "preview or thumbnail image, not decoded",
            )
        }
    }

    fn jpeg_stream(&self, _w: &Walk<'_>, _ifd: &Ifd) -> Fate {
        Fate::because(Disposition::Skipped, "embedded JPEG preview, not decoded")
    }

    fn extents(&self, w: &Walk<'_>, ifd: &Ifd) -> Vec<ExtentRule> {
        if !matches!(ifd.kind, Kind::Page(_) | Kind::Sub) {
            return Vec::new();
        }
        let mut rules = Vec::new();
        if w.magic == 0x55 && ifd.kind == Kind::Page(0) {
            // RW2: no byte count is stored; rawloader's Panasonic decoder
            // reads as far as the sensor size needs (see `rw2_raw_len`).
            rules.push(ExtentRule {
                offsets: PANA_OFFSETS,
                counts: rw2_raw_len(w, ifd).map_or(Count::ToEnd, Count::Bytes),
                what: "raw data",
                kind: PartKind::Extent,
                fate: self.image_data(w, ifd),
            });
        }
        if w.find(ifd, SONY_OFFSET).is_some() && w.find(ifd, SONY_LENGTH).is_some() {
            rules.push(ExtentRule {
                offsets: SONY_OFFSET,
                counts: Count::Tag(SONY_LENGTH),
                what: "encrypted SR2SubIFD",
                kind: PartKind::Extent,
                fate: Fate::because(
                    Disposition::Structure,
                    "rawloader decrypts it for the white-balance levels",
                ),
            });
        }
        if self.role == Role::RafCfa {
            rules.push(ExtentRule {
                offsets: RAF_OFFSETS,
                counts: Count::Tag(RAF_BYTE_COUNTS),
                what: "raw data",
                kind: PartKind::Extent,
                fate: self.image_data(w, ifd),
            });
        }
        rules.extend(default_extents(self, w, ifd));
        rules
    }
}

/// Bytes rawloader's Panasonic decoder (`Rw2Decoder::decode_panasonic`)
/// reads for a PanaWidth x PanaLength image: each row starts at
/// `(w*row*9 + w/14*2*row)/8`, and the bit pump loads whole 0x4000-byte
/// blocks.
fn rw2_raw_len(w: &Walk<'_>, ifd: &Ifd) -> Option<u64> {
    let width = w
        .find(ifd, 0x0002)
        .and_then(|e| w.uints(e, 1).first().copied())?;
    let height = w
        .find(ifd, 0x0003)
        .and_then(|e| w.uints(e, 1).first().copied())?;
    let bits = width
        .checked_mul(height)?
        .checked_mul(9)?
        .checked_add((width / 14).checked_mul(2)?.checked_mul(height)?)?;
    let bytes = bits / 8;
    bytes.checked_add(0x3FFF).map(|n| n & !0x3FFF)
}

/// The strip/tile/free/JPEG rules of [`Rules::extents`]' default.
fn default_extents(rules: &RawRules, w: &Walk<'_>, ifd: &Ifd) -> Vec<ExtentRule> {
    struct Default<'r>(&'r RawRules);
    impl Rules for Default<'_> {
        fn follows(&self, w: &Walk<'_>, p: &Ifd, e: &Entry) -> bool {
            self.0.follows(w, p, e)
        }
        fn follows_next(&self, ifd: &Ifd) -> bool {
            self.0.follows_next(ifd)
        }
        fn ifd(&self, w: &Walk<'_>, ifd: &Ifd) -> Fate {
            self.0.ifd(w, ifd)
        }
        fn entry(&self, w: &Walk<'_>, ifd: &Ifd, i: usize) -> Fate {
            self.0.entry(w, ifd, i)
        }
        fn image_data(&self, w: &Walk<'_>, ifd: &Ifd) -> Fate {
            self.0.image_data(w, ifd)
        }
        fn jpeg_stream(&self, w: &Walk<'_>, ifd: &Ifd) -> Fate {
            self.0.jpeg_stream(w, ifd)
        }
    }
    Default(rules).extents(w, ifd)
}

/// IFD indices in rawloader's depth-first order: each chain IFD, then the
/// sub-IFDs its entries point to (recursively), in entry order.
fn rawloader_order(w: &Walk<'_>) -> Vec<usize> {
    let mut children: Vec<Vec<(usize, usize)>> = alloc::vec![Vec::new(); w.ifds.len()];
    let mut roots = Vec::new();
    for (i, ifd) in w.ifds.iter().enumerate() {
        if !ifd.followed {
            continue;
        }
        match ifd.parent {
            Some((p, e)) => children[p].push((e, i)),
            None if matches!(ifd.kind, Kind::Page(_)) => roots.push(i),
            None => {}
        }
    }
    for c in &mut children {
        c.sort_unstable();
    }
    let mut order = Vec::new();
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    while let Some(i) = stack.pop() {
        order.push(i);
        for &(_, c) in children[i].iter().rev() {
            stack.push(c);
        }
    }
    order
}

/// The IFD rawloader decodes, approximating its per-make choice from
/// structure: DNG takes the first full-resolution IFD with a compression it
/// handles; otherwise the first IFD carrying a CR2 marker, a CFA pattern, an
/// RW2 or RAF raw offset, and failing those the first with strips or tiles.
fn pick_raw_ifd(w: &Walk<'_>) -> Option<u64> {
    let order = rawloader_order(w);
    let has = |i: usize, tag: u16| w.find(&w.ifds[i], tag).is_some();
    let dng = order.iter().any(|&i| has(i, DNG_VERSION));
    if dng {
        return order
            .iter()
            .copied()
            .find(|&i| {
                let ifd = &w.ifds[i];
                let compression = w
                    .find(ifd, COMPRESSION)
                    .and_then(|e| w.uints(e, 1).first().copied());
                let reduced = w
                    .find(ifd, NEW_SUBFILE_TYPE)
                    .and_then(|e| w.uints(e, 1).first().copied())
                    .is_some_and(|v| v & 1 != 0);
                !reduced && matches!(compression, Some(1 | 7 | 0x884c))
            })
            .map(|i| w.ifds[i].at);
    }
    for tag in [CR2_ID, CFA_PATTERN, PANA_OFFSETS, RAF_OFFSETS] {
        if tag == PANA_OFFSETS && w.magic != 0x55 {
            continue;
        }
        if let Some(i) = order.iter().copied().find(|&i| has(i, tag)) {
            return Some(w.ifds[i].at);
        }
    }
    order
        .iter()
        .copied()
        .find(|&i| has(i, STRIP_OFFSETS) || has(i, TILE_OFFSETS))
        .map(|i| w.ifds[i].at)
}

/// Walk and place one TIFF of the file.
fn place_tiff(
    data: &[u8],
    base: u64,
    limit: u64,
    role: Role,
    xmp: bool,
    first: Vec<Cand>,
) -> (Vec<Cand>, u64, bool, Option<String>) {
    let mut rules = RawRules {
        role,
        raw_ifd: None,
        orientation_ifd: None,
        xmp,
        dng: false,
        parsed_notes: Vec::new(),
    };
    let mut w = walk(data, base, limit, &rules);
    rules.dng = w.ifds.iter().any(|ifd| w.find(ifd, DNG_VERSION).is_some());
    if matches!(role, Role::Main | Role::RafCfa) {
        rules.raw_ifd = pick_raw_ifd(&w);
    }
    if matches!(role, Role::Main | Role::RafJpegExif) {
        rules.orientation_ifd = rawloader_order(&w)
            .into_iter()
            .find(|&i| w.find(&w.ifds[i], ORIENTATION).is_some())
            .map(|i| w.ifds[i].at);
    }
    let notes = if matches!(role, Role::Main | Role::RafJpegExif) {
        maker_notes(data, limit, &w, &rules)
    } else {
        Vec::new()
    };
    rules.parsed_notes = notes.iter().map(|n| n.range.start).collect();
    let p = place(&mut w, &rules, first);
    let parts = if notes.is_empty() {
        p.parts
    } else {
        attach_maker_notes(p.parts, notes)
    };
    (parts, p.logical_end, p.ifd0_ok, w.fatal.clone())
}

/// What one CR3 `CMTn` TIFF's entries mean on the zencodec path: rawler
/// reads Make/Model from CMT1 for the camera lookup and the quality mode
/// (tag 1) from CMT3; the rest goes into rawler's own EXIF, which zenraw
/// never reports (decoders/cr3.rs `new`, `raw_metadata`, `get_mode`).
fn cr3_cmt_entry(n: u8, tag: u16, kind: Kind) -> Fate {
    if !RAWLER {
        return if tag_known(kind, tag) {
            Fate::because(Disposition::Skipped, CR3_NOT_DECODED)
        } else {
            Fate::because(Disposition::Unknown, CR3_NOT_DECODED)
        };
    }
    match (n, tag) {
        (1, 271 | 272) => Fate::because(
            Disposition::Structure,
            "camera lookup (rawler cr3.rs `new`)",
        ),
        (3, 1) => Fate::because(
            Disposition::Structure,
            "quality mode (rawler cr3.rs `get_mode`)",
        ),
        (4, _) => Fate::because(
            Disposition::Dropped,
            "GPS: parsed by rawler into its EXIF; zenraw reports none",
        ),
        _ => Fate::because(
            Disposition::Dropped,
            "parsed by rawler into its EXIF and maker-note data; zenraw reports none",
        ),
    }
}

/// A maker note rawloader parses as an IFD, walked with its vendor rules.
struct MakerNote {
    /// The MakerNote value's range.
    range: Range<u64>,
    /// Signature and embedded-header parts.
    header: Vec<Cand>,
    /// Parts of the walked maker-note IFDs.
    parts: Vec<Cand>,
}

/// Maker notes in `w` that rawloader parses (decoders/tiff.rs
/// `TiffIFD::new_makernote`), each walked with the same base and byte-order
/// rules rawloader uses.
fn maker_notes(data: &[u8], limit: u64, w: &Walk<'_>, rules: &RawRules) -> Vec<MakerNote> {
    let mut out = Vec::new();
    for ifd in w.ifds.iter().filter(|i| i.followed && i.problem.is_none()) {
        let Some(e) = w.find(ifd, MAKER_NOTE) else {
            continue;
        };
        let tiff_walk::Loc::At(mn, size) = w.loc(e) else {
            continue;
        };
        let end = mn.saturating_add(size).min(limit);
        if mn >= end || out.iter().any(|m: &MakerNote| m.range.start == mn) {
            continue;
        }
        if let Some(note) = maker_note(data, limit, w, rules, mn..end) {
            out.push(note);
        }
    }
    out
}

fn sig(data: &[u8], at: u64, s: &[u8]) -> bool {
    get(data, at, s.len() as u64) == Some(s)
}

fn byte_order(data: &[u8], at: u64) -> Option<bool> {
    match get(data, at, 2) {
        Some(b"II") => Some(true),
        Some(b"MM") => Some(false),
        _ => None,
    }
}

/// One maker note at `r`, or `None` when rawloader would fail to parse it
/// as an IFD (more than 4000 entries, as with SONY/Panasonic/Fujifilm/Apple
/// signatures it does not know).
fn maker_note(
    data: &[u8],
    limit: u64,
    w: &Walk<'_>,
    rules: &RawRules,
    r: Range<u64>,
) -> Option<MakerNote> {
    let mn = r.start;
    let mut header = Vec::new();
    let mut head = |range: Range<u64>, d: Disposition, what: &str| {
        if range.start < range.end && range.end <= r.end {
            header.push(Cand::new(
                range,
                PartKind::Header,
                PartTag::None,
                d,
                what.into(),
            ));
        }
    };
    let le = w.lay.le;
    let lay = |le: bool| tiff_walk::Layout { le, big: false };
    let unread = if RAWLER {
        Disposition::Structure
    } else {
        Disposition::Skipped
    };
    // (IFD position, offset base, byte order, extra IFD for Olympus ImgProc)
    let (at, base, le, nested) = if sig(data, mn, b"OLYMP") {
        let new_style = sig(data, mn, b"OLYMPUS");
        let off = if new_style { 12 } else { 8 };
        head(mn..mn + 5, Disposition::Structure, "maker-note signature");
        head(
            mn + 5..mn + off,
            unread,
            "rest of the Olympus header; rawloader does not read it",
        );
        (mn + off, w.base, le, new_style)
    } else if sig(data, mn, b"PENTAX") {
        let ble = byte_order(data, mn + 8)?;
        head(mn..mn + 6, Disposition::Structure, "maker-note signature");
        head(
            mn + 6..mn + 8,
            unread,
            "Pentax header bytes rawloader does not read",
        );
        head(mn + 8..mn + 10, Disposition::Structure, "byte order");
        (mn + 10, mn, ble, false)
    } else if sig(data, mn, b"Nikon\0\x02") {
        let ble = byte_order(data, mn + 10)?;
        head(mn..mn + 7, Disposition::Structure, "maker-note signature");
        head(
            mn + 7..mn + 10,
            unread,
            "maker-note version; rawloader does not read it",
        );
        head(
            mn + 10..mn + 12,
            Disposition::Structure,
            "embedded TIFF byte order",
        );
        head(
            mn + 12..mn + 18,
            unread,
            "embedded TIFF magic and IFD offset; rawloader assumes 8",
        );
        (mn + 18, mn + 10, ble, false)
    } else {
        let mut off = 0;
        if sig(data, mn, b"EPSON") {
            head(mn..mn + 5, Disposition::Structure, "maker-note signature");
            head(
                mn + 5..mn + 8,
                unread,
                "rest of the Epson header; rawloader does not read it",
            );
            off = 8;
        } else if sig(data, mn, b"AOC\0") {
            head(mn..mn + 4, Disposition::Structure, "maker-note signature");
            off = 4;
        }
        // rawloader checks "II" and then "MM" at the advanced offset.
        let mut ble = le;
        if byte_order(data, mn + off) == Some(true) {
            head(mn + off..mn + off + 2, Disposition::Structure, "byte order");
            off += 2;
            ble = true;
        }
        if byte_order(data, mn + off) == Some(false) {
            head(mn + off..mn + off + 2, Disposition::Structure, "byte order");
            off += 2;
            ble = false;
        }
        (mn + off, w.base, ble, false)
    };
    // rawloader's `TiffIFD::new` rejects more than 4000 entries.
    let count = lay(le).u16(data, at)?;
    if count > 4000 {
        return None;
    }
    let mut mw = tiff_walk::walk_ifd(
        data,
        base,
        limit,
        lay(le),
        at,
        Kind::MakerNote,
        "maker-note IFD".into(),
        rules,
    );
    let mut parts = place(&mut mw, rules, Vec::new()).parts;
    if nested {
        // New-style Olympus: the ImgProc sub-IFD (0x2040) sits at an offset
        // from the maker note's start, with values relative to it.
        let ioff = mw
            .ifds
            .first()
            .and_then(|i| mw.find(i, 0x2040))
            .and_then(|e| mw.uints(e, 1).first().copied());
        if let Some(ioff) = ioff.and_then(|o| mn.checked_add(o)) {
            let mut iw = tiff_walk::walk_ifd(
                data,
                mn,
                limit,
                lay(le),
                ioff,
                Kind::MakerNote,
                "Olympus ImgProc IFD".into(),
                rules,
            );
            parts.extend(place(&mut iw, rules, Vec::new()).parts);
        }
    }
    Some(MakerNote {
        range: r,
        header,
        parts,
    })
}

/// Hang each walked maker note under its MakerNote value part (a container
/// whose unclaimed bytes become `Unreferenced`); maker-note parts outside
/// the value are placed at the top level.
fn attach_maker_notes(parts: Vec<Cand>, notes: Vec<MakerNote>) -> Vec<Cand> {
    let mut parts = parts;
    let mut outside = Vec::new();
    for note in notes {
        let Some(host) = parts.iter_mut().find(|c| {
            c.kind == PartKind::Field
                && c.tag == PartTag::Code(u32::from(MAKER_NOTE))
                && c.range == note.range
                && c.disp.is_consumed()
        }) else {
            continue;
        };
        let mut inner = Placed::default();
        for c in note.header {
            inner.insert(c);
        }
        for c in note.parts {
            if c.range.start >= host.range.start && c.range.end <= host.range.end {
                inner.insert(c);
            } else {
                outside.push(c);
            }
        }
        host.children = inner.into_sorted();
        host.body = Some(host.range.clone());
        host.disp = Disposition::Structure;
        host.detail = String::from(
            "MakerNote value, walked as an IFD below; bytes nothing references are Unreferenced",
        );
    }
    if outside.is_empty() {
        return parts;
    }
    let mut placed = Placed::default();
    for c in parts {
        placed.insert(c);
    }
    for c in outside {
        placed.insert(c);
    }
    placed.into_sorted()
}

/// The surfaced XMP packet, as a part to place ahead of everything else.
#[cfg(feature = "xmp")]
fn xmp_cand(data: &[u8]) -> Option<Cand> {
    let r = crate::xmp::extract_xmp_range(data)?;
    let range = r.start as u64..r.end as u64;
    Some(Cand::new(
        range,
        PartKind::Field,
        PartTag::Name("XMP packet".into()),
        Disposition::Metadata(MetadataKind::Xmp),
        "first XMP packet zenraw's byte scan finds (xmp.rs extract_xmp)".into(),
    ))
}

#[cfg(not(feature = "xmp"))]
fn xmp_cand(_data: &[u8]) -> Option<Cand> {
    None
}

/// Inventory `data` the way zenraw's zencodec decode job reads it.
pub(crate) fn inventory(data: &[u8], format: ImageFormat) -> Result<Inventory, InventoryError> {
    let len = data.len() as u64;
    let xmp = cfg!(feature = "xmp");
    let mut inv = Inventory::new(format, len);
    let (parts, logical_end, gap) = if data.starts_with(b"FUJIFILM") {
        raf(data, xmp)
    } else if data.get(4..8) == Some(b"ftyp") {
        cr3(data)
    } else if matches!(data.get(..2), Some(b"II" | b"MM")) {
        let first: Vec<Cand> = xmp_cand(data).into_iter().collect();
        let (parts, end, ok, fatal) = place_tiff(data, 0, len, Role::Main, xmp, first);
        let gap = if ok {
            (Disposition::Unreferenced, None)
        } else {
            (Disposition::Malformed, fatal)
        };
        (parts, end, gap)
    } else {
        (
            Vec::new(),
            len,
            (
                Disposition::Malformed,
                Some(String::from("not a TIFF-based RAW, RAF or CR3 container")),
            ),
        )
    };
    emit_top(&mut inv, data, parts, logical_end, gap)?;
    inv.fill_gaps(None, Disposition::Trailing)?;
    Ok(inv)
}

type Placement = (Vec<Cand>, u64, (Disposition, Option<String>));

fn be32(d: &[u8], at: u64) -> Option<u64> {
    let b: [u8; 4] = get(d, at, 4)?.try_into().ok()?;
    Some(u64::from(u32::from_be_bytes(b)))
}

fn be16(d: &[u8], at: u64) -> Option<u64> {
    let b: [u8; 2] = get(d, at, 2)?.try_into().ok()?;
    Some(u64::from(u16::from_be_bytes(b)))
}

/// A section `[off, off + len)` clipped to the input, or `None` when it
/// starts outside it.
fn section(off: u64, n: u64, len: u64) -> Option<(Range<u64>, bool)> {
    if off >= len || n == 0 {
        return None;
    }
    let end = off.saturating_add(n);
    Some((off..end.min(len), end > len))
}

/// RAF: header, JPEG preview, CFA header records, CFA section
/// (rawloader `TiffIFD::new_file`).
fn raf(data: &[u8], xmp: bool) -> Placement {
    let len = data.len() as u64;
    let mut placed = Placed::default();
    if let Some(c) = xmp_cand(data) {
        placed.insert(c);
    }
    let (
        Some(jpeg_off),
        Some(jpeg_len),
        Some(hdr_off),
        Some(hdr_len),
        Some(cfa_off),
        Some(cfa_len),
    ) = (
        be32(data, 84),
        be32(data, 88),
        be32(data, 92),
        be32(data, 96),
        be32(data, 100),
        be32(data, 104),
    )
    else {
        let mut h = Cand::new(
            0..len,
            PartKind::Header,
            PartTag::None,
            Disposition::Malformed,
            "truncated RAF header".into(),
        );
        h.label = Some(String::from("FUJIFILM"));
        placed.insert(h);
        return (placed.into_sorted(), len, (Disposition::Malformed, None));
    };
    let header_end = if (108..=4096).contains(&jpeg_off) {
        jpeg_off.min(len)
    } else {
        108
    };
    let mut h = Cand::new(
        0..header_end,
        PartKind::Header,
        PartTag::None,
        Disposition::Structure,
        "RAF header".into(),
    );
    h.label = Some(
        String::from_utf8_lossy(&data[..16.min(data.len())])
            .trim_end()
            .into(),
    );
    // rawloader checks the magic and reads three section offsets; the
    // version, camera ID and other fields are never read.
    let (unread, why) = if RAWLER {
        (
            Disposition::Structure,
            "version, camera ID and other header fields, parsed by rawler",
        )
    } else {
        (
            Disposition::Skipped,
            "version, camera ID and other header fields; rawloader does not read them",
        )
    };
    for (r, d, what) in [
        (0..8, Disposition::Structure, "magic"),
        (8..84, unread, why),
        (
            84..108,
            Disposition::Structure,
            "JPEG, CFA-header and CFA offsets and lengths",
        ),
        (108..header_end, unread, why),
    ] {
        if r.start < r.end && r.end <= header_end {
            h.children
                .push(Cand::new(r, PartKind::Field, PartTag::None, d, what.into()));
        }
    }
    placed.insert(h);
    let mut logical_end = header_end;

    if let Some((r, cut)) = section(jpeg_off, jpeg_len, len) {
        logical_end = logical_end.max(r.end);
        let mut c = Cand::new(
            r.clone(),
            PartKind::EmbeddedImage,
            PartTag::None,
            if cut { Disposition::Malformed } else { Disposition::Skipped },
            "JPEG preview; rawloader reads only the EXIF TIFF inside its APP1 (make, model, orientation)".into(),
        );
        // FFD8 FFE1 <len> "Exif\0\0" <TIFF>
        if get(data, r.start, 4) == Some(&[0xFF, 0xD8, 0xFF, 0xE1])
            && get(data, r.start + 6, 6) == Some(b"Exif\0\0")
        {
            let app1_end = be16(data, r.start + 4).map_or(r.end, |n| (r.start + 4 + n).min(r.end));
            let (parts, _, _, _) = place_tiff(
                data,
                r.start + 12,
                app1_end,
                Role::RafJpegExif,
                xmp,
                Vec::new(),
            );
            c.children = parts;
        }
        placed.insert(c);
    }
    if let Some((r, cut)) = section(hdr_off, hdr_len, len) {
        logical_end = logical_end.max(r.end);
        placed.insert(raf_cfa_header(data, r, cut));
    }
    if let Some((r, cut)) = section(cfa_off, cfa_len, len) {
        logical_end = logical_end.max(r.end);
        let is_tiff = matches!(get(data, r.start, 4), Some(b"II*\0" | b"MM\0*"));
        let mut c = Cand::new(
            r.clone(),
            PartKind::Chunk,
            PartTag::None,
            if cut {
                Disposition::Malformed
            } else {
                Disposition::Structure
            },
            "RAF CFA section".into(),
        );
        if is_tiff {
            let (parts, _, _, _) = place_tiff(data, r.start, r.end, Role::RafCfa, xmp, Vec::new());
            c.children = parts;
        } else if !cut {
            c.disp = Disposition::ImageData;
            c.detail = "RAF CFA data rawloader decodes (offset at byte 100)".into();
        }
        placed.insert(c);
    }
    if cfa_off.saturating_add(cfa_len) > len
        || hdr_off.saturating_add(hdr_len) > len
        || jpeg_off.saturating_add(jpeg_len) > len
    {
        logical_end = len;
    }
    (
        placed.into_sorted(),
        logical_end,
        (Disposition::Unreferenced, None),
    )
}

/// RAF CFA header: a big-endian record count, then `tag:u16 len:u16 data`
/// records (rawloader `TiffIFD::new_fuji`, which reads ImageWidth 0x100 and
/// RafOldWB 0x2FF0).
fn raf_cfa_header(data: &[u8], r: Range<u64>, cut: bool) -> Cand {
    let mut c = Cand::new(
        r.clone(),
        PartKind::Ifd,
        PartTag::None,
        if cut {
            Disposition::Malformed
        } else {
            Disposition::Structure
        },
        "RAF CFA header records".into(),
    );
    let d = &data[..r.end as usize];
    let Some(count) = be32(d, r.start) else {
        return c;
    };
    let mut at = r.start + 4;
    for i in 0..count.min(4000) {
        let (Some(tag), Some(n)) = (be16(d, at), be16(d, at + 2)) else {
            c.note(&format!("record {i} runs past the end of the section"));
            break;
        };
        let end = at + 4 + n;
        if end > r.end {
            c.note(&format!("record {i} runs past the end of the section"));
            break;
        }
        let fate = match tag {
            _ if RAWLER => Fate::because(Disposition::Structure, "read by rawler"),
            0x100 | 0x2FF0 => Fate::because(Disposition::Structure, "read by rawloader"),
            _ => Fate::because(
                Disposition::Skipped,
                "rawloader reads only ImageWidth and RafOldWB here",
            ),
        };
        let mut detail = format!("record {i}, {n} bytes");
        if let Some(note) = fate.note {
            tiff_walk::push_note(&mut detail, &note);
        }
        c.children.push(Cand::new(
            at..end,
            PartKind::Field,
            PartTag::Code(tag as u32),
            fate.d,
            detail,
        ));
        at = end;
    }
    c
}

/// ISOBMFF container boxes the CR3 walk recurses into.
fn is_container(fourcc: &[u8; 4]) -> bool {
    matches!(
        fourcc,
        b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"dinf" | b"edts"
    )
}

/// Canon's XMP `uuid` at the top level of a CR3 (rawler `Cr3XpacketBox`).
const CR3_XMP_UUID: [u8; 16] = [
    0xbe, 0x7a, 0xcf, 0xcb, 0x97, 0xa9, 0x42, 0xe8, 0x9c, 0x71, 0x99, 0x94, 0x91, 0xe3, 0xaf, 0xac,
];

/// Sample tables of one `trak`, as payload ranges.
#[derive(Default)]
struct Trak {
    /// Type of the first `stsd` sample entry (`CRAW`, `CTMD`, ...).
    entry: Option<[u8; 4]>,
    stsz: Option<Range<u64>>,
    stsc: Option<Range<u64>>,
    co64: Option<Range<u64>>,
    stco: Option<Range<u64>>,
}

/// State of one CR3 walk.
struct Cr3<'a> {
    data: &'a [u8],
    rawler: bool,
    budget: usize,
    traks: Vec<Trak>,
    /// 0-based index of the `trak` CCTP names as the full raw image
    /// (CCDT image type 0, rawler `Cr3ImageType::CrxBix`).
    raw_trak: Option<usize>,
}

/// Samples mapped per file, so a crafted sample table cannot explode.
const MAX_CR3_SAMPLES: usize = 1 << 16;

/// CR3 (ISOBMFF): every box, recursing into containers and Canon's metadata
/// `uuid`; the `CMT1`..`CMT4` TIFFs walked; `mdat` split into the samples
/// each `trak` locates (rawler decoders/cr3.rs and formats/bmff).
fn cr3(data: &[u8]) -> Placement {
    let len = data.len() as u64;
    let mut walk = Cr3 {
        data,
        rawler: RAWLER,
        budget: 4096,
        traks: Vec::new(),
        raw_trak: None,
    };
    let mut boxes = Vec::new();
    let mut at = 0u64;
    let mut logical_end = 0;
    let mut tail = None;
    while at < len {
        match walk.walk_box(at, len, 0, None, true) {
            Some(c) => {
                at = c.range.end;
                logical_end = at;
                boxes.push(c);
            }
            None => {
                tail = Some(Cand::new(
                    at..len,
                    PartKind::Gap,
                    PartTag::None,
                    Disposition::Malformed,
                    "box header runs past the end of the file".into(),
                ));
                logical_end = len;
                break;
            }
        }
    }
    walk.map_samples(&mut boxes);
    let mut placed = Placed::default();
    for c in boxes.into_iter().chain(tail) {
        placed.insert(c);
    }
    (
        placed.into_sorted(),
        logical_end,
        (Disposition::Unreferenced, None),
    )
}

impl Cr3<'_> {
    /// One box at `at` within `end`, with its children. `trak` is the index
    /// of the enclosing `trak`, `top` whether the box sits at file level.
    fn walk_box(
        &mut self,
        at: u64,
        end: u64,
        depth: u32,
        trak: Option<usize>,
        top: bool,
    ) -> Option<Cand> {
        let data = self.data;
        let size32 = be32(data, at)?;
        let fourcc: [u8; 4] = get(data, at + 4, 4)?.try_into().ok()?;
        let (mut header, size) = match size32 {
            0 => (8, end - at),
            1 => {
                let b: [u8; 8] = get(data, at + 8, 8)?.try_into().ok()?;
                (16, u64::from_be_bytes(b))
            }
            n => (8, n),
        };
        let uuid = if &fourcc == b"uuid" {
            let u: [u8; 16] = get(data, at + header, 16)?.try_into().ok()?;
            header += 16;
            Some(u)
        } else {
            None
        };
        if size < header {
            return None;
        }
        let box_end = at.saturating_add(size);
        let cut = box_end > end;
        let box_end = box_end.min(end);
        let payload = (at + header).min(box_end)..box_end;
        self.budget = self.budget.saturating_sub(1);
        let rawler = self.rawler;
        let canon_meta = uuid == Some(CANON_UUID);
        let container = is_container(&fourcc) || canon_meta;
        let cmt = match &fourcc {
            b"CMT1" => Some(1),
            b"CMT2" => Some(2),
            b"CMT3" => Some(3),
            b"CMT4" => Some(4),
            _ => None,
        };
        // (disposition, detail, split: payload disposition and detail)
        let (disp, note, split): (Disposition, &str, Option<(Disposition, &str)>) = match (
            &fourcc, rawler,
        ) {
            (b"ftyp", _) => (
                Disposition::Structure,
                "brand checked by zenraw's format detection",
                None,
            ),
            (b"free" | b"skip", false) => (Disposition::Padding, "free space", None),
            (b"free" | b"skip", true) => (
                Disposition::Structure,
                "box header parsed by rawler",
                Some((Disposition::Padding, "free space")),
            ),
            (_, false) => (Disposition::Skipped, CR3_NOT_DECODED, None),
            (b"mdat", true) => (
                Disposition::Structure,
                "box header; samples the tracks locate are listed below",
                None,
            ),
            (b"THMB", true) => (
                Disposition::Structure,
                "box header parsed by rawler",
                Some((
                    Disposition::Dropped,
                    "thumbnail JPEG rawler parses but does not use",
                )),
            ),
            (b"uuid", true) if top && uuid == Some(CR3_XMP_UUID) => (
                Disposition::Structure,
                "box header parsed by rawler",
                Some((
                    Disposition::Dropped,
                    "XMP packet rawler reads but zenraw does not use; zenraw's XMP comes from its own byte scan",
                )),
            ),
            (b"moov", true) => (Disposition::Structure, "parsed by rawler", None),
            (_, true) if top => (
                Disposition::Structure,
                "box header parsed by rawler",
                Some((
                    Disposition::Dropped,
                    "vendor box rawler reads and does not use (preview data)",
                )),
            ),
            (_, true) => (Disposition::Structure, "parsed by rawler", None),
        };
        let disp = if cut { Disposition::Malformed } else { disp };
        let mut c = Cand::new(
            at..box_end,
            PartKind::Box,
            PartTag::FourCc(fourcc),
            disp,
            note.into(),
        );
        if cut {
            c.note("box runs past the end of its parent");
        }
        if let Some((d, n)) = split
            && !cut
            && payload.start < payload.end
        {
            c.body = Some(at..box_end);
            c.children.push(Cand::new(
                at..payload.start,
                PartKind::Header,
                PartTag::None,
                Disposition::Structure,
                "box header".into(),
            ));
            c.children.push(Cand::new(
                payload.clone(),
                PartKind::Gap,
                PartTag::None,
                d,
                n.into(),
            ));
        }
        let trak = if &fourcc == b"trak" && depth <= 1 {
            self.traks.push(Trak::default());
            Some(self.traks.len() - 1)
        } else {
            trak
        };
        if let Some(t) = trak.and_then(|t| self.traks.get_mut(t)) {
            match &fourcc {
                b"stsz" => t.stsz = Some(payload.clone()),
                b"stsc" => t.stsc = Some(payload.clone()),
                b"co64" => t.co64 = Some(payload.clone()),
                b"stco" => t.stco = Some(payload.clone()),
                b"stsd" => {
                    t.entry = get(data, payload.start + 12, 4).and_then(|b| b.try_into().ok());
                }
                _ => {}
            }
        }
        if &fourcc == b"CCTP" {
            self.read_cctp(payload.clone());
        }
        if let Some(n) = cmt
            && !cut
        {
            let (parts, _, _, _) = place_tiff(
                data,
                payload.start,
                payload.end,
                Role::Cr3Cmt(n),
                false,
                Vec::new(),
            );
            c.children = parts;
            c.body = Some(payload.clone());
            if rawler {
                c.detail = format!("CMT{n} TIFF, parsed by rawler");
            }
        }
        if (container || &fourcc == b"mdat") && !cut {
            c.body = Some(payload.clone());
        }
        if container && depth < 16 && self.budget > 0 {
            let mut child_at = payload.start;
            while child_at < box_end && self.budget > 0 {
                match self.walk_box(child_at, box_end, depth + 1, trak, false) {
                    Some(ch) => {
                        child_at = ch.range.end;
                        c.children.push(ch);
                    }
                    None => break,
                }
            }
        }
        Some(c)
    }

    /// CCTP: version/flags, two u32s, then CCDT boxes of
    /// `image_type:u64 dual_pixel:u32 trak_index:u32` (rawler ext_cr3).
    fn read_cctp(&mut self, payload: Range<u64>) {
        let mut at = payload.start + 12;
        while at + 24 <= payload.end {
            let (Some(size), Some(b"CCDT")) = (be32(self.data, at), get(self.data, at + 4, 4))
            else {
                break;
            };
            let image_type = get(self.data, at + 8, 8)
                .and_then(|b| <[u8; 8]>::try_from(b).ok())
                .map(u64::from_be_bytes);
            let trak_index = be32(self.data, at + 20);
            if image_type == Some(0)
                && let Some(t) = trak_index.filter(|&t| t > 0)
            {
                self.raw_trak = Some(t as usize - 1);
            }
            if size < 8 {
                break;
            }
            at += size;
        }
    }

    /// The samples of `trak`: (1-based sample number, byte range), from
    /// `stsc` runs over `co64`/`stco` chunk offsets and `stsz` sizes.
    fn samples(&self, t: &Trak) -> Vec<(u32, Range<u64>)> {
        let d = self.data;
        let mut out = Vec::new();
        let (Some(stsz), Some(stsc)) = (&t.stsz, &t.stsc) else {
            return out;
        };
        let Some(fixed) = be32(d, stsz.start + 4) else {
            return out;
        };
        let Some(count) = be32(d, stsz.start + 8) else {
            return out;
        };
        let size_of = |n: u64| -> Option<u64> {
            if fixed != 0 {
                Some(fixed)
            } else {
                be32(d, stsz.start + 12 + 4 * (n - 1))
            }
        };
        let runs = be32(d, stsc.start + 4).unwrap_or(0).min(65_536);
        let run = |i: u64| -> Option<(u64, u64)> {
            let at = stsc.start + 8 + 12 * i;
            Some((be32(d, at)?, be32(d, at + 4)?))
        };
        let (chunks, chunk_at): (u64, Box<dyn Fn(u64) -> Option<u64> + '_>) =
            match (&t.co64, &t.stco) {
                (Some(co), _) => (
                    be32(d, co.start + 4).unwrap_or(0),
                    Box::new(move |i| {
                        let b: [u8; 8] = get(d, co.start + 8 + 8 * i, 8)?.try_into().ok()?;
                        Some(u64::from_be_bytes(b))
                    }),
                ),
                (None, Some(co)) => (
                    be32(d, co.start + 4).unwrap_or(0),
                    Box::new(move |i| be32(d, co.start + 8 + 4 * i)),
                ),
                _ => return out,
            };
        let mut sample = 1u64;
        let mut r = 0u64;
        for chunk in 1..=chunks.min(MAX_CR3_SAMPLES as u64) {
            while r + 1 < runs && run(r + 1).is_some_and(|(first, _)| first <= chunk) {
                r += 1;
            }
            let Some((_, per_chunk)) = run(r) else { break };
            let Some(mut off) = chunk_at(chunk - 1) else {
                break;
            };
            for _ in 0..per_chunk.min(MAX_CR3_SAMPLES as u64) {
                if sample > count || out.len() >= MAX_CR3_SAMPLES {
                    return out;
                }
                let Some(n) = size_of(sample) else { return out };
                let end = off.saturating_add(n);
                out.push((sample as u32, off..end));
                off = end;
                sample += 1;
            }
        }
        out
    }

    /// Hang every track's samples under the `mdat` that holds them.
    fn map_samples(&self, boxes: &mut [Cand]) {
        let mut extents = Vec::new();
        for (i, t) in self.traks.iter().enumerate() {
            let entry = t.entry.map(|e| String::from_utf8_lossy(&e).into_owned());
            let ctmd = t.entry == Some(*b"CTMD");
            for (n, r) in self.samples(t) {
                if r.start >= r.end {
                    continue;
                }
                let (disp, why): (Disposition, String) = if !self.rawler {
                    (Disposition::Skipped, CR3_NOT_DECODED.into())
                } else if self.raw_trak == Some(i) && n == 1 && t.co64.is_some() {
                    (
                        Disposition::ImageData,
                        "CRX raw image rawler decodes (CCTP raw track, image 0)".into(),
                    )
                } else if self.raw_trak == Some(i) && t.co64.is_none() {
                    (Disposition::Skipped, "rawler reads only co64 chunk offsets; this track uses stco, so the decode fails".into())
                } else if self.raw_trak == Some(i) {
                    (
                        Disposition::Skipped,
                        format!("image {}, not decoded (zencodec decodes image 0)", n - 1),
                    )
                } else if ctmd && n == 1 {
                    (
                        Disposition::Structure,
                        "CTMD records rawler reads for white balance and levels".into(),
                    )
                } else {
                    (
                        Disposition::Skipped,
                        "preview or other track sample, not decoded".into(),
                    )
                };
                let mut c = Cand::new(
                    r,
                    PartKind::Extent,
                    PartTag::Code(i as u32 + 1),
                    disp,
                    format!("trak {} sample {n}; {why}", i + 1),
                );
                c.label = entry.clone();
                extents.push(c);
            }
        }
        for mdat in boxes
            .iter_mut()
            .filter(|b| b.tag == PartTag::FourCc(*b"mdat"))
        {
            let Some(body) = mdat.body.clone() else {
                continue;
            };
            let mut inner = Placed::default();
            let mut outside = 0usize;
            for c in extents.iter() {
                if c.range.start >= body.start && c.range.end <= body.end {
                    let mut copy = Cand::new(
                        c.range.clone(),
                        c.kind,
                        c.tag.clone(),
                        c.disp,
                        c.detail.clone(),
                    );
                    copy.label = c.label.clone();
                    inner.insert(copy);
                } else if c.range.start < body.end && c.range.end > body.start {
                    outside += 1;
                }
            }
            mdat.children = inner.into_sorted();
            if outside > 0 {
                mdat.note(&format!(
                    "{outside} sample(s) cross the mdat boundary and are not listed"
                ));
            }
        }
    }
}
