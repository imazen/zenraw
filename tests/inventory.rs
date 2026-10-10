//! Structural inventory (`DecodeJob::inventory`) tests: synthetic DNG, RAF
//! and CR3 fixtures (one pinned part by part), the codec-corpus
//! `raw-conformance` set when `ZENRAW_RAW_CONFORMANCE_DIR` names it, a check
//! that bytes marked unread cannot change the decode, and an optional
//! cross-check against `exiftool -v3`.

#![cfg(feature = "zencodec")]

use std::borrow::Cow;
use std::path::PathBuf;

use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::inventory::{Disposition, Inventory, MetadataKind, Part, PartKind, PartTag};
use zenraw::RawDecoderConfig;

fn inventory(data: &[u8]) -> Inventory {
    RawDecoderConfig::new()
        .job()
        .inventory(data)
        .expect("inventory")
        .expect("declared capability")
}

// ── Synthetic TIFF builder ─────────────────────────────────────────────

/// A little-endian classic TIFF assembled byte by byte.
struct W {
    b: Vec<u8>,
}

#[derive(Clone, Copy)]
struct E {
    tag: u16,
    typ: u16,
    count: u32,
    field: [u8; 4],
}

fn short(tag: u16, v: u16) -> E {
    let mut field = [0; 4];
    field[..2].copy_from_slice(&v.to_le_bytes());
    E {
        tag,
        typ: 3,
        count: 1,
        field,
    }
}

fn long(tag: u16, v: u32) -> E {
    E {
        tag,
        typ: 4,
        count: 1,
        field: v.to_le_bytes(),
    }
}

fn at(tag: u16, typ: u16, count: u32, off: u32) -> E {
    E {
        tag,
        typ,
        count,
        field: off.to_le_bytes(),
    }
}

fn inline(tag: u16, typ: u16, count: u32, bytes: &[u8]) -> E {
    let mut field = [0; 4];
    field[..bytes.len()].copy_from_slice(bytes);
    E {
        tag,
        typ,
        count,
        field,
    }
}

fn rational(n: u32, d: u32) -> Vec<u8> {
    [n.to_le_bytes(), d.to_le_bytes()].concat()
}

impl W {
    fn new() -> Self {
        Self {
            b: b"II\x2a\x00\0\0\0\0".to_vec(),
        }
    }

    fn pos(&self) -> u32 {
        self.b.len() as u32
    }

    fn put(&mut self, d: &[u8]) -> u32 {
        let at = self.pos();
        self.b.extend_from_slice(d);
        at
    }

    fn pad(&mut self) {
        if self.b.len() % 2 == 1 {
            self.b.push(0);
        }
    }

    /// Write an IFD (entries sorted by tag); returns its offset.
    fn ifd(&mut self, entries: &[E], next: u32) -> u32 {
        self.pad();
        let at = self.pos();
        self.b
            .extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for e in entries {
            self.b.extend_from_slice(&e.tag.to_le_bytes());
            self.b.extend_from_slice(&e.typ.to_le_bytes());
            self.b.extend_from_slice(&e.count.to_le_bytes());
            self.b.extend_from_slice(&e.field);
        }
        self.b.extend_from_slice(&next.to_le_bytes());
        at
    }
}

const XMP_PACKET: &[u8] = b"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?><x:xmpmeta xmlns:x='adobe:ns:meta/'/><?xpacket end='w'?>";

/// A decodable 4x4 DNG holding every TIFF unit type a RAW carries: a preview
/// IFD0 with its strip, a raw SubIFD, EXIF (with a MakerNote and an Interop
/// pointer), GPS and Interop IFDs, inline and out-of-line values, an XMP
/// packet, a private tag, unreferenced bytes, pad bytes and trailing junk.
fn dng_fixture() -> Vec<u8> {
    let mut w = W::new();
    let preview = w.put(&[200; 12]);
    let bps3 = w.put(&[8, 0, 8, 0, 8, 0]);
    let make = w.put(b"ZenRaw\0");
    w.pad();
    let model = w.put(b"SyntheticDng\0");
    w.pad();
    let xmp = w.put(XMP_PACKET);
    w.pad();
    let private = w.put(b"secret!!");
    w.put(b"orphan!!");
    // Raw CFA data: 4x4 16-bit.
    let raw: Vec<u8> = (0..16u16).flat_map(|i| (i * 4000).to_le_bytes()).collect();
    let raw_at = w.put(&raw);
    let exposure = w.put(&rational(1, 250));
    // A maker note rawloader parses as an empty IFD.
    let maker = w.put(b"\0\0\0\0\0\0ZENR");
    let serial = w.put(b"SN-0042\0");
    let lat = w.put(&[rational(47, 1), rational(36, 1), rational(0, 1)].concat());

    let raw_ifd = w.ifd(
        &[
            long(254, 0),
            short(256, 4),
            short(257, 4),
            short(258, 16),
            short(259, 1),
            short(262, 32803),
            long(273, raw_at),
            short(277, 1),
            long(279, 32),
            inline(33422, 1, 4, &[0, 1, 1, 2]),
            short(50717, u16::MAX),
        ],
        0,
    );
    let interop = w.ifd(&[inline(1, 2, 4, b"R98\0")], 0);
    let exif = w.ifd(
        &[
            at(33434, 5, 1, exposure),
            at(37500, 7, 10, maker),
            long(40965, interop),
            at(42033, 2, 8, serial),
        ],
        0,
    );
    let gps = w.ifd(&[inline(0, 1, 4, &[2, 3, 0, 0]), at(2, 5, 3, lat)], 0);
    let ifd0 = w.ifd(
        &[
            long(254, 1),
            short(256, 2),
            short(257, 2),
            at(258, 3, 3, bps3),
            short(259, 1),
            short(262, 2),
            at(271, 2, 7, make),
            at(272, 2, 13, model),
            long(273, preview),
            short(274, 6),
            short(277, 3),
            long(279, 12),
            long(330, raw_ifd),
            at(700, 7, XMP_PACKET.len() as u32, xmp),
            long(34665, exif),
            long(34853, gps),
            inline(50706, 1, 4, &[1, 4, 0, 0]),
            at(65000, 7, 8, private),
        ],
        0,
    );
    w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    w.put(b"TRAILING junk");
    w.b
}

