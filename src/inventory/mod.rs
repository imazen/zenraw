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
    Cand, ChunkUse, Count, Entry, ExtentRule, Fate, GPS_IFD, Ifd, Kind, Placed, Rules,
    STRIP_OFFSETS, TILE_OFFSETS, Walk, emit_top, get, place, union_fate, walk,
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
const SONY_KEY: u16 = 0x7221;
const WHITE_LEVEL: u16 = 0xC61D;
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

/// The backend's name, for details.
const BACKEND: &str = if RAWLER { "rawler" } else { "rawloader" };

/// zenraw's zencodec decode path for one TIFF of the file.
struct RawRules<'s> {
    role: Role,
    /// Absolute offset of the IFD whose strips/tiles the backend decodes.
    raw_ifd: Option<u64>,
    /// Absolute offset of the IFD whose Orientation entry the backend reads.
    orientation_ifd: Option<u64>,
    /// Whether the build surfaces XMP (`xmp` feature).
    xmp: bool,
    /// Whether the file is a DNG (DNGVersion anywhere the backend looks).
    dng: bool,
    /// The first Make the backend finds, cut at its NUL.
    make: Option<String>,
    /// Make "SONY" and not a DNG: the ARW decoder of either backend reads
    /// the SR2Private IFD DNGPrivateData points at (rawloader `arw.rs`
    /// `get_wb`, rawler `arw.rs` `get_params`).
    sony_arw: bool,
    /// Offsets of the MakerNote values the backend parses as IFDs.
    parsed_notes: Vec<u64>,
    stop: &'s dyn enough::Stop,
}

impl RawRules<'_> {
    fn unread(&self, kind: Kind, tag: u16, why: &'static str) -> Fate {
        if tag_known(kind, tag) {
            Fate::because(Disposition::Skipped, why)
        } else {
            Fate::because(Disposition::Unknown, why)
        }
    }

    fn make_is(&self, makes: &[&str]) -> bool {
        self.make.as_deref().is_some_and(|m| makes.contains(&m))
    }

    /// Whether `ifd` is the SR2Private IFD (reached through DNGPrivateData).
    fn is_sr2_private(w: &Walk<'_>, ifd: &Ifd) -> bool {
        ifd.parent
            .and_then(|(p, e)| w.ifds.get(p).and_then(|i| i.entries.get(e)))
            .is_some_and(|e| e.tag == DNG_PRIVATE_DATA)
    }

    /// The directory's fate when reached as `kind`.
    fn ifd_as(&self, kind: Kind, followed: bool) -> Fate {
        if matches!(self.role, Role::Cr3Cmt(_)) && !RAWLER {
            return Fate::because(Disposition::Skipped, CR3_NOT_DECODED);
        }
        if !followed {
            let why = match kind {
                Kind::Gps if !RAWLER => "rawloader does not walk GPS IFDs; zenraw reports no GPS",
                Kind::Interop => "neither backend walks Interop IFDs",
                Kind::Page(_) => "past the backend's IFD chain limit",
                Kind::MakerNote => "maker note this backend does not parse for this make",
                _ if RAWLER => {
                    "rawler follows only LONG (or small unknown-type) SubIFDs, EXIF and GPS pointers of main-chain IFDs (formats/tiff/ifd.rs)"
                }
                _ => "not walked by rawloader",
            };
            return Fate::because(Disposition::Skipped, why);
        }
        match kind {
            Kind::MakerNote if RAWLER => Fate::because(
                Disposition::Structure,
                "maker-note IFD, located the way rawloader does; rawler's parse_makernote may differ",
            ),
            Kind::MakerNote => Fate::because(
                Disposition::Structure,
                "maker-note IFD rawloader parses (TiffIFD::new_makernote)",
            ),
            _ if RAWLER => Fate::because(Disposition::Structure, "parsed by rawler"),
            _ => Fate::because(Disposition::Structure, "walked by rawloader"),
        }
    }

    fn entry_fate(&self, w: &Walk<'_>, ifd: &Ifd, index: usize) -> Fate {
        let Some(e) = ifd.entries.get(index) else {
            return Fate::new(Disposition::Malformed);
        };
        if !ifd.followed {
            return self.unread(ifd.kind, e.tag, "in a directory the decoder does not walk");
        }
        // rawloader keeps known tags in a HashMap and rawler all tags in a
        // BTreeMap: the last duplicate wins. rawloader follows every
        // occurrence of a pointer tag.
        let duplicate = ifd.last.get(&e.tag).is_some_and(|&l| l != index);
        if duplicate && (RAWLER || (rawloader_knows(e.tag) && !rawloader_follows(e.tag))) {
            return Fate::because(
                Disposition::Dropped,
                format!("overwritten by a later duplicate; {BACKEND} keeps the last"),
            );
        }
        union_fate(ifd, |kind, followed| {
            if !followed {
                self.unread(kind, e.tag, "in a directory the decoder does not walk")
            } else if let Role::Cr3Cmt(n) = self.role {
                cr3_cmt_entry(n, e.tag, kind)
            } else if RAWLER {
                self.rawler_entry(w, ifd, e, kind)
            } else {
                self.rawloader_entry(w, ifd, e, kind)
            }
        })
    }

    fn rawler_entry(&self, w: &Walk<'_>, ifd: &Ifd, e: &Entry, kind: Kind) -> Fate {
        match (kind, e.tag) {
            (Kind::Exif | Kind::Gps | Kind::Interop, _) => Fate::because(
                Disposition::Dropped,
                "parsed by rawler into its EXIF; zenraw reports no EXIF through zencodec (zenraw#19)",
            ),
            // rawler sets orientation only for DNG, from the root IFD
            // (dng.rs:41); every other image is Normal (rawimage.rs:389).
            (_, ORIENTATION) if self.orientation_ifd == Some(ifd.at) => {
                Fate::new(Disposition::Metadata(MetadataKind::Orientation))
            }
            (_, ORIENTATION) => Fate::because(
                Disposition::Dropped,
                "parsed by rawler, which reports Normal orientation here (zenraw#22)",
            ),
            (_, XMP) => Fate::because(
                Disposition::Dropped,
                "parsed by rawler, value unused; zenraw's XMP comes from a byte scan",
            ),
            (_, SONY_OFFSET | SONY_LENGTH | SONY_KEY)
                if self.sony_arw && Self::is_sr2_private(w, ifd) =>
            {
                Fate::because(Disposition::Structure, "locates and decrypts the SR2SubIFD")
            }
            _ => Fate::because(
                Disposition::Structure,
                "parsed by rawler, which reads every tag of the IFDs it walks; its decoders read what they need",
            ),
        }
    }

    fn rawloader_entry(&self, w: &Walk<'_>, ifd: &Ifd, e: &Entry, kind: Kind) -> Fate {
        if !rawloader_knows(e.tag) {
            return self.unread(
                kind,
                e.tag,
                if kind == Kind::Exif {
                    "rawloader skips tags it does not know; zenraw reports no EXIF through zencodec"
                } else {
                    "rawloader skips tags it does not know"
                },
            );
        }
        match e.tag {
            ORIENTATION if self.orientation_ifd == Some(ifd.at) => {
                Fate::new(Disposition::Metadata(MetadataKind::Orientation))
            }
            ORIENTATION => Fate::because(
                Disposition::Dropped,
                "parsed by rawloader; another Orientation entry comes first in its search",
            ),
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
            SONY_OFFSET | SONY_LENGTH | SONY_KEY | DNG_PRIVATE_DATA
                if !(self.sony_arw
                    && (e.tag == DNG_PRIVATE_DATA || Self::is_sr2_private(w, ifd))) =>
            {
                Fate::because(
                    Disposition::Dropped,
                    "parsed by rawloader; only its ARW decoder reads it, from a Sony file's SR2Private IFD",
                )
            }
            _ => Fate::because(
                Disposition::Structure,
                "parsed by rawloader (raw layout, camera parameters); its decoders may read only part of the value (StripOffsets[0], for example), which is not split out",
            ),
        }
    }

    /// Decoded bytes of one row of the DNG raw IFD for the backend's
    /// unpacker, and the image height.
    fn dng_rows(&self, w: &Walk<'_>, ifd: &Ifd) -> Option<(u64, u64)> {
        let one = |tag: u16| {
            w.find(ifd, tag)
                .and_then(|e| w.uints(e, 1).first().copied())
        };
        let width = one(256)?.checked_mul(one(277).unwrap_or(1).max(1))?;
        let height = one(257)?;
        let bits = one(258)?;
        let row = if RAWLER {
            // rawler's PackedDecompressor.
            width.checked_mul(bits)?.div_ceil(8)
        } else {
            // rawloader dng.rs `decode_uncompressed`: 16le, 12be, 10le, 8.
            match bits {
                8 | 10 | 12 | 16 => width.checked_mul(bits)? / 8,
                _ => return None,
            }
        };
        Some((row, height))
    }
}