/// A RAF: header, a JPEG preview whose APP1 holds an EXIF TIFF, CFA header
/// records, and a CFA section that is a TIFF pointing at the raw data.
fn raf_fixture() -> Vec<u8> {
    let mut b = vec![0u8; 148];
    b[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
    b[16..20].copy_from_slice(b"0201");
    b[20..28].copy_from_slice(b"FF129502");
    b[28..36].copy_from_slice(b"X-Test\0\0");
    b[60..64].copy_from_slice(b"0100");
    // JPEG preview with an EXIF APP1: IFD0 {Make, Model, Orientation}.
    let mut tiff = W::new();
    let make = tiff.put(b"FUJIFILM\0");
    tiff.pad();
    let model = tiff.put(b"X-Test\0");
    tiff.pad();
    let ifd0 = tiff.ifd(
        &[at(271, 2, 9, make), at(272, 2, 7, model), short(274, 1)],
        0,
    );
    tiff.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let app1_len = (2 + 6 + tiff.b.len()) as u16;
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&app1_len.to_be_bytes());
    jpeg.extend_from_slice(b"Exif\0\0");
    jpeg.extend_from_slice(&tiff.b);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    let jpeg_off = b.len() as u32;
    b.extend_from_slice(&jpeg);
    b.extend_from_slice(&[0; 6]);
    // CFA header records: ImageWidth (read), one other record (not read).
    let hdr_off = b.len() as u32;
    b.extend_from_slice(&2u32.to_be_bytes());
    b.extend_from_slice(&[0x01, 0x00, 0x00, 0x04, 0x00, 0x04, 0x00, 0x04]);
    b.extend_from_slice(&[0x01, 0x30, 0x00, 0x02, 0xAB, 0xCD]);
    let hdr_len = b.len() as u32 - hdr_off;
    // CFA section: a TIFF whose IFD points at a raw sub-IFD with RafOffsets.
    let cfa_off = b.len() as u32;
    let mut cfa = W::new();
    let raw = cfa.put(&[7u8; 32]);
    let sub = cfa.ifd(
        &[
            short(0xF001, 4),
            short(0xF002, 4),
            short(0xF003, 16),
            long(0xF007, raw),
            long(0xF008, 32),
        ],
        0,
    );
    let top = cfa.ifd(&[long(0xF000, sub)], 0);
    cfa.b[4..8].copy_from_slice(&top.to_le_bytes());
    b.extend_from_slice(&cfa.b);
    let cfa_len = b.len() as u32 - cfa_off;
    for (at, v) in [
        (84, jpeg_off),
        (88, jpeg.len() as u32),
        (92, hdr_off),
        (96, hdr_len),
        (100, cfa_off),
        (104, cfa_len),
    ] {
        b[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
    b
}

/// A CR3-shaped ISOBMFF file: ftyp, moov with Canon's metadata uuid, a free
/// box, mdat, and trailing junk.
fn cr3_fixture() -> Vec<u8> {
    fn bx(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(fourcc);
        v.extend_from_slice(payload);
        v
    }
    let canon_uuid: [u8; 16] = [
        0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a,
        0x48,
    ];
    let mut uuid_payload = canon_uuid.to_vec();
    uuid_payload.extend(bx(b"CNCV", b"CanonCR3_001/00.09.00/00.00.00"));
    uuid_payload.extend(bx(b"CMT1", b"II*\0\x08\0\0\0\0\0\0\0\0\0"));
    let mut moov = bx(b"uuid", &uuid_payload);
    moov.extend(bx(b"mvhd", &[0; 20]));
    let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
    f.extend(bx(b"moov", &moov));
    f.extend(bx(b"free", &[0; 8]));
    f.extend(bx(b"mdat", &[0x5A; 64]));
    f.extend_from_slice(b"junk");
    f
}

/// One expected part: kind, tag, range, disposition, label, depth.
#[cfg(not(feature = "rawler"))]
type Row = (
    PartKind,
    PartTag,
    u64,
    u64,
    Disposition,
    Option<String>,
    usize,
);

#[cfg(not(feature = "rawler"))]
fn rows(inv: &Inventory) -> Vec<Row> {
    fn walk(
        inv: &Inventory,
        parent: Option<zencodec::inventory::PartId>,
        depth: usize,
        out: &mut Vec<Row>,
    ) {
        for id in inv.children(parent) {
            let p = inv.get(id).unwrap();
            out.push((
                p.kind,
                p.tag.clone(),
                p.range.start,
                p.range.end,
                p.disposition,
                p.label.as_ref().map(|l| l.to_string()),
                depth,
            ));
            walk(inv, Some(id), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(inv, None, 0, &mut out);
    out
}

/// `check_inventory` without its "valid input has image data" rule: the
/// file, the file plus junk (which must stay unconsumed) and truncations.
fn check_inventory_without_image_data(data: &[u8]) -> Result<(), String> {
    let run = |d: &[u8]| -> Result<Inventory, String> {
        let inv = inventory(d);
        if inv.input_len() != d.len() as u64 {
            return Err(format!("covers {} of {} bytes", inv.input_len(), d.len()));
        }
        inv.validate()
            .map_err(|e| format!("{} bytes: {e}\n{inv}", d.len()))?;
        Ok(inv)
    };
    run(data)?;
    let mut junked = data.to_vec();
    junked.extend((0..37u8).map(|i| i.wrapping_mul(97) ^ 0x5A));
    let inv = run(&junked)?;
    let mut has_child = vec![false; inv.parts().len()];
    for p in inv.parts() {
        if let Some(id) = p.parent {
            has_child[id.index()] = true;
        }
    }
    for (i, p) in inv.parts().iter().enumerate() {
        if !has_child[i] && p.range.end > data.len() as u64 && p.disposition.is_consumed() {
            return Err(format!(
                "appended junk reported as {}\n{inv}",
                p.disposition
            ));
        }
    }
    let n = data.len();
    let mut lens = vec![0, 1, 2, 3, 4, 8, 16, n.saturating_sub(1)];
    lens.extend((1..8).map(|k| n * k / 8));
    for l in lens.into_iter().filter(|&l| l < n) {
        run(&data[..l])?;
    }
    Ok(())
}

#[test]
fn dng_fixture_decodes_and_passes_check_inventory() {
    let data = dng_fixture();
    let out = zenraw::decode(
        &data,
        &zenraw::RawDecodeConfig::default(),
        &enough::Unstoppable,
    )
    .expect("fixture decodes");
    assert_eq!((out.info.sensor_width, out.info.sensor_height), (4, 4));
    zencodec_testkit::check_inventory(RawDecoderConfig::new(), &data).unwrap();
}

#[test]
fn raf_fixture_passes_check_inventory() {
    let data = raf_fixture();
    zencodec_testkit::check_inventory(RawDecoderConfig::new(), &data).unwrap();
    let inv = inventory(&data);
    // The raw data inside the CFA section's TIFF is the image data.
    assert!(
        inv.parts()
            .iter()
            .any(|p| p.disposition == Disposition::ImageData && p.tag == PartTag::Code(0xF007)),
        "{inv}"
    );
    // The EXIF TIFF inside the JPEG preview is walked: its orientation is
    // what rawloader reports.
    assert!(
        inv.parts()
            .iter()
            .any(|p| p.disposition == Disposition::Metadata(MetadataKind::Orientation)),
        "{inv}"
    );
}

#[test]
fn cr3_fixture_inventory_is_valid() {
    let data = cr3_fixture();
    let inv = inventory(&data);
    inv.validate().unwrap();
    let boxes: Vec<_> = inv
        .parts()
        .iter()
        .filter(|p| p.kind == PartKind::Box)
        .map(|p| p.tag.to_string())
        .collect();
    assert_eq!(
        boxes,
        [
            "ftyp", "moov", "uuid", "CNCV", "CMT1", "mvhd", "free", "mdat"
        ],
        "{inv}"
    );
    if cfg!(feature = "rawler") {
        zencodec_testkit::check_inventory(RawDecoderConfig::new(), &data).unwrap();
    } else {
        // rawloader cannot decode CR3, so nothing is image data.
        check_inventory_without_image_data(&data).unwrap();
        assert!(
            !inv.parts()
                .iter()
                .any(|p| p.disposition == Disposition::ImageData)
        );
    }
}

/// Pinned for the rawloader backend; rawler builds mark every walked entry
/// `Structure` (see `rawler_build_marks_walked_entries_structure`).
#[cfg(not(feature = "rawler"))]
#[test]
fn dng_fixture_part_list_is_pinned() {
    use Disposition::{
        Dropped, ImageData, Metadata as M, Padding, Skipped, Structure, Trailing, Unknown,
        Unreferenced,
    };
    use MetadataKind::{Orientation, Xmp};
    use PartKind::{Extent, Field, Gap, Header, Ifd};
    let c = |t: u32| PartTag::Code(t);
    let n = PartTag::None;
    let data = dng_fixture();
    let inv = inventory(&data);
    inv.validate().unwrap();
    let xmp_value = if cfg!(feature = "xmp") {
        // The packet fills the whole value, so the value is listed as the packet.
        (
            Field,
            PartTag::Name("XMP packet".into()),
            48,
            154,
            M(Xmp),
            None,
            0,
        )
    } else {
        (Field, c(700), 48, 154, Skipped, None, 0)
    };
    let expected: Vec<Row> = vec![
        (Header, n.clone(), 0, 8, Structure, None, 0),
        (Extent, c(273), 8, 20, Skipped, None, 0),
        (Field, c(258), 20, 26, Structure, None, 0),
        (Field, c(271), 26, 33, Structure, None, 0),
        (Gap, n.clone(), 33, 34, Padding, None, 0),
        (Field, c(272), 34, 47, Structure, None, 0),
        (Gap, n.clone(), 47, 48, Padding, None, 0),
        xmp_value,
        (Field, c(65000), 154, 162, Unknown, None, 0),
        (Gap, n.clone(), 162, 170, Unreferenced, None, 0),
        (Extent, c(273), 170, 202, ImageData, None, 0),
        (Field, c(33434), 202, 210, Skipped, None, 0),
        (Field, c(37500), 210, 220, Structure, None, 0),
        (Field, c(42033), 220, 228, Skipped, None, 0),
        (Field, c(2), 228, 252, Skipped, None, 0),
        (Ifd, n.clone(), 252, 390, Structure, None, 0),
        (Field, c(254), 254, 266, Structure, None, 1),
        (Field, c(256), 266, 278, Structure, None, 1),
        (Field, c(257), 278, 290, Structure, None, 1),
        (Field, c(258), 290, 302, Structure, None, 1),
        (Field, c(259), 302, 314, Structure, None, 1),
        (Field, c(262), 314, 326, Structure, None, 1),
        (Field, c(273), 326, 338, Structure, None, 1),
        (Field, c(277), 338, 350, Structure, None, 1),
        (Field, c(279), 350, 362, Structure, None, 1),
        (Field, c(33422), 362, 374, Structure, None, 1),
        (Field, c(50717), 374, 386, Structure, None, 1),
        (Ifd, n.clone(), 390, 408, Skipped, None, 0),
        (Field, c(1), 392, 404, Skipped, None, 1),
        (Ifd, n.clone(), 408, 462, Structure, None, 0),
        (Field, c(33434), 410, 422, Skipped, None, 1),
        (Field, c(37500), 422, 434, Structure, None, 1),
        (Field, c(40965), 434, 446, Skipped, None, 1),
        (Field, c(42033), 446, 458, Skipped, None, 1),
        (Ifd, n.clone(), 462, 492, Skipped, None, 0),
        (Field, c(0), 464, 476, Skipped, None, 1),
        (Field, c(2), 476, 488, Skipped, None, 1),
        (Ifd, n.clone(), 492, 714, Structure, None, 0),
        (Field, c(254), 494, 506, Structure, None, 1),
        (Field, c(256), 506, 518, Structure, None, 1),
        (Field, c(257), 518, 530, Structure, None, 1),
        (Field, c(258), 530, 542, Structure, None, 1),
        (Field, c(259), 542, 554, Structure, None, 1),
        (Field, c(262), 554, 566, Structure, None, 1),
        (Field, c(271), 566, 578, Structure, None, 1),
        (Field, c(272), 578, 590, Structure, None, 1),
        (Field, c(273), 590, 602, Structure, None, 1),
        (Field, c(274), 602, 614, M(Orientation), None, 1),
        (Field, c(277), 614, 626, Structure, None, 1),
        (Field, c(279), 626, 638, Structure, None, 1),
        (Field, c(330), 638, 650, Structure, None, 1),
        (Field, c(700), 650, 662, Dropped, None, 1),
        (Field, c(34665), 662, 674, Structure, None, 1),
        (Field, c(34853), 674, 686, Skipped, None, 1),
        (Field, c(50706), 686, 698, Structure, None, 1),
        (Field, c(65000), 698, 710, Unknown, None, 1),
        (Gap, n.clone(), 714, 727, Trailing, None, 0),
    ];
    let actual = rows(&inv);
    if actual != expected {
        panic!("pinned part list changed:\n{inv}\nactual rows:\n{actual:#?}");
    }
}

// ── raw-conformance corpus ─────────────────────────────────────────────

/// The `raw-conformance` set, when `ZENRAW_RAW_CONFORMANCE_DIR` names it.
fn conformance_files() -> Option<Vec<(PathBuf, bool)>> {
    let dir = PathBuf::from(std::env::var_os("ZENRAW_RAW_CONFORMANCE_DIR")?);
    let mut out = Vec::new();
    for (sub, valid) in [("valid", true), ("invalid", false)] {
        let mut stack = vec![dir.join(sub)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("read {}: {e}", d.display())) {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push((p, valid));
                }
            }
        }
    }
    out.sort();
    Some(out)
}

/// Every `raw-conformance` file passes `check_inventory`; the `invalid/` files
/// and (without the `rawler` backend) the CR3, which nothing in the build
/// decodes, pass it without the image-data rule.
#[test]
fn conformance_inventories_pass_check_inventory() {
    let Some(files) = conformance_files() else {
        eprintln!("ZENRAW_RAW_CONFORMANCE_DIR not set; corpus check not requested");
        return;
    };
    assert_eq!(files.len(), 13, "raw-conformance file count changed");
    let mut failures = Vec::new();
    for (path, valid) in &files {
        let data = std::fs::read(path).unwrap();
        let cr3_without_rawler = data.get(4..12) == Some(b"ftypcrx ") && !cfg!(feature = "rawler");
        let result = if *valid && !cr3_without_rawler {
            zencodec_testkit::check_inventory(RawDecoderConfig::new(), &data)
                .map_err(|e| e.to_string())
        } else {
            check_inventory_without_image_data(&data)
        };
        if let Err(e) = result {
            failures.push(format!("{}: {e}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ── Dispositions against the decoder itself ───────────────────────────

fn decode_summary(data: &[u8]) -> Result<(Vec<u8>, String), String> {
    let out = RawDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(data), &[])
        .and_then(|d| d.decode())
        .map_err(|e| e.to_string())?;
    let pixels = out.pixels().contiguous_bytes().into_owned();
    Ok((pixels, format!("{:?}", out.info())))
}

fn unread(p: &Part) -> bool {
    matches!(
        p.disposition,
        Disposition::Skipped
            | Disposition::Unknown
            | Disposition::Unreferenced
            | Disposition::Padding
            | Disposition::Trailing
    )
}

/// Byte ranges to XOR for each part the inventory marks never read: whole
/// parts with nothing consumed inside them; the uncovered bytes of an unread
/// part that holds consumed parts; an IFD entry's count and value field (the
/// directory parser reads its tag and type to decide to skip it).
fn mutation_targets(inv: &Inventory) -> Vec<(std::ops::Range<u64>, String)> {
    let parts = inv.parts();
    let mut consumed_below = vec![false; parts.len()];
    for i in (0..parts.len()).rev() {
        if let Some(par) = parts[i].parent
            && (parts[i].disposition.is_consumed() || consumed_below[i])
        {
            consumed_below[par.index()] = true;
        }
    }
    let target = |i: usize| unread(&parts[i]) && !consumed_below[i];
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if !unread(p) {
            continue;
        }
        if let Some(par) = p.parent
            && target(par.index())
        {
            continue;
        }
        let what = format!(
            "{} {}..{} {} ({})",
            p.kind.name(),
            p.range.start,
            p.range.end,
            p.disposition,
            p.detail.as_deref().unwrap_or("")
        );
        if consumed_below[i] {
            // Only the bytes no child covers.
            let mut cursor = p.range.start;
            let mut kids: Vec<_> = parts
                .iter()
                .filter(|c| c.parent.is_some_and(|id| id.index() == i))
                .map(|c| c.range.clone())
                .collect();
            kids.sort_by_key(|r| r.start);
            for r in kids {
                if r.start > cursor {
                    out.push((cursor..r.start, format!("uncovered bytes of {what}")));
                }
                cursor = cursor.max(r.end);
            }
            if cursor < p.range.end {
                out.push((cursor..p.range.end, format!("uncovered bytes of {what}")));
            }
            continue;
        }
        let entry = p.kind == PartKind::Field
            && p.parent
                .is_some_and(|id| parts[id.index()].kind == PartKind::Ifd);
        let r = if entry {
            p.range.start + 4..p.range.end
        } else {
            p.range.clone()
        };
        out.push((r, what));
    }
    out
}

/// XOR the bytes of every part marked never read (see [`mutation_targets`]),
/// the largest `per_file` first; the decode must not change.
/// `None` when the file does not decode (nothing to compare against).
fn mutate_unread(name: &str, data: &[u8], per_file: usize) -> Option<(usize, Vec<String>)> {
    let baseline = decode_summary(data).ok()?;
    let inv = inventory(data);
    let mut targets = mutation_targets(&inv);
    targets.sort_by_key(|(r, _)| std::cmp::Reverse(r.end - r.start));
    let mut failures = Vec::new();
    let mut n = 0;
    for (r, what) in targets.into_iter().take(per_file) {
        let mut mutated = data.to_vec();
        for b in &mut mutated[r.start as usize..r.end as usize] {
            *b ^= 0x5A;
        }
        n += 1;
        if decode_summary(&mutated).as_ref() != Ok(&baseline) {
            failures.push(format!("{name}: {what} changed the decode"));
        }
    }
    Some((n, failures))
}

#[test]
fn unread_parts_do_not_influence_decode_fixture() {
    let data = dng_fixture();
    let (n, failures) = mutate_unread("dng_fixture", &data, usize::MAX).expect("fixture decodes");
    assert!(n >= 8, "only {n} parts mutated");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // Sanity: the raw strip does drive the decode.
    let inv = inventory(&data);
    let raw = inv
        .parts()
        .iter()
        .find(|p| p.disposition == Disposition::ImageData)
        .unwrap();
    let mut mutated = data.clone();
    for b in &mut mutated[raw.range.start as usize..raw.range.end as usize] {
        *b ^= 0x5A;
    }
    assert_ne!(decode_summary(&mutated), decode_summary(&data));
}

/// The same check over the conformance RAWs that decode.
#[test]
fn unread_parts_do_not_influence_decode_corpus() {
    let Some(files) = conformance_files() else {
        eprintln!("ZENRAW_RAW_CONFORMANCE_DIR not set; corpus check not requested");
        return;
    };
    let (mut decodable, mut mutated) = (0, 0);
    let mut failures = Vec::new();
    for (path, valid) in files.iter().filter(|(_, v)| *v) {
        let _ = valid;
        let data = std::fs::read(path).unwrap();
        let Some((n, f)) = mutate_unread(&path.display().to_string(), &data, 16) else {
            continue;
        };
        decodable += 1;
        mutated += n;
        failures.extend(f);
    }
    println!("{decodable} decodable files, {mutated} mutated parts");
    assert!(
        failures.is_empty(),
        "{} parts influence the decode:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ── Independent oracle: exiftool -v3 ───────────────────────────────────

/// One unit `exiftool -v3` lists inside a TIFF directory.
#[derive(Debug)]
enum OracleUnit {
    /// A directory: name, declared entry count, (index, dumped offset) of
    /// its first inline value, and whether it belongs to a TIFF embedded in
    /// another TIFF's value.
    Dir(String, u64, Option<(u64, u64)>, bool),
    /// A tag value: directory, tag, byte count, dumped offset, embedded.
    Value(String, u16, u64, u64, bool),
}

/// Directories exiftool names for TIFF IFDs whose dumped offsets are
/// relative to a TIFF header (not MakerNotes, ICC, XMP, decrypted SR2SubIFD).
fn is_tiff_dir(name: &str) -> bool {
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    name.strip_prefix("IFD")
        .is_some_and(|r| !r.is_empty() && digits(r))
        || name.strip_prefix("SubIFD").is_some_and(digits)
        || matches!(name, "ExifIFD" | "GPS" | "InteropIFD" | "FujiIFD" | "SR2")
}

/// Parse `exiftool -v3` output into the units of TIFF directories.
fn parse_exiftool_v3(text: &str) -> Vec<OracleUnit> {
    let mut out = Vec::new();
    // (depth, unit index or usize::MAX for non-TIFF dirs, embedded)
    let mut stack: Vec<(usize, usize, bool)> = Vec::new();
    let mut entry_index: Option<u64> = None;
    let mut pending: Option<(u16, u64)> = None;
    // A second IFD0 starts another TIFF (one stored in a value, such as an
    // RW2's JPEG-from-raw); it and every directory after it belong to it.
    let mut ifd0_seen = false;
    let mut second_tiff = false;
    for line in text.lines() {
        let mut rest = line.strip_prefix("  ").unwrap_or(line);
        let mut depth = 0;
        while let Some(r) = rest.strip_prefix("| ") {
            rest = r;
            depth += 1;
        }
        let body = rest.trim_start();
        if let Some(dir) = body.strip_prefix("+ [") {
            let name = dir.split_whitespace().next().unwrap_or("").to_string();
            let n = dir
                .split(" with ")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            stack.retain(|(d, _, _)| *d < depth);
            let inside_tiff = stack.iter().any(|&(_, i, _)| i != usize::MAX);
            let parent_embedded = stack.last().is_some_and(|&(_, _, e)| e);
            // A new IFD0 under another TIFF's directory is a TIFF stored in a
            // value (a JPEG preview's EXIF, for example).
            if name == "IFD0" {
                second_tiff |= ifd0_seen;
                ifd0_seen = true;
            }
            let embedded =
                second_tiff || parent_embedded || (inside_tiff && name.starts_with("IFD"));
            if is_tiff_dir(&name) {
                out.push(OracleUnit::Dir(name, n, None, embedded));
                stack.push((depth, out.len() - 1, embedded));
            } else {
                stack.push((depth, usize::MAX, embedded));
            }
            pending = None;
            continue;
        }
        let Some(&(dir_depth, dir_idx, embedded)) = stack.iter().rev().find(|(d, _, _)| *d < depth)
        else {
            continue;
        };
        if dir_depth + 1 != depth || dir_idx == usize::MAX {
            continue;
        }
        if let Some((idx, _)) = body.split_once(')')
            && let Ok(i) = idx.parse::<u64>()
        {
            entry_index = Some(i);
            pending = None;
            continue;
        }
        if let Some(t) = body.strip_prefix("- Tag 0x") {
            let tag = t
                .get(..4)
                .and_then(|h| u16::from_str_radix(h, 16).ok())
                .unwrap_or(0);
            let size = t
                .split('(')
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            pending = Some((tag, size));
            continue;
        }
        if let Some((tag, size)) = pending.take()
            && let Some((hex, _)) = body.split_once(':')
            && let Ok(off) = u64::from_str_radix(hex.trim(), 16)
        {
            let OracleUnit::Dir(name, _, first, _) = &mut out[dir_idx] else {
                unreachable!()
            };
            let name = name.clone();
            if first.is_none()
                && size <= 4
                && let Some(i) = entry_index
            {
                *first = Some((i, off));
            }
            out.push(OracleUnit::Value(name, tag, size, off, embedded));
        }
    }
    out
}

/// How one file's units compare: matched, explained (with why), unexplained.
#[derive(Default)]
struct OracleTally {
    matched: usize,
    explained: Vec<String>,
    unexplained: Vec<String>,
}

/// Compare an inventory with exiftool's units. exiftool dumps offsets
/// relative to the TIFF header that owns the directory, so each unit is
/// tried against every TIFF header part the inventory lists.
fn oracle_compare(inv: &Inventory, units: &[OracleUnit]) -> OracleTally {
    let parts = inv.parts();
    // exiftool dumps offsets relative to the TIFF header, or to the start
    // of the stream holding it (a RAF's JPEG preview): try the file start,
    // every TIFF header and every part enclosing one.
    let mut headers: Vec<(u64, bool)> = vec![(0, false)];
    for p in parts.iter().filter(|p| p.kind == PartKind::Header) {
        let big = match p.detail.as_deref() {
            Some(d) if d.starts_with("TIFF header") => false,
            Some(d) if d.starts_with("BigTIFF header") => true,
            _ => continue,
        };
        headers.push((p.range.start, big));
        let mut up = p.parent;
        while let Some(id) = up {
            headers.push((parts[id.index()].range.start, big));
            up = parts[id.index()].parent;
        }
    }
    if parts.first().is_some_and(|p| {
        p.detail
            .as_deref()
            .is_some_and(|d| d.starts_with("BigTIFF"))
    }) {
        headers[0].1 = true;
    }
    headers.sort_unstable();
    headers.dedup();
    let mut child_count = vec![0u64; parts.len()];
    for p in parts {
        if let Some(id) = p.parent {
            child_count[id.index()] += 1;
        }
    }
    let ifd_at = |at: Option<u64>, n: u64| {
        parts.iter().enumerate().any(|(i, p)| {
            p.kind == PartKind::Ifd && at.is_none_or(|a| a == p.range.start) && child_count[i] == n
        })
    };
    let mut t = OracleTally::default();
    for u in units {
        let ok = headers.iter().any(|&(base, big)| {
            let (field_off, count_len, entry_len, inline_cap) =
                if big { (12, 8, 20, 8) } else { (8, 2, 12, 4) };
            match u {
                OracleUnit::Dir(_, n, Some((i, off)), _) => {
                    match (base + off).checked_sub(field_off + count_len + i * entry_len) {
                        Some(at) => ifd_at(Some(at), *n),
                        None => false,
                    }
                }
                OracleUnit::Dir(_, n, None, _) => ifd_at(None, *n),
                OracleUnit::Value(_, tag, size, off, _) => {
                    let code = PartTag::Code(u32::from(*tag));
                    let at = base + off;
                    if *size <= inline_cap {
                        parts.iter().any(|p| {
                            p.parent.is_some() && p.tag == code && p.range.start + field_off == at
                        })
                    } else {
                        parts
                            .iter()
                            .any(|p| p.tag == code && p.range.start == at && p.len() == *size)
                    }
                }
            }
        });
        if ok {
            t.matched += 1;
            continue;
        }
        let (what, embedded, off) = match u {
            OracleUnit::Dir(name, n, first, e) => {
                (format!("{name} ({n} entries)"), *e, first.map(|(_, o)| o))
            }
            OracleUnit::Value(dir, tag, size, off, e) => (
                format!("{dir} tag {tag:#06x} ({size} bytes at {off})"),
                *e,
                Some(*off),
            ),
        };
        if embedded {
            t.explained.push(format!(
                "{what}: TIFF stored inside an opaque value the decoder does not parse"
            ));
            continue;
        }
        let split = off.is_some_and(|off| {
            headers.iter().any(|&(base, _)| {
                parts.iter().any(|p| {
                    p.range.start <= base + off
                        && base + off < p.range.end
                        && p.detail.as_deref().is_some_and(|d| d.contains("overlaps"))
                })
            })
        });
        if split {
            t.explained
                .push(format!("{what}: overlaps another part; listed split"));
        } else {
            t.unexplained.push(what);
        }
    }
    t
}

/// Run exiftool on each file and compare; returns the markdown table rows
/// and every unexplained mismatch.
fn run_oracle(
    tool: &std::ffi::OsStr,
    files: &[(String, Vec<u8>)],
) -> (Vec<String>, Vec<String>, usize, usize) {
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    let (mut checked, mut units_total) = (0, 0);
    for (name, data) in files {
        let path = PathBuf::from(name);
        let out = std::process::Command::new(tool)
            .arg("-v3")
            .arg(&path)
            .output()
            .unwrap_or_else(|e| panic!("run {}: {e}", tool.to_string_lossy()));
        let units = parse_exiftool_v3(&String::from_utf8_lossy(&out.stdout));
        if units.is_empty() {
            continue;
        }
        checked += 1;
        units_total += units.len();
        let t = oracle_compare(&inventory(data), &units);
        let short = path.file_name().unwrap().to_string_lossy().into_owned();
        rows.push(format!(
            "| {short} | {} | {} | {} | {} |",
            units.len(),
            t.matched,
            t.explained.len(),
            t.unexplained.len()
        ));
        for e in t.explained.iter().take(3) {
            println!("explained {short}: {e}");
        }
        for u in t.unexplained {
            failures.push(format!("{short}: {u}"));
        }
    }
    (rows, failures, checked, units_total)
}

/// Every directory and tag value `exiftool -v3` lists for the TIFF-based
/// `raw-conformance` files appears in the inventory at the same offset and
/// length, or the mismatch is explained. Runs when both
/// `INVENTORY_ORACLE_EXIFTOOL` and `ZENRAW_RAW_CONFORMANCE_DIR` are set
/// (`just inventory-oracle`).
#[test]
fn exiftool_oracle_agrees() {
    let (Some(tool), Some(files)) = (
        std::env::var_os("INVENTORY_ORACLE_EXIFTOOL"),
        conformance_files(),
    ) else {
        eprintln!(
            "INVENTORY_ORACLE_EXIFTOOL / ZENRAW_RAW_CONFORMANCE_DIR not set; oracle cross-check not requested"
        );
        return;
    };
    let files: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .filter(|(_, valid)| *valid)
        .map(|(p, _)| (p.display().to_string(), std::fs::read(&p).unwrap()))
        // exiftool dumps CR3's CMT TIFFs relative to their boxes; the CR3
        // walk lists boxes, not those TIFFs.
        .filter(|(_, d)| d.get(4..8) != Some(b"ftyp"))
        .collect();
    let (rows, failures, checked, units) = run_oracle(&tool, &files);
    println!("| file | units | matched | explained | unexplained |");
    println!("|---|---|---|---|---|");
    for r in &rows {
        println!("{r}");
    }
    println!("{checked} files, {units} units compared");
    assert_eq!(
        checked, 7,
        "every TIFF-based conformance RAW has TIFF directories"
    );
    assert!(
        failures.is_empty(),
        "{} unexplained mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ── Findings the inventory encodes ─────────────────────────────────────

/// zenraw advertises `with_exif(true)` but no EXIF reaches `ImageInfo`, so
/// the inventory never reports EXIF as surfaced. With the `xmp` feature it
/// attaches XMP without declaring `with_xmp`. If either changes, update
/// `RawRules` and this test together.
#[test]
fn exif_is_advertised_but_never_surfaced() {
    let caps = <RawDecoderConfig as DecoderConfig>::capabilities();
    assert!(caps.exif());
    assert!(!caps.xmp());
    let data = dng_fixture();
    let out = RawDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(&data), &[])
        .and_then(|d| d.decode())
        .unwrap();
    assert!(out.info().embedded_metadata.exif.is_none());
    assert_eq!(
        out.info().embedded_metadata.xmp.is_some(),
        cfg!(feature = "xmp")
    );
    let inv = inventory(&data);
    assert!(
        !inv.parts()
            .iter()
            .any(|p| p.disposition == Disposition::Metadata(MetadataKind::Exif))
    );
}

/// rawler keeps every tag of the IFDs it walks, so a rawler build reports
/// each entry of the walked IFDs as consumed, while GPS and Interop IFDs stay
/// unread.
#[cfg(feature = "rawler")]
#[test]
fn rawler_build_marks_walked_entries_structure() {
    let data = dng_fixture();
    let inv = inventory(&data);
    inv.validate().unwrap();
    for p in inv.parts().iter().filter(|p| p.kind == PartKind::Ifd) {
        let walked = p.disposition == Disposition::Structure;
        let gps_or_interop = p
            .detail
            .as_deref()
            .is_some_and(|d| d.starts_with("GPS") || d.starts_with("Interop"));
        assert_eq!(walked, !gps_or_interop, "{inv}");
        for c in inv
            .parts()
            .iter()
            .filter(|c| c.parent.is_some_and(|id| inv.get(id) == Some(p)))
        {
            assert_eq!(c.disposition.is_consumed(), walked, "{inv}");
        }
    }
}