impl Rules for RawRules<'_> {
    fn magic(&self, magic: u16) -> bool {
        // Olympus ORF ("IIRO"/"IIRS"/"MMOR") and Panasonic RW2 ("IIU\0").
        matches!(magic, 0x4F52 | 0x5352 | 0x0055)
    }

    fn header_extension(&self, w: &Walk<'_>) -> Option<(u64, Fate)> {
        if self.role != Role::Main || w.lay.big || get(w.d, w.base + 8, 2) != Some(b"CR") {
            return None;
        }
        // Both backends' CR2 decoders find the raw IFD by tag (Cr2Id, then
        // CFAPattern), never through this offset.
        Some((
            8,
            Fate::because(
                Disposition::Skipped,
                "CR2 header extension: version and raw IFD offset; the decoder finds the raw IFD by tag",
            ),
        ))
    }

    fn value_tail(&self, w: &Walk<'_>, ifd: &Ifd, index: usize) -> Option<(u64, Fate)> {
        // rawloader's `get_str` stops at the first NUL.
        let e = ifd.entries.get(index)?;
        if RAWLER || e.typ != 2 || self.entry_fate(w, ifd, index).d != Disposition::Structure {
            return None;
        }
        let nul = w.bytes(e)?.iter().position(|&b| b == 0)?;
        Some((
            nul as u64 + 1,
            Fate::because(
                Disposition::Dropped,
                "after the string's NUL, where rawloader's get_str stops",
            ),
        ))
    }

    fn chunk_use(
        &self,
        w: &Walk<'_>,
        ifd: &Ifd,
        tiles: bool,
        offs: &[u64],
        cnts: &[u64],
    ) -> Vec<ChunkUse> {
        let mut out = alloc::vec![ChunkUse::default(); cnts.len()];
        let compression = w
            .find(ifd, COMPRESSION)
            .and_then(|e| w.uints(e, 1).first().copied());
        // Vendor RAWs pack rows their own way and compressed data has no
        // cheap end: not split.
        if !self.dng || tiles || compression != Some(1) || self.raw_ifd != Some(ifd.at) {
            return out;
        }
        let Some((row, height)) = self.dng_rows(w, ifd) else {
            return out;
        };
        let Some(total) = row.checked_mul(height) else {
            return out;
        };
        if !RAWLER {
            // rawloader dng.rs `decode_uncompressed`: every row from
            // StripOffsets[0]; StripByteCounts and later offsets unread.
            let Some(s0) = offs.first().and_then(|&o| w.abs(o)) else {
                return out;
            };
            let region = s0..s0.saturating_add(total);
            for (k, u) in out.iter_mut().enumerate() {
                if k == 0 {
                    u.used = Some(total);
                    u.past_count = true;
                    u.tail = "after the rows rawloader reads";
                    u.note = Some(
                        "rawloader reads every row from the first strip's offset, across the other strips; StripByteCounts are not read"
                            .into(),
                    );
                    continue;
                }
                let start = offs.get(k).and_then(|&o| w.abs(o)).unwrap_or(u64::MAX);
                let end = start.saturating_add(cnts.get(k).copied().unwrap_or(0));
                if start >= region.start && end <= region.end {
                    u.covered = true;
                } else {
                    u.unread = Some(
                        "rawloader reads every row from the first strip's offset and never this strip",
                    );
                }
            }
            return out;
        }
        // rawler `decode_strips`: strips that touch decode as one buffer;
        // otherwise each strip on its own, rows_per_strip rows at a time.
        let contiguous = offs
            .windows(2)
            .zip(cnts.iter())
            .all(|(o, &n)| o[0].checked_add(n) == Some(o[1]));
        if contiguous {
            let mut before = 0u64;
            for (u, &n) in out.iter_mut().zip(cnts.iter()) {
                let used = total.saturating_sub(before).min(n);
                before = before.saturating_add(n);
                if used == 0 {
                    u.unread = Some("past the rows rawler reads from the contiguous strips");
                } else {
                    u.used = Some(used);
                    u.tail = "after the rows rawler reads";
                }
            }
            return out;
        }
        let rps = w
            .find(ifd, 278)
            .and_then(|e| w.uints(e, 1).first().copied())
            .unwrap_or(height)
            .max(1);
        for (k, (u, &n)) in out.iter_mut().zip(cnts.iter()).enumerate() {
            let rows = rps.min(height.saturating_sub((k as u64).saturating_mul(rps)));
            u.used = Some(rows.saturating_mul(row).min(n));
            u.tail = "after the strip's rows rawler reads";
        }
        out
    }

    fn pointer(&self, kind: Kind, e: &Entry) -> Option<(Kind, &'static str)> {
        match (e.tag, kind) {
            (330, _) => Some((Kind::Sub, "SubIFD")),
            (RAF_RAW_SUB_IFD | KODAK_IFD | KDC_IFD, _) => Some((Kind::Sub, "sub-IFD")),
            (34665, _) => Some((Kind::Exif, "EXIF IFD")),
            (GPS_IFD, Kind::Page(_) | Kind::Sub) => Some((Kind::Gps, "GPS IFD")),
            (40965, Kind::Exif) => Some((Kind::Interop, "Interop IFD")),
            // Sony stores a pointer to its SR2Private IFD in DNGPrivateData
            // (a single LONG, or BYTE[4]); DNGs store maker data bytes there.
            (DNG_PRIVATE_DATA, Kind::Page(_) | Kind::Sub)
                if self.sony_arw
                    && ((e.count == 1 && matches!(e.typ, 4 | 13))
                        || (e.count == 4 && matches!(e.typ, 1 | 7))) =>
            {
                Some((Kind::Sub, "SR2Private IFD"))
            }
            _ => None,
        }
    }

    fn pointer_offsets(&self, w: &Walk<'_>, e: &Entry, max: usize) -> Vec<u64> {
        // ARW stores the SR2Private offset as BYTE[4]; both backends read it
        // as a u32 in the file's byte order.
        if e.tag == DNG_PRIVATE_DATA && e.count == 4 && matches!(e.typ, 1 | 7) {
            return w
                .bytes(e)
                .and_then(|b| w.lay.u32(b, 0))
                .map(|v| alloc::vec![u64::from(v)])
                .unwrap_or_default();
        }
        // Every pointer the walk can still queue: both backends follow them
        // all.
        w.uints(e, max)
    }

    fn follows(&self, w: &Walk<'_>, parent: &Ifd, kind: Kind, e: &Entry) -> bool {
        if e.tag == DNG_PRIVATE_DATA {
            return self.sony_arw;
        }
        if RAWLER {
            // Only main-chain IFDs follow pointers; sub-IFDs follow nothing.
            // GenericTiffReader (the file, CMT boxes) follows SubIFDs,
            // ExifIFD and GPSInfo (wellknown_sub_ifd_tags), plus KodakIFD for
            // the CR2 decoder; the RAF decoder's `new_root` follows SubIFDs
            // and ExifIFD in the JPEG's TIFF, and only FujiIFD in the CFA's.
            // rawler follows a sub-IFD tag for a LONG value, and for a type
            // it does not know (IFD, LONG8, IFD8: `Value::Unknown`) at the
            // value's first byte only (formats/tiff/ifd.rs `IFD::new`,
            // entry.rs); an UNDEFINED one takes the value's own offset (not
            // modelled). Other types are not followed.
            let first_byte = || {
                let v = w.uints(e, 1).first().copied();
                let b = w.bytes(e).and_then(|b| b.first().copied()).map(u64::from);
                v.is_some() && v == b
            };
            let usable = e.typ == 4 || (e.typ >= 13 && e.count >= 1 && first_byte());
            if !matches!(kind, Kind::Page(_)) || !usable {
                return false;
            }
            return match (self.role, e.tag) {
                (Role::RafCfa, tag) => tag == RAF_RAW_SUB_IFD,
                (Role::RafJpegExif, tag) => matches!(tag, 330 | 34665),
                (_, 330 | 34665 | GPS_IFD) => true,
                (Role::Main, KODAK_IFD) => self.make_is(&["Canon"]),
                _ => false,
            };
        }
        // rawloader `TiffIFD::new`: five pointer tags from every IFD, to
        // depth 10.
        rawloader_follows(e.tag) && parent.depth < 10
    }

    fn follows_next(&self, _w: &Walk<'_>, _ifd: &Ifd, kind: Kind) -> bool {
        let Kind::Page(n) = kind else {
            return false;
        };
        match self.role {
            // rawler reads one IFD from a CMT box (`new_with_buffer`,
            // max_chained Some(0)).
            Role::Cr3Cmt(_) => false,
            // rawler's RAF decoder: `new_root` / `new_root_with_correction`,
            // chain of 10 after the first IFD.
            Role::RafCfa | Role::RafJpegExif if RAWLER => n < 11,
            // GenericTiffReader: no chain limit.
            _ if RAWLER => true,
            // rawloader `new_root`: at most 100 IFDs.
            _ => n < 99,
        }
    }

    fn cancelled(&self) -> bool {
        self.stop.should_stop()
    }

    fn rejects(&self, w: &Walk<'_>, at: u64, _kind: Kind) -> Option<&'static str> {
        // rawloader `TiffIFD::new` fails for more than 4000 entries
        // (decoders/tiff.rs:197); its caller keeps only the pointer entry.
        let count = w.lay.u16(w.d, at)?;
        (!RAWLER && count > 4000)
            .then_some("rawloader rejects an IFD of more than 4000 entries (decoders/tiff.rs:197)")
    }

    fn ifd(&self, _w: &Walk<'_>, ifd: &Ifd) -> Fate {
        union_fate(ifd, |kind, followed| self.ifd_as(kind, followed))
    }

    fn entry(&self, w: &Walk<'_>, ifd: &Ifd, index: usize) -> Fate {
        self.entry_fate(w, ifd, index)
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
            Fate::because(
                Disposition::ImageData,
                format!("raw sensor data {BACKEND} decodes"),
            )
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
        if !ifd.holds_image() {
            return Vec::new();
        }
        let mut rules = Vec::new();
        if w.magic == 0x55 && ifd.page() == Some(0) {
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
        if self.sony_arw
            && Self::is_sr2_private(w, ifd)
            && w.find(ifd, SONY_OFFSET).is_some()
            && w.find(ifd, SONY_LENGTH).is_some()
        {
            rules.push(ExtentRule {
                offsets: SONY_OFFSET,
                counts: Count::Tag(SONY_LENGTH),
                what: "encrypted SR2SubIFD",
                kind: PartKind::Extent,
                fate: Fate::because(
                    Disposition::Structure,
                    "the ARW decoder decrypts it for white balance and levels",
                ),
            });
        }
        if self.role == Role::RafCfa {
            rules.push(ExtentRule {
                offsets: RAF_OFFSETS,
                counts: if RAWLER {
                    // rawler raf.rs: a view of RafByteCounts bytes.
                    Count::Tag(RAF_BYTE_COUNTS)
                } else {
                    raf_raw_len(w, ifd).map_or(Count::Tag(RAF_BYTE_COUNTS), Count::Bytes)
                },
                what: "raw data",
                kind: PartKind::Extent,
                fate: match (RAWLER, raf_raw_len(w, ifd)) {
                    (false, None) => Fate::because(
                        self.image_data(w, ifd).d,
                        "rawloader sizes the raw data from the CFA header's ImageWidth record; this extent uses RafByteCounts",
                    ),
                    _ => self.image_data(w, ifd),
                },
            });
        }
        rules.extend(default_extents(self, w, ifd));
        rules
    }
}

/// Bytes rawloader's RAF decoder reads (`raf.rs`) for RafImageWidth ×
/// RafImageLength at BitsPerSample (0xF003, default 16): 12-bit packed
/// (`decode_12le`), 14-bit unpacked and 16-bit at two bytes a pixel. Camera
/// hints (`double_width`, `jpeg32`) change this for some models.
fn raf_raw_len(w: &Walk<'_>, ifd: &Ifd) -> Option<u64> {
    let one = |tag: u16| {
        w.find(ifd, tag)
            .and_then(|e| w.uints(e, 1).first().copied())
    };
    let pixels = one(0xF001)?.checked_mul(one(0xF002)?)?;
    match one(0xF003).unwrap_or(16) {
        12 => Some(pixels.checked_mul(12)? / 8),
        14 | 16 => pixels.checked_mul(2),
        _ => None,
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
fn default_extents(rules: &RawRules<'_>, w: &Walk<'_>, ifd: &Ifd) -> Vec<ExtentRule> {
    struct Default<'r, 's>(&'r RawRules<'s>);
    impl Rules for Default<'_, '_> {
        fn follows(&self, w: &Walk<'_>, p: &Ifd, kind: Kind, e: &Entry) -> bool {
            self.0.follows(w, p, kind, e)
        }
        fn follows_next(&self, w: &Walk<'_>, ifd: &Ifd, kind: Kind) -> bool {
            self.0.follows_next(w, ifd, kind)
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

/// One step of the backend's depth-first IFD search: a directory of the
/// walk, or a maker-note IFD found under one (rawloader pushes maker notes
/// into the sub-IFD list at the MakerNote entry's position).
#[derive(Clone, Copy)]
enum Node {
    Ifd(usize),
    Note(u64),
}

/// The followed IFDs in rawloader's depth-first order: each chain IFD, then
/// the sub-IFDs and maker notes its entries lead to (recursively), in entry
/// order. `notes` maps a host IFD's (index, MakerNote entry index) to the
/// maker-note IFD offsets.
fn rawloader_order(w: &Walk<'_>, notes: &[(usize, usize, Vec<u64>)]) -> Vec<Node> {
    let mut children: Vec<Vec<(usize, Vec<Node>)>> = alloc::vec![Vec::new(); w.ifds.len()];
    let mut roots = Vec::new();
    for (i, ifd) in w.ifds.iter().enumerate() {
        if !ifd.followed {
            continue;
        }
        match ifd.parent {
            Some((p, e)) => children[p].push((e, alloc::vec![Node::Ifd(i)])),
            None if ifd.page().is_some() => roots.push(i),
            None => {}
        }
    }
    for (host, e, ats) in notes {
        if let Some(c) = children.get_mut(*host) {
            c.push((*e, ats.iter().map(|&a| Node::Note(a)).collect()));
        }
    }
    for c in &mut children {
        c.sort_by_key(|(e, _)| *e);
    }
    let mut order = Vec::new();
    let mut stack: Vec<Node> = roots.into_iter().rev().map(Node::Ifd).collect();
    while let Some(n) = stack.pop() {
        order.push(n);
        if let Node::Ifd(i) = n {
            for (_, nodes) in children[i].iter().rev() {
                stack.extend(nodes.iter().rev().copied());
            }
        }
    }
    order
}

/// The value of `tag` in `ifd` as a string cut at its NUL (rawloader
/// `get_str`).
fn str_tag(w: &Walk<'_>, ifd: &Ifd, tag: u16) -> Option<String> {
    let b = w.bytes(w.find(ifd, tag)?)?;
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    Some(String::from_utf8_lossy(&b[..end]).into_owned())
}

/// The first value of `tag` in rawloader's depth-first search.
fn first_str(w: &Walk<'_>, order: &[Node], tag: u16) -> Option<String> {
    order.iter().find_map(|n| match *n {
        Node::Ifd(i) => str_tag(w, &w.ifds[i], tag),
        Node::Note(_) => None,
    })
}

/// The IFD the backend decodes, after rawloader's `get_decoder` dispatch:
/// DNG first, then by Make (the first one in its search), each decoder
/// taking the first IFD in depth-first order that carries its tag. rawler
/// builds use the same choice (its per-decoder differences are not
/// modelled).
fn pick_raw_ifd(w: &Walk<'_>, order: &[Node], dng: bool, make: Option<&str>) -> Option<u64> {
    let ifds: Vec<usize> = order
        .iter()
        .filter_map(|n| match *n {
            Node::Ifd(i) => Some(i),
            Node::Note(_) => None,
        })
        .collect();
    let has = |i: usize, tag: u16| w.find(&w.ifds[i], tag).is_some();
    let one = |i: usize, tag: u16| {
        w.find(&w.ifds[i], tag)
            .and_then(|e| w.uints(e, 1).first().copied())
    };
    let first = |tag: u16| ifds.iter().copied().find(|&i| has(i, tag));
    let wide = |tag: u16| {
        ifds.iter()
            .copied()
            .find(|&i| has(i, tag) && one(i, 256).is_some_and(|v| v > 1000))
    };
    let at = |i: Option<usize>| i.map(|i| w.ifds[i].at);
    if dng {
        // dng.rs: IFDs with Compression, not reduced-resolution, of
        // compression 1, 7 or 0x884c.
        return at(ifds.iter().copied().find(|&i| {
            let reduced = one(i, NEW_SUBFILE_TYPE).is_some_and(|v| v & 1 != 0);
            !reduced && matches!(one(i, COMPRESSION), Some(1 | 7 | 0x884c))
        }));
    }
    if first_str(w, order, 0x0110).as_deref() == Some("DCS560C") {
        return at(first(CR2_ID).or_else(|| first(CFA_PATTERN)));
    }
    let Some(make) = make else {
        // mos.rs: Leaf files without a Make, found by Software.
        return if first_str(w, order, 0x0131).as_deref() == Some("Camera Library") {
            at(first(TILE_OFFSETS))
        } else {
            None
        };
    };
    match make {
        "SONY"
        | "OLYMPUS IMAGING CORP."
        | "OLYMPUS CORPORATION"
        | "OLYMPUS OPTICAL CO.,LTD"
        | "SAMSUNG"
        | "PENTAX Corporation"
        | "RICOH IMAGING COMPANY, LTD."
        | "PENTAX" => at(first(STRIP_OFFSETS)),
        "Mamiya-OP Co.,Ltd."
        | "SEIKO EPSON CORP."
        | "Kodak"
        | "NIKON CORPORATION"
        | "EASTMAN KODAK COMPANY"
        | "Eastman Kodak Company" => at(first(CFA_PATTERN)),
        "KODAK" => at(wide(STRIP_OFFSETS)),
        "NIKON" => at(wide(CFA_PATTERN)),
        "Panasonic" | "LEICA" => at(first(PANA_OFFSETS).or_else(|| first(STRIP_OFFSETS))),
        "FUJIFILM" => at(first(RAF_OFFSETS)),
        "Hasselblad" => at(first(WHITE_LEVEL)),
        // cr2.rs: Cr2Id, then CFAPattern; Cr2OldOffset (0x0081) data is not
        // an IFD extent.
        "Canon" => at(first(CR2_ID).or_else(|| first(CFA_PATTERN))),
        // Leaf and Phase One go to the IIQ decoder, which does not use IFD
        // extents; other makes have no decoder.
        _ => None,
    }
}

/// Walk and place one TIFF of the file.
fn place_tiff(
    data: &[u8],
    base: u64,
    limit: u64,
    role: Role,
    xmp: bool,
    stop: &dyn enough::Stop,
    first: Vec<Cand>,
) -> TiffPlacement {
    let mut rules = RawRules {
        role,
        raw_ifd: None,
        orientation_ifd: None,
        xmp,
        dng: false,
        make: None,
        sony_arw: false,
        parsed_notes: Vec::new(),
        stop,
    };
    let mut w = walk(data, base, limit, &rules);
    let probe = rawloader_order(&w, &[]);
    rules.dng = probe
        .iter()
        .any(|n| matches!(*n, Node::Ifd(i) if w.find(&w.ifds[i], DNG_VERSION).is_some()));
    rules.make = if RAWLER {
        // rawler: the first Make of the main chain, trailing spaces trimmed.
        w.ifds
            .iter()
            .filter(|i| i.page().is_some())
            .find_map(|i| str_tag(&w, i, 0x010F))
            .map(|m| String::from(m.trim_end()))
    } else {
        first_str(&w, &probe, 0x010F)
    };
    rules.sony_arw = role == Role::Main && !rules.dng && rules.make_is(&["SONY"]);
    if rules.sony_arw {
        // DNGPrivateData leads to the SR2Private IFD only for Sony ARWs:
        // walk again with that pointer.
        w = walk(data, base, limit, &rules);
    }
    // rawler parses maker notes only in the decoders of these makes.
    let notes_parsed = !RAWLER
        || rules.make_is(&[
            "SONY",
            "SEIKO EPSON CORP.",
            "SAMSUNG",
            "EASTMAN KODAK COMPANY",
            "Eastman Kodak Company",
            "OLYMPUS IMAGING CORP.",
            "OLYMPUS CORPORATION",
            "OLYMPUS OPTICAL CO.,LTD",
            "NIKON CORPORATION",
            "Canon",
            "PENTAX",
            "PENTAX Corporation",
            "RICOH IMAGING COMPANY, LTD.",
            "FUJIFILM",
        ]);
    let notes = if matches!(role, Role::Main | Role::RafJpegExif) && notes_parsed {
        maker_notes(data, limit, &w, &rules)
    } else {
        Vec::new()
    };
    let hosts: Vec<(usize, usize, Vec<u64>)> = notes
        .iter()
        .map(|n| {
            (
                n.host.0,
                n.host.1,
                n.ifds.iter().map(|&(at, _)| at).collect(),
            )
        })
        .collect();
    let order = rawloader_order(&w, &hosts);
    if matches!(role, Role::Main | Role::RafCfa) {
        // A RAF goes to the FUJIFILM decoder whatever its CFA TIFF holds
        // (the Make sits in the JPEG preview's EXIF).
        let make = if role == Role::RafCfa {
            Some("FUJIFILM")
        } else {
            rules.make.as_deref()
        };
        rules.raw_ifd = pick_raw_ifd(&w, &order, rules.dng, make);
    }
    rules.orientation_ifd = if RAWLER {
        // rawler dng.rs:41: the root IFD of a DNG.
        (role == Role::Main && rules.dng)
            .then(|| {
                w.ifds
                    .first()
                    .filter(|i| w.find(i, ORIENTATION).is_some())
                    .map(|i| i.at)
            })
            .flatten()
    } else if matches!(role, Role::Main | Role::RafJpegExif) {
        order.iter().find_map(|n| match *n {
            Node::Ifd(i) => w.find(&w.ifds[i], ORIENTATION).map(|_| w.ifds[i].at),
            Node::Note(at) => notes
                .iter()
                .flat_map(|m| m.ifds.iter())
                .find(|&&(a, orient)| a == at && orient)
                .map(|&(a, _)| a),
        })
    } else {
        None
    };
    rules.parsed_notes = notes.iter().map(|n| n.range.start).collect();
    // Maker-note IFD parts were placed with rules that did not know the
    // orientation IFD yet; place them again now.
    let notes: Vec<MakerNote> = notes
        .into_iter()
        .filter_map(|n| maker_note(data, limit, &w, &rules, n.range.clone(), n.host))
        .collect();
    let p = place(&mut w, &rules, first);
    let parts = if notes.is_empty() {
        p.parts
    } else {
        attach_maker_notes(p.parts, notes)
    };
    TiffPlacement {
        parts,
        logical_end: p.logical_end,
        ifd0_ok: p.ifd0_ok,
        fatal: w.fatal.clone(),
        cancelled: w.cancelled,
        budget_hit: p.budget_hit,
        chain_rejected: !RAWLER
            && w.ifds.iter().any(|i| {
                i.page().is_some_and(|n| n < 100) && w.lay.u16(w.d, i.at).is_some_and(|c| c > 4000)
            }),
    }
}

/// One placed TIFF.
struct TiffPlacement {
    parts: Vec<Cand>,
    logical_end: u64,
    ifd0_ok: bool,
    fatal: Option<String>,
    cancelled: bool,
    /// A budget or cap stopped part of the walk.
    budget_hit: bool,
    /// rawloader rejects a main-chain IFD (more than 4000 entries), so its
    /// `new_root` fails and nothing decodes.
    chain_rejected: bool,
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

/// Details of the maker-note header bytes the backend reads to locate the
/// maker-note IFD.
const NOTE_SIGNATURE: &str = "maker-note signature, read to locate the maker-note IFD; the decode changes only through tags camera decoders read there";
const NOTE_BYTE_ORDER: &str = "maker-note byte order, read to locate the maker-note IFD; the decode changes only through tags camera decoders read there";

/// A maker note rawloader parses as an IFD, walked with its vendor rules.
struct MakerNote {
    /// The MakerNote value's range.
    range: Range<u64>,
    /// Signature and embedded-header parts.
    header: Vec<Cand>,
    /// Parts of the walked maker-note IFDs.
    parts: Vec<Cand>,
    /// The IFD (index in the main walk) and MakerNote entry it hangs from.
    host: (usize, usize),
    /// Each walked maker-note IFD in the backend's search order: offset and
    /// whether it holds an Orientation entry.
    ifds: Vec<(u64, bool)>,
    /// A budget or cap stopped the maker-note walk.
    budget_hit: bool,
}

/// Maker notes in `w` that rawloader parses (decoders/tiff.rs
/// `TiffIFD::new_makernote`), each walked with the same base and byte-order
/// rules rawloader uses.
fn maker_notes(data: &[u8], limit: u64, w: &Walk<'_>, rules: &RawRules<'_>) -> Vec<MakerNote> {
    let mut out = Vec::new();
    for (i, ifd) in w.ifds.iter().enumerate() {
        if !ifd.followed || ifd.problem.is_some() || ifd.depth >= 10 {
            continue;
        }
        let Some(&ei) = ifd.last.get(&MAKER_NOTE) else {
            continue;
        };
        let e = &ifd.entries[ei];
        let tiff_walk::Loc::At(mn, size) = w.loc(e) else {
            continue;
        };
        let end = mn.saturating_add(size).min(limit);
        if mn >= end || out.iter().any(|m: &MakerNote| m.range.start == mn) {
            continue;
        }
        if let Some(note) = maker_note(data, limit, w, rules, mn..end, (i, ei)) {
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
    rules: &RawRules<'_>,
    r: Range<u64>,
    host: (usize, usize),
) -> Option<MakerNote> {
    // rawloader parses a maker note at its host IFD's depth plus one.
    let depth = w.ifds.get(host.0).map_or(0, |i| i.depth.saturating_add(1));
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
        head(mn..mn + 5, Disposition::Structure, NOTE_SIGNATURE);
        head(
            mn + 5..mn + off,
            unread,
            "rest of the Olympus header; rawloader does not read it",
        );
        (mn + off, w.base, le, new_style)
    } else if sig(data, mn, b"PENTAX") {
        let ble = byte_order(data, mn + 8)?;
        head(mn..mn + 6, Disposition::Structure, NOTE_SIGNATURE);
        head(
            mn + 6..mn + 8,
            unread,
            "Pentax header bytes rawloader does not read",
        );
        head(mn + 8..mn + 10, Disposition::Structure, NOTE_BYTE_ORDER);
        (mn + 10, mn, ble, false)
    } else if sig(data, mn, b"Nikon\0\x02") {
        let ble = byte_order(data, mn + 10)?;
        head(mn..mn + 7, Disposition::Structure, NOTE_SIGNATURE);
        head(
            mn + 7..mn + 10,
            unread,
            "maker-note version; rawloader does not read it",
        );
        head(mn + 10..mn + 12, Disposition::Structure, NOTE_BYTE_ORDER);
        head(
            mn + 12..mn + 18,
            unread,
            "embedded TIFF magic and IFD offset; rawloader assumes 8",
        );
        (mn + 18, mn + 10, ble, false)
    } else {
        let mut off = 0;
        if sig(data, mn, b"EPSON") {
            head(mn..mn + 5, Disposition::Structure, NOTE_SIGNATURE);
            head(
                mn + 5..mn + 8,
                unread,
                "rest of the Epson header; rawloader does not read it",
            );
            off = 8;
        } else if sig(data, mn, b"AOC\0") {
            head(mn..mn + 4, Disposition::Structure, NOTE_SIGNATURE);
            off = 4;
        }
        // rawloader checks "II" and then "MM" at the advanced offset.
        let mut ble = le;
        if byte_order(data, mn + off) == Some(true) {
            head(
                mn + off..mn + off + 2,
                Disposition::Structure,
                NOTE_BYTE_ORDER,
            );
            off += 2;
            ble = true;
        }
        if byte_order(data, mn + off) == Some(false) {
            head(
                mn + off..mn + off + 2,
                Disposition::Structure,
                NOTE_BYTE_ORDER,
            );
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
        depth,
        rules,
    );
    let mp = place(&mut mw, rules, Vec::new());
    let mut budget_hit = mp.budget_hit;
    let mut parts = mp.parts;
    let mut ifds: Vec<(u64, bool)> = mw
        .ifds
        .iter()
        .filter(|i| i.followed)
        .map(|i| (i.at, mw.find(i, ORIENTATION).is_some()))
        .collect();
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
                depth.saturating_add(1),
                rules,
            );
            let ip = place(&mut iw, rules, Vec::new());
            budget_hit |= ip.budget_hit;
            parts.extend(ip.parts);
            ifds.extend(
                iw.ifds
                    .iter()
                    .filter(|i| i.followed)
                    .map(|i| (i.at, iw.find(i, ORIENTATION).is_some())),
            );
        }
    }
    Some(MakerNote {
        range: r,
        header,
        parts,
        host,
        ifds,
        budget_hit,
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
        if note.budget_hit {
            host.gap = Some((Disposition::Unknown, tiff_walk::BUDGET_GAP.into()));
        }
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
fn xmp_cand(data: &[u8]) -> Option<Cand> {
    Some(Cand::new(
        xmp_range(data)?,
        PartKind::Field,
        PartTag::Name("XMP packet".into()),
        Disposition::Metadata(MetadataKind::Xmp),
        "first XMP packet zenraw's byte scan finds (xmp.rs extract_xmp)".into(),
    ))
}

/// Why [`inventory`] returned no inventory.
pub(crate) enum Failed {
    /// The part cap.
    Parts(InventoryError),
    /// The stop token fired.
    Stopped,
}

impl From<InventoryError> for Failed {
    fn from(e: InventoryError) -> Self {
        Self::Parts(e)
    }
}

/// Detail for files zenraw's decode rejects outright.
const NOT_RAW: &str = "zenraw does not take this file (is_raw_file is false), so the decode fails";

/// Mark every consumed part of `c` `Skipped`: nothing reads it.
fn not_decoded(c: &mut Cand, why: &str) {
    if c.disp.is_consumed() {
        c.disp = Disposition::Skipped;
        tiff_walk::push_note(&mut c.detail, why);
    }
    for ch in &mut c.children {
        not_decoded(ch, why);
    }
}

/// Detail when rawloader rejects a main-chain IFD.
const CHAIN_REJECTED: &str = "rawloader rejects a main-chain IFD of more than 4000 entries (decoders/tiff.rs:197), so the decode fails";

/// Inventory `data` the way zenraw's zencodec decode job reads it.
pub(crate) fn inventory(
    data: &[u8],
    format: ImageFormat,
    stop: &dyn enough::Stop,
) -> Result<Inventory, Failed> {
    let len = data.len() as u64;
    let xmp = cfg!(feature = "xmp");
    let mut inv = Inventory::new(format, len);
    let mut cancelled = false;
    let mut chain_rejected = false;
    let (mut parts, logical_end, gap) = if data.starts_with(b"FUJIFILM") {
        raf(data, xmp, stop, &mut cancelled)
    } else if data.get(4..8) == Some(b"ftyp") {
        cr3(data, stop, &mut cancelled)
    } else if matches!(data.get(..2), Some(b"II" | b"MM")) {
        let first: Vec<Cand> = xmp_cand(data).into_iter().collect();
        let t = place_tiff(data, 0, len, Role::Main, xmp, stop, first);
        cancelled |= t.cancelled;
        chain_rejected = t.chain_rejected;
        let gap = if t.ifd0_ok && t.budget_hit {
            (
                Disposition::Unknown,
                Some(String::from(tiff_walk::BUDGET_GAP)),
            )
        } else if t.ifd0_ok {
            (Disposition::Unreferenced, None)
        } else {
            (Disposition::Malformed, t.fatal)
        };
        (t.parts, t.logical_end, gap)
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
    if cancelled || stop.should_stop() {
        return Err(Failed::Stopped);
    }
    // BigTIFF and other containers zenraw rejects before any backend runs
    // (zencodec_impl `decoder` → `decode::is_raw_file`).
    if !crate::is_raw_file(data) {
        for c in &mut parts {
            not_decoded(c, NOT_RAW);
        }
    } else if chain_rejected {
        for c in &mut parts {
            not_decoded(c, CHAIN_REJECTED);
        }
    }
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
fn raf(data: &[u8], xmp: bool, stop: &dyn enough::Stop, cancelled: &mut bool) -> Placement {
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
    // Both backends check the magic and read three section offsets (84,
    // 92, 100), never their lengths; rawloader reads nothing else here.
    // Neither backend's decode reads them (rawler's `read_embedded_jpeg`,
    // which reads 84..92, serves thumbnails, not the decode).
    let (unread, why) = (
        Disposition::Skipped,
        "version, camera ID and other header fields; the decode does not read them",
    );
    let length = (
        Disposition::Skipped,
        "section length; the decode reads the section offsets only",
    );
    for (r, d, what) in [
        (0..8, Disposition::Structure, "magic"),
        (8..84, unread, why),
        (84..88, Disposition::Structure, "JPEG preview offset"),
        (88..92, length.0, length.1),
        (92..96, Disposition::Structure, "CFA header offset"),
        (96..100, length.0, length.1),
        (100..104, Disposition::Structure, "CFA section offset"),
        (104..108, length.0, length.1),
        (108..header_end, unread, why),
    ] {
        if r.start < r.end && r.end <= header_end {
            h.children
                .push(Cand::new(r, PartKind::Field, PartTag::None, d, what.into()));
        }
    }
    placed.insert(h);
    let mut logical_end = header_end;
    // Whether the TIFFs hold RafImageWidth (0xF001) and RafWBGRB (0xF00E).
    let mut seen_tags = (false, false);
    let mut note_tags = |parts: &[Cand]| {
        seen_tags.0 |= cands_have(parts, 0xF001);
        seen_tags.1 |= cands_have(parts, 0xF00E);
    };

    if let Some((r, cut)) = section(jpeg_off, jpeg_len, len) {
        logical_end = logical_end.max(r.end);
        let mut c = Cand::new(
            r.clone(),
            PartKind::EmbeddedImage,
            PartTag::None,
            if cut {
                Disposition::Malformed
            } else {
                Disposition::Skipped
            },
            format!(
                "JPEG preview; {BACKEND} reads only the EXIF TIFF inside its APP1 (make, model, orientation)"
            ),
        );
        // FFD8 FFE1 <len> "Exif\0\0" <TIFF>
        if get(data, r.start, 4) == Some(&[0xFF, 0xD8, 0xFF, 0xE1])
            && get(data, r.start + 6, 6) == Some(b"Exif\0\0")
            && r.start + 12 < r.end
        {
            let app1_end = be16(data, r.start + 4)
                .map_or(r.end, |n| (r.start + 4 + n).min(r.end))
                .max(r.start + 12);
            let t = place_tiff(
                data,
                r.start + 12,
                app1_end,
                Role::RafJpegExif,
                xmp,
                stop,
                Vec::new(),
            );
            *cancelled |= t.cancelled;
            note_tags(&t.parts);
            c.children.push(Cand::new(
                r.start + 2..r.start + 12,
                PartKind::Header,
                PartTag::None,
                Disposition::Skipped,
                "APP1 marker, length and Exif signature; the backend starts at the TIFF header 12 bytes in".into(),
            ));
            let mut tiff = Cand::new(
                r.start + 12..app1_end,
                PartKind::Chunk,
                PartTag::None,
                Disposition::Structure,
                format!(
                    "EXIF TIFF in APP1, parsed by {BACKEND}; bytes nothing references are Unreferenced"
                ),
            );
            tiff.body = Some(tiff.range.clone());
            if t.budget_hit {
                tiff.gap = Some((Disposition::Unknown, tiff_walk::BUDGET_GAP.into()));
            }
            tiff.children = t.parts;
            c.children.push(tiff);
        }
        placed.insert(c);
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
            let t = place_tiff(data, r.start, r.end, Role::RafCfa, xmp, stop, Vec::new());
            *cancelled |= t.cancelled;
            note_tags(&t.parts);
            c.children = t.parts;
            if t.budget_hit {
                c.gap = Some((Disposition::Unknown, tiff_walk::BUDGET_GAP.into()));
            }
            if !cut {
                c.body = Some(r.clone());
                c.detail =
                    "RAF CFA section: a TIFF; bytes nothing references are Unreferenced".into();
            }
        } else if !cut {
            c.disp = Disposition::ImageData;
            c.detail = format!("RAF CFA data {BACKEND} decodes (offset at byte 100)");
        }
        placed.insert(c);
    }
    // rawloader's RAF decoder uses the CFA header's ImageWidth only without
    // RafImageWidth, and RafOldWB only without RafWBGRB (raf.rs).
    let uses = (!seen_tags.0, !seen_tags.1);
    if let Some((r, cut)) = section(hdr_off, hdr_len, len) {
        logical_end = logical_end.max(r.end);
        placed.insert(raf_cfa_header(data, r, cut, uses));
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

/// Whether any of `parts` (at any depth) is a field with `tag`.
fn cands_have(parts: &[Cand], tag: u16) -> bool {
    parts.iter().any(|c| {
        (c.kind == PartKind::Field && c.tag == PartTag::Code(u32::from(tag)))
            || cands_have(&c.children, tag)
    })
}

/// RAF CFA header: a big-endian record count, then `tag:u16 len:u16 data`
/// records (rawloader `TiffIFD::new_fuji`, which reads ImageWidth 0x100 and
/// RafOldWB 0x2FF0).
fn raf_cfa_header(data: &[u8], r: Range<u64>, cut: bool, uses: (bool, bool)) -> Cand {
    let mut c = Cand::new(
        r.clone(),
        PartKind::Ifd,
        PartTag::None,
        if cut {
            Disposition::Malformed
        } else {
            Disposition::Structure
        },
        "RAF CFA header records; bytes after the last record are Unreferenced".into(),
    );
    if !cut {
        c.body = Some(r.clone());
    }
    let d = &data[..r.end as usize];
    let Some(count) = be32(d, r.start) else {
        return c;
    };
    let (count_fate, count_why) = if RAWLER || uses.0 || uses.1 {
        (Disposition::Structure, "record count")
    } else {
        (
            Disposition::Dropped,
            "record count, parsed by rawloader; neither record its RAF decoder can use (ImageWidth, RafOldWB) is used for this file",
        )
    };
    c.children.push(Cand::new(
        r.start..r.start + 4,
        PartKind::Field,
        PartTag::None,
        count_fate,
        count_why.into(),
    ));
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
        // rawler `parse_raf_format` keeps eight tags and seeks past the
        // rest; its RAF decoder uses the size (without RawImageFullWidth),
        // crop and X-Trans layout.
        let fate = match tag {
            0x100 if RAWLER && uses.0 => Fate::because(
                Disposition::Structure,
                "image size rawler's RAF decoder uses (no RawImageFullWidth)",
            ),
            0x110 | 0x111 | 0x131 if RAWLER => Fate::because(
                Disposition::Structure,
                "crop or X-Trans layout rawler's RAF decoder uses",
            ),
            0x100 | 0x115 | 0x130 | 0x2FF0 | 0xC000 if RAWLER => Fate::because(
                Disposition::Dropped,
                "parsed by rawler; its RAF decoder does not use it",
            ),
            _ if RAWLER => Fate::because(
                Disposition::Skipped,
                "rawler reads the record header and seeks past its data",
            ),
            0x100 if uses.0 => Fate::because(
                Disposition::Structure,
                "image size rawloader's RAF decoder uses (no RafImageWidth)",
            ),
            0x2FF0 if uses.1 => Fate::because(
                Disposition::Structure,
                "white balance rawloader's RAF decoder uses (no RafWBGRB)",
            ),
            0x100 => Fate::because(
                Disposition::Dropped,
                "parsed by rawloader; its RAF decoder sizes the image from RafImageWidth instead",
            ),
            0x2FF0 => Fate::because(
                Disposition::Dropped,
                "parsed by rawloader; its RAF decoder takes white balance from RafWBGRB instead",
            ),
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

/// `CMT1`..`CMT4`.
fn cmt_number(fourcc: &[u8; 4]) -> Option<u8> {
    match fourcc {
        b"CMT1" => Some(1),
        b"CMT2" => Some(2),
        b"CMT3" => Some(3),
        b"CMT4" => Some(4),
        _ => None,
    }
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
    /// The XMP packet zenraw's byte scan surfaces (rawler builds with the
    /// `xmp` feature).
    xmp: Option<Range<u64>>,
    /// Whether it was placed inside the XMP `uuid`.
    xmp_placed: bool,
    stop: &'a dyn enough::Stop,
    cancelled: bool,
    /// The box budget or depth cap stopped part of the walk.
    stopped: bool,
}

/// Samples mapped per file, so a crafted sample table cannot explode.
const MAX_CR3_SAMPLES: usize = 1 << 16;

/// Which box a CR3 box sits in, for what rawler's BMFF reader
/// (formats/bmff) does with it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parent {
    Top,
    Moov,
    Trak,
    Mdia,
    Minf,
    Stbl,
    /// Canon's metadata `uuid` in `moov` (rawler `Cr3DescBox`).
    Canon,
    /// Boxes rawler does not parse (rawloader builds enumerate them).
    Other,
}

/// What rawler does with a box.
enum Parse {
    /// Reads the payload (a leaf box it parses).
    Whole,
    /// Parses the payload as child boxes.
    Container(Parent),
    /// Reads the header, then the payload has this fate.
    Header(Disposition, &'static str),
    Mdat,
    Thmb,
    Xmp,
    Cmt(u8),
}

/// rawler 0.7's BMFF reader: `FileBox`, `MoovBox`, `TrakBox`, `MdiaBox`,
/// `MinfBox`, `StblBox` and `Cr3DescBox` parse the boxes below; everything
/// else is a `VendorBox`, which seeks past its payload.
fn rawler_parse(parent: Parent, fourcc: &[u8; 4], uuid: Option<[u8; 16]>) -> Parse {
    const SEEK: &str = "rawler reads its header and seeks past it";
    match (parent, fourcc) {
        (Parent::Top, b"ftyp") => Parse::Whole,
        (Parent::Top, b"moov") => Parse::Container(Parent::Moov),
        (Parent::Top, b"mdat") => Parse::Mdat,
        (Parent::Top, b"uuid") if uuid == Some(CR3_XMP_UUID) => Parse::Xmp,
        (Parent::Top | Parent::Canon, b"free" | b"skip") => {
            Parse::Header(Disposition::Padding, "free space")
        }
        (Parent::Moov, b"mvhd") => Parse::Whole,
        (Parent::Moov, b"trak") => Parse::Container(Parent::Trak),
        (Parent::Moov, b"uuid") if uuid == Some(CANON_UUID) => Parse::Container(Parent::Canon),
        // MoovBox reads the header of a uuid box it does not know and does
        // not seek past it: the payload is read as further moov boxes.
        (Parent::Moov, b"uuid") => Parse::Header(
            Disposition::Unknown,
            "rawler does not skip this uuid box and reads its payload as more moov boxes",
        ),
        (Parent::Trak, b"tkhd") => Parse::Whole,
        (Parent::Trak, b"mdia") => Parse::Container(Parent::Mdia),
        (Parent::Mdia, b"mdhd" | b"hdlr") => Parse::Whole,
        (Parent::Mdia, b"minf") => Parse::Container(Parent::Minf),
        (Parent::Minf, b"vmhd") => Parse::Whole,
        (Parent::Minf, b"stbl") => Parse::Container(Parent::Stbl),
        (Parent::Stbl, b"stsd" | b"stts" | b"stsc" | b"stsz" | b"co64") => Parse::Whole,
        (Parent::Canon, b"CNCV" | b"CCTP" | b"CTBO" | b"CNOP") => Parse::Whole,
        (Parent::Canon, b"CMT1") => Parse::Cmt(1),
        (Parent::Canon, b"CMT2") => Parse::Cmt(2),
        (Parent::Canon, b"CMT3") => Parse::Cmt(3),
        (Parent::Canon, b"CMT4") => Parse::Cmt(4),
        (Parent::Canon, b"THMB") => Parse::Thmb,
        (Parent::Canon, _) => Parse::Header(
            Disposition::Malformed,
            "rawler rejects unknown boxes in Canon's metadata uuid; the decode fails",
        ),
        _ => Parse::Header(Disposition::Skipped, SEEK),
    }
}

/// The XMP packet zenraw's byte scan surfaces.
#[cfg(feature = "xmp")]
fn xmp_range(data: &[u8]) -> Option<Range<u64>> {
    let r = crate::xmp::extract_xmp_range(data)?;
    Some(r.start as u64..r.end as u64)
}

#[cfg(not(feature = "xmp"))]
fn xmp_range(_data: &[u8]) -> Option<Range<u64>> {
    None
}

/// CR3 (ISOBMFF): every box, recursing into containers and Canon's metadata
/// `uuid`; the `CMT1`..`CMT4` TIFFs walked; `mdat` split into the samples
/// each `trak` locates (rawler decoders/cr3.rs and formats/bmff).
fn cr3(data: &[u8], stop: &dyn enough::Stop, cancelled: &mut bool) -> Placement {
    let len = data.len() as u64;
    let mut walk = Cr3 {
        data,
        rawler: RAWLER,
        budget: 4096,
        traks: Vec::new(),
        raw_trak: None,
        // Only a decode that succeeds surfaces XMP; rawloader builds do not
        // decode CR3.
        xmp: if RAWLER { xmp_range(data) } else { None },
        xmp_placed: false,
        stop,
        cancelled: false,
        stopped: false,
    };
    let mut boxes = Vec::new();
    let mut at = 0u64;
    let mut logical_end = 0;
    let mut tail = None;
    while at < len {
        match walk.walk_box(at, len, 0, None, Parent::Top) {
            Some(c) => {
                at = c.range.end;
                logical_end = at;
                boxes.push(c);
            }
            None if walk.cancelled => break,
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
    *cancelled |= walk.cancelled;
    walk.map_samples(&mut boxes);
    let mut placed = Placed::default();
    if let Some(xr) = walk.xmp.clone().filter(|_| !walk.xmp_placed) {
        placed.insert(Cand::new(
            xr,
            PartKind::Field,
            PartTag::Name("XMP packet".into()),
            Disposition::Metadata(MetadataKind::Xmp),
            "first XMP packet zenraw's byte scan finds (xmp.rs extract_xmp)".into(),
        ));
    }
    for c in boxes.into_iter().chain(tail) {
        placed.insert(c);
    }
    let gap = if walk.stopped || walk.budget == 0 {
        (
            Disposition::Unknown,
            Some(String::from(tiff_walk::BUDGET_GAP)),
        )
    } else {
        (Disposition::Unreferenced, None)
    };
    (placed.into_sorted(), logical_end, gap)
}

impl Cr3<'_> {
    /// One box at `at` within `end`, with its children. `trak` is the index
    /// of the enclosing `trak`, `parent` what contains it.
    fn walk_box(
        &mut self,
        at: u64,
        end: u64,
        depth: u32,
        trak: Option<usize>,
        parent: Parent,
    ) -> Option<Cand> {
        if self.stop.should_stop() {
            self.cancelled = true;
            return None;
        }
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
        let mut c = Cand::new(
            at..box_end,
            PartKind::Box,
            PartTag::FourCc(fourcc),
            Disposition::Skipped,
            CR3_NOT_DECODED.into(),
        );
        // Under rawloader nothing is decoded; containers are still
        // enumerated.
        let parse = if self.rawler {
            rawler_parse(parent, &fourcc, uuid)
        } else if &fourcc == b"ftyp" {
            Parse::Whole
        } else if is_container(&fourcc) || uuid == Some(CANON_UUID) {
            Parse::Container(if uuid == Some(CANON_UUID) {
                Parent::Canon
            } else {
                Parent::Other
            })
        } else if let Some(n) = cmt_number(&fourcc).filter(|_| parent == Parent::Canon) {
            Parse::Cmt(n)
        } else if &fourcc == b"mdat" {
            Parse::Mdat
        } else {
            Parse::Header(Disposition::Skipped, CR3_NOT_DECODED)
        };
        let header_part = |c: &mut Cand| {
            c.children.push(Cand::new(
                at..payload.start,
                PartKind::Header,
                PartTag::None,
                Disposition::Structure,
                "box header".into(),
            ));
        };
        let mut split: Option<(Disposition, String)> = None;
        let mut children_of: Option<Parent> = None;
        match parse {
            Parse::Whole if &fourcc == b"ftyp" => {
                c.disp = Disposition::Structure;
                c.detail = "brand checked by zenraw's format detection".into();
            }
            Parse::Whole => {
                c.disp = Disposition::Structure;
                c.detail = "parsed by rawler".into();
            }
            Parse::Container(p) => {
                if self.rawler {
                    c.disp = Disposition::Structure;
                    c.detail = "parsed by rawler".into();
                }
                children_of = Some(p);
            }
            Parse::Header(d, why) => {
                if self.rawler {
                    c.disp = Disposition::Structure;
                    c.detail = "box header parsed by rawler".into();
                    split = Some((d, why.into()));
                } else if &fourcc == b"free" || &fourcc == b"skip" {
                    c.disp = Disposition::Padding;
                    c.detail = "free space".into();
                }
            }
            Parse::Mdat => {
                if self.rawler {
                    c.disp = Disposition::Structure;
                    c.detail = "box header; samples the tracks locate are listed below".into();
                }
                if !cut {
                    c.body = Some(payload.clone());
                }
            }
            Parse::Thmb => {
                c.disp = Disposition::Structure;
                c.detail = "box header parsed by rawler".into();
                split = Some((
                    Disposition::Dropped,
                    "thumbnail JPEG rawler parses but does not use".into(),
                ));
            }
            Parse::Xmp => {
                c.disp = Disposition::Structure;
                c.detail = "box header parsed by rawler".into();
                match self.xmp.clone() {
                    Some(xr) if !cut && xr.start >= payload.start && xr.end <= payload.end => {
                        // zenraw's byte scan surfaces the packet in here.
                        self.xmp_placed = true;
                        c.body = Some(at..box_end);
                        header_part(&mut c);
                        // rawler reads the whole payload into cr3xpacket;
                        // zenraw surfaces only the packet its scan finds.
                        for r in [payload.start..xr.start, xr.end..payload.end] {
                            if r.start < r.end {
                                c.children.push(Cand::new(
                                    r,
                                    PartKind::Gap,
                                    PartTag::None,
                                    Disposition::Dropped,
                                    "XMP uuid payload outside the packet zenraw surfaces; rawler reads it into cr3xpacket".into(),
                                ));
                            }
                        }
                        c.children.push(Cand::new(
                            xr,
                            PartKind::Field,
                            PartTag::Name("XMP packet".into()),
                            Disposition::Metadata(MetadataKind::Xmp),
                            "first XMP packet zenraw's byte scan finds (xmp.rs extract_xmp)".into(),
                        ));
                    }
                    _ => {
                        split = Some((
                            Disposition::Dropped,
                            if cfg!(feature = "xmp") {
                                "XMP packet rawler reads into cr3xpacket; zenraw's byte scan surfaces another packet".into()
                            } else {
                                "XMP packet rawler reads into cr3xpacket; nothing surfaces it without the `xmp` feature".into()
                            },
                        ));
                    }
                }
            }
            Parse::Cmt(n) => {
                if !cut {
                    let t = place_tiff(
                        data,
                        payload.start,
                        payload.end,
                        Role::Cr3Cmt(n),
                        false,
                        self.stop,
                        Vec::new(),
                    );
                    self.cancelled |= t.cancelled;
                    c.children = t.parts;
                    c.body = Some(payload.clone());
                    if self.rawler {
                        c.disp = Disposition::Structure;
                        c.detail = format!("CMT{n} TIFF, parsed by rawler");
                    }
                }
            }
        }
        if cut {
            c.disp = Disposition::Malformed;
            c.note("box runs past the end of its parent");
        }
        if let Some((d, n)) = split
            && !cut
            && payload.start < payload.end
        {
            c.body = Some(at..box_end);
            header_part(&mut c);
            c.children.push(Cand::new(
                payload.clone(),
                PartKind::Gap,
                PartTag::None,
                d,
                n,
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
        if let Some(p) = children_of
            && !cut
        {
            c.body = Some(payload.clone());
            let mut child_at = payload.start;
            while child_at < box_end && depth < 16 && self.budget > 0 {
                match self.walk_box(child_at, box_end, depth + 1, trak, p) {
                    Some(ch) => {
                        child_at = ch.range.end;
                        c.children.push(ch);
                    }
                    None if self.cancelled => break,
                    None => {
                        c.children.push(Cand::new(
                            child_at..box_end,
                            PartKind::Gap,
                            PartTag::None,
                            Disposition::Malformed,
                            "box header runs past the end of its parent".into(),
                        ));
                        child_at = box_end;
                        break;
                    }
                }
            }
            // The box budget or depth cap left children unlisted.
            if child_at < box_end && !self.cancelled {
                self.stopped = true;
                c.gap = Some((Disposition::Unknown, tiff_walk::BUDGET_GAP.into()));
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
