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

/// A CR3-shaped ISOBMFF file: ftyp; moov with Canon's metadata uuid (CNCV,
/// a CCTP naming track 1 as the raw image, CMT1), mvhd and one trak whose
/// sample tables locate a CRAW sample in mdat; a free box; mdat; trailing
/// junk.
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
    let build = |sample_at: u64| -> Vec<u8> {
        let mut uuid_payload = canon_uuid.to_vec();
        uuid_payload.extend(bx(b"CNCV", b"CanonCR3_001/00.09.00/00.00.00"));
        // CCTP: version/flags, two u32s, then one CCDT: image type 0 (the
        // full raw image), dual pixel 0, track 1.
        let mut cctp = vec![0u8; 12];
        let mut ccdt = 0u64.to_be_bytes().to_vec();
        ccdt.extend_from_slice(&0u32.to_be_bytes());
        ccdt.extend_from_slice(&1u32.to_be_bytes());
        cctp.extend(bx(b"CCDT", &ccdt));
        uuid_payload.extend(bx(b"CCTP", &cctp));
        uuid_payload.extend(bx(b"CMT1", b"II*\0\x08\0\0\0\0\0\0\0\0\0"));
        // stsd with one CRAW entry; one 64-byte sample in one chunk.
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(bx(b"CRAW", &[0; 8]));
        let mut stsc = vec![0, 0, 0, 0, 0, 0, 0, 1];
        for v in [1u32, 1, 1] {
            stsc.extend_from_slice(&v.to_be_bytes());
        }
        let mut stsz = vec![0, 0, 0, 0];
        stsz.extend_from_slice(&64u32.to_be_bytes());
        stsz.extend_from_slice(&1u32.to_be_bytes());
        let mut co64 = vec![0, 0, 0, 0, 0, 0, 0, 1];
        co64.extend_from_slice(&sample_at.to_be_bytes());
        let mut stbl = bx(b"stsd", &stsd);
        stbl.extend(bx(b"stts", &[0; 8]));
        stbl.extend(bx(b"stsc", &stsc));
        stbl.extend(bx(b"stsz", &stsz));
        stbl.extend(bx(b"co64", &co64));
        let mut minf = bx(b"vmhd", &[0; 12]);
        minf.extend(bx(b"dinf", &bx(b"dref", &[0; 8])));
        minf.extend(bx(b"stbl", &stbl));
        let mut mdia = bx(b"mdhd", &[0; 24]);
        mdia.extend(bx(b"hdlr", &[0; 24]));
        mdia.extend(bx(b"minf", &minf));
        let mut trak = bx(b"tkhd", &[0; 84]);
        trak.extend(bx(b"mdia", &mdia));
        let mut moov = bx(b"uuid", &uuid_payload);
        moov.extend(bx(b"mvhd", &[0; 20]));
        moov.extend(bx(b"trak", &trak));
        let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        f.extend(bx(b"moov", &moov));
        f.extend(bx(b"free", &[0; 8]));
        f.extend(bx(b"mdat", &[0x5A; 64]));
        f.extend_from_slice(b"junk");
        f
    };
    let probe = build(0);
    let mdat_payload = probe.len() as u64 - 4 - 64;
    build(mdat_payload)
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
    // what rawloader reports; rawler reports Normal for RAF (zenraw#22).
    let orientation = inv
        .parts()
        .iter()
        .find(|p| p.tag == PartTag::Code(274))
        .unwrap_or_else(|| panic!("{inv}"));
    let expected = if cfg!(feature = "rawler") {
        Disposition::Dropped
    } else {
        Disposition::Metadata(MetadataKind::Orientation)
    };
    assert_eq!(orientation.disposition, expected, "{inv}");
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
    // rawler seeks past dinf; rawloader builds enumerate its dref.
    let mut expected = vec![
        "ftyp", "moov", "uuid", "CNCV", "CCTP", "CMT1", "mvhd", "trak", "tkhd", "mdia", "mdhd",
        "hdlr", "minf", "vmhd", "dinf", "stbl", "stsd", "stts", "stsc", "stsz", "co64", "free",
        "mdat",
    ];
    if !cfg!(feature = "rawler") {
        expected.insert(15, "dref");
    }
    assert_eq!(boxes, expected, "{inv}");
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
        xmp_value.clone(),
        (Field, c(65000), 154, 162, Unknown, None, 0),
        (Gap, n.clone(), 162, 170, Unreferenced, None, 0),
        (Extent, c(273), 170, 202, ImageData, None, 0),
        (Field, c(33434), 202, 210, Skipped, None, 0),
        (Field, c(37500), 210, 220, Structure, None, 0),
        (Ifd, n.clone(), 210, 216, Structure, None, 1),
        (Gap, n.clone(), 216, 220, Unreferenced, None, 1),
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
        // Printed in the form above, to re-pin after checking each change.
        let listing: String = actual
            .iter()
            .map(|(k, t, s, e, d, l, depth)| {
                let tag = match t {
                    PartTag::Code(c) => format!("c({c})"),
                    PartTag::None => "n.clone()".into(),
                    t => format!("{t:?}"),
                };
                let disp = match d {
                    M(m) => format!("M({m:?})"),
                    d => format!("{d:?}"),
                };
                format!("        ({k:?}, {tag}, {s}, {e}, {disp}, {l:?}, {depth}),\n")
            })
            .collect();
        panic!("pinned part list changed:\n{inv}\nactual rows:\n{listing}");
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

/// What a zencodec decode returns: pixels, the reported `ImageInfo`, and the
/// XMP bytes in full.
type Summary = (Vec<u8>, String, Option<Vec<u8>>);

fn decode_summary(data: &[u8]) -> Result<Summary, String> {
    let out = RawDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(data), &[])
        .and_then(|d| d.decode())
        .map_err(|e| e.to_string())?;
    let pixels = out.pixels().contiguous_bytes().into_owned();
    let xmp = out
        .info()
        .embedded_metadata
        .xmp
        .as_ref()
        .map(|x| x.to_vec());
    Ok((pixels, format!("{:?}", out.info()), xmp))
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

/// `unread`, or `Dropped` too: for the DNG fixture, whose dropped parts
/// (EXIF and GPS values in rawler builds, the XMP entry) cannot turn valid
/// under a mutation.
fn unread_or_dropped(p: &Part) -> bool {
    unread(p) || p.disposition == Disposition::Dropped
}

/// Byte ranges to XOR for each part the inventory marks never read: whole
/// parts with nothing consumed inside them; the uncovered bytes of an unread
/// part that holds consumed parts; an IFD entry's count and value field (the
/// directory parser reads its tag and type to decide to skip it).
fn mutation_targets(
    inv: &Inventory,
    unread: fn(&Part) -> bool,
) -> Vec<(std::ops::Range<u64>, String)> {
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
        // A dropped entry is parsed: both backends slice its value from the
        // count and offset, so corrupting them can fail the parse. Only
        // dropped values are mutated.
        if entry && p.disposition == Disposition::Dropped {
            continue;
        }
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
fn mutate_unread(
    name: &str,
    data: &[u8],
    per_file: usize,
    unread: fn(&Part) -> bool,
) -> Option<(usize, Vec<String>)> {
    let baseline = decode_summary(data).ok()?;
    let inv = inventory(data);
    let mut targets = mutation_targets(&inv, unread);
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
    let (n, failures) = mutate_unread("dng_fixture", &data, usize::MAX, unread_or_dropped)
        .expect("fixture decodes");
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
        let Some((n, f)) = mutate_unread(&path.display().to_string(), &data, 16, unread) else {
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

/// Details that name why a read part leaves this decode unchanged.
const EXPLAINED: &[&str] = &[
    // Known tags both backends parse; which ones a camera decoder reads is
    // per make and not modelled.
    "its decoders may read only part",
    "its decoders read what they need",
    "camera decoders read",
];

/// Leaf parts reported as read (`Structure`, `Metadata`, `ImageData`), and
/// whether changing each changes the decode (masks, then decrements),
/// unless its detail says why not. Entries are mutated in their value field.
fn mutate_read(name: &str, data: &[u8], per_file: usize) -> Option<(usize, usize, Vec<String>)> {
    let base = decode_summary(data).ok()?;
    let inv = inventory(data);
    let parts = inv.parts();
    let mut has_child = vec![false; parts.len()];
    for p in parts {
        if let Some(id) = p.parent {
            has_child[id.index()] = true;
        }
    }
    let leaves: Vec<&Part> = parts
        .iter()
        .enumerate()
        .filter(|&(i, p)| p.disposition.is_consumed() && !has_child[i])
        .map(|(_, p)| p)
        .collect();
    let explained = |p: &Part| {
        let d = p.detail.as_deref().unwrap_or("");
        EXPLAINED.iter().any(|x| d.contains(x))
    };
    let exempt = leaves.iter().filter(|p| explained(p)).count();
    let targets: Vec<&Part> = leaves.into_iter().filter(|p| !explained(p)).collect();
    let step = targets.len().div_ceil(per_file).max(1);
    let mut failures = Vec::new();
    let mut n = 0;
    for p in targets.into_iter().step_by(step) {
        // IFD entries (12 bytes) in their value field; RAF records whole.
        let entry = p.kind == PartKind::Field
            && p.len() == 12
            && p.parent.is_some_and(|id| {
                parts[id.index()].kind == PartKind::Ifd
                    && !parts[id.index()]
                        .detail
                        .as_deref()
                        .is_some_and(|d| d.starts_with("RAF CFA header"))
            });
        let (s, e) = if entry {
            (p.range.start + 8, p.range.end)
        } else {
            (p.range.start, p.range.end)
        };
        n += 1;
        // Whole-range masks, then single bytes (both ends, and the second
        // byte, where a big-endian SHORT keeps its low byte).
        let mut singles = vec![(s..s + 1, 0x01), (s..s + 1, 0x07), (e - 1..e, 0x01)];
        if e - s >= 2 {
            singles.extend([(s + 1..s + 2, 0x01), (s + 1..s + 2, 0x07)]);
        }
        let mut variants: Vec<Vec<u8>> = [(s..e, 0x5Au8), (s..e, 0x01)]
            .into_iter()
            .chain(singles)
            .map(|(r, m)| xor(data, r, m))
            .collect();
        for le in [true, false] {
            let mut m = data.to_vec();
            let bytes = &mut m[s as usize..e as usize];
            let order: Vec<usize> = if le {
                (0..bytes.len()).collect()
            } else {
                (0..bytes.len()).rev().collect()
            };
            for i in order {
                let (v, borrow) = bytes[i].overflowing_sub(1);
                bytes[i] = v;
                if !borrow {
                    break;
                }
            }
            variants.push(m);
        }
        if !variants
            .iter()
            .any(|m| decode_summary(m).as_ref() != Ok(&base))
        {
            failures.push(format!(
                "{name}: {} {s}..{e} {} ({}) left the decode unchanged",
                p.kind.name(),
                p.disposition,
                p.detail.as_deref().unwrap_or("")
            ));
        }
    }
    Some((n, exempt, failures))
}

/// The reverse of the unread-parts checks: a part reported as read changes
/// the decode when changed, or its detail says why it does not. The DNG
/// fixture and every decodable raw-conformance file (16 parts each).
#[test]
fn read_parts_influence_decode() {
    let mut inputs: Vec<(String, Vec<u8>)> = vec![("dng_fixture".into(), dng_fixture())];
    if let Some(files) = conformance_files() {
        inputs.extend(
            files
                .into_iter()
                .filter(|(_, v)| *v)
                .map(|(p, _)| (p.display().to_string(), std::fs::read(&p).unwrap())),
        );
    }
    let (mut files, mut mutated, mut exempt) = (0, 0, 0);
    let mut failures = Vec::new();
    for (name, data) in &inputs {
        let Some((n, x, f)) = mutate_read(name, data, 16) else {
            continue;
        };
        files += 1;
        mutated += n;
        exempt += x;
        failures.extend(f);
    }
    println!(
        "{files} decodable files, {mutated} read parts mutated, {exempt} explained and not mutated"
    );
    assert!(
        failures.is_empty(),
        "{} read parts do not influence the decode:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ── Independent oracle: exiftool -v3 ───────────────────────────────────

/// One unit `exiftool -v3` lists inside a TIFF directory.
#[derive(Debug)]
enum OracleUnit {
    /// A directory: name, declared entry count, (index, dumped offset) of
    /// its first inline value, whether it belongs to a TIFF embedded in
    /// another TIFF's value, and its entries' tags in order.
    Dir(String, u64, Option<(u64, u64)>, bool, Vec<u16>),
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
                out.push(OracleUnit::Dir(name, n, None, embedded, Vec::new()));
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
            if let OracleUnit::Dir(_, _, _, _, tags) = &mut out[dir_idx] {
                tags.push(tag);
            }
            continue;
        }
        if let Some((tag, size)) = pending.take()
            && let Some((hex, _)) = body.split_once(':')
            && let Ok(off) = u64::from_str_radix(hex.trim(), 16)
        {
            let OracleUnit::Dir(name, _, first, _, _) = &mut out[dir_idx] else {
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
    // Each IFD's entry tags, in file order.
    let mut entry_tags: Vec<Vec<(u64, u16)>> = vec![Vec::new(); parts.len()];
    for p in parts {
        if let Some(id) = p.parent
            && parts[id.index()].kind == PartKind::Ifd
            && p.kind == PartKind::Field
            && let PartTag::Code(c) = p.tag
        {
            entry_tags[id.index()].push((p.range.start, c as u16));
        }
    }
    for t in &mut entry_tags {
        t.sort_unstable();
    }
    // A directory matches on its entry count and, where exiftool listed
    // every entry, on its tag sequence.
    let ifd_at = |at: Option<u64>, n: u64, tags: &[u16]| {
        parts.iter().enumerate().any(|(i, p)| {
            p.kind == PartKind::Ifd
                && at.is_none_or(|a| a == p.range.start)
                && entry_tags[i].len() as u64 == n
                && (tags.len() as u64 != n
                    || entry_tags[i]
                        .iter()
                        .map(|&(_, t)| t)
                        .eq(tags.iter().copied()))
        })
    };
    let mut t = OracleTally::default();
    for u in units {
        let ok = headers.iter().any(|&(base, big)| {
            let (field_off, count_len, entry_len, inline_cap) =
                if big { (12, 8, 20, 8) } else { (8, 2, 12, 4) };
            match u {
                OracleUnit::Dir(_, n, Some((i, off)), _, tags) => {
                    match (base + off).checked_sub(field_off + count_len + i * entry_len) {
                        Some(at) => ifd_at(Some(at), *n, tags),
                        None => false,
                    }
                }
                OracleUnit::Dir(_, n, None, _, tags) => ifd_at(None, *n, tags),
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
            OracleUnit::Dir(name, n, first, e, _) => {
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
    let Some(tool) = std::env::var_os("INVENTORY_ORACLE_EXIFTOOL") else {
        eprintln!("INVENTORY_ORACLE_EXIFTOOL not set; oracle cross-check not requested");
        return;
    };
    // The raw-conformance set, and files listed one per line in the file
    // INVENTORY_ORACLE_FILES names (real DNGs, for example).
    let conformance: Vec<PathBuf> = conformance_files()
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, valid)| *valid)
        .map(|(p, _)| p)
        .collect();
    let listed: Vec<PathBuf> = std::env::var_os("INVENTORY_ORACLE_FILES")
        .map(|l| {
            std::fs::read_to_string(&l)
                .unwrap_or_else(|e| panic!("read {}: {e}", l.to_string_lossy()))
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !conformance.is_empty() || !listed.is_empty(),
        "set ZENRAW_RAW_CONFORMANCE_DIR or INVENTORY_ORACLE_FILES"
    );
    let read = |p: &PathBuf| (p.display().to_string(), std::fs::read(p).unwrap());
    let conformance: Vec<(String, Vec<u8>)> = conformance
        .iter()
        .map(read)
        // exiftool dumps CR3's CMT TIFFs relative to their boxes; the CR3
        // walk lists boxes, not those TIFFs.
        .filter(|(_, d)| d.get(4..8) != Some(b"ftyp"))
        .collect();
    let listed: Vec<(String, Vec<u8>)> = listed.iter().map(read).collect();
    let (mut rows, mut failures, checked, mut units) = run_oracle(&tool, &conformance);
    if !conformance.is_empty() {
        assert_eq!(
            checked, 7,
            "every TIFF-based conformance RAW has TIFF directories"
        );
    }
    let (r, f, c, u) = run_oracle(&tool, &listed);
    assert_eq!(c, listed.len(), "every listed file has TIFF directories");
    rows.extend(r);
    failures.extend(f);
    units += u;
    let checked = checked + c;
    println!("| file | units | matched | explained | unexplained |");
    println!("|---|---|---|---|---|");
    for r in &rows {
        println!("{r}");
    }
    println!("{checked} files, {units} units compared");
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

/// rawler reads every entry of the IFDs it walks: the main chain and the
/// SubIFD, EXIF and GPS IFDs those point at. Main-chain and SubIFD entries
/// feed its decoders (`Structure`); EXIF and GPS entries go into rawler's
/// EXIF, which zenraw never reports (`Dropped`). The Interop IFD is not
/// walked.
#[cfg(feature = "rawler")]
#[test]
fn rawler_build_walks_and_drops_exif_entries() {
    let data = dng_fixture();
    let inv = inventory(&data);
    inv.validate().unwrap();
    let mut seen = 0;
    for (i, p) in inv.parts().iter().enumerate() {
        if p.kind != PartKind::Ifd {
            continue;
        }
        let d = p.detail.as_deref().unwrap_or("");
        let entries: Vec<&Part> = inv
            .parts()
            .iter()
            .filter(|c| c.parent.is_some_and(|id| id.index() == i))
            .collect();
        if d.starts_with("Interop") {
            assert_eq!(p.disposition, Disposition::Skipped, "{inv}");
            assert!(
                entries.iter().all(|c| !c.disposition.is_consumed()),
                "{inv}"
            );
        } else if d.starts_with("EXIF") || d.starts_with("GPS") {
            seen += 1;
            assert_eq!(p.disposition, Disposition::Structure, "{inv}");
            assert!(
                entries
                    .iter()
                    .all(|c| c.disposition == Disposition::Dropped),
                "{inv}"
            );
        } else {
            assert_eq!(p.disposition, Disposition::Structure, "{inv}");
            assert!(
                entries
                    .iter()
                    .all(|c| c.disposition.is_consumed() || c.disposition == Disposition::Dropped),
                "{inv}"
            );
        }
    }
    assert_eq!(seen, 2, "{inv}");
}

// ── Review round 1 ─────────────────────────────────────────────────────

fn xor(data: &[u8], r: std::ops::Range<u64>, mask: u8) -> Vec<u8> {
    let mut m = data.to_vec();
    for b in &mut m[r.start as usize..r.end as usize] {
        *b ^= mask;
    }
    m
}

/// The deepest part holding byte `at`.
fn leaf_at(inv: &Inventory, at: u64) -> &Part {
    let parts = inv.parts();
    let depth = |i: usize| {
        let (mut d, mut cur) = (0, parts[i].parent);
        while let Some(id) = cur {
            d += 1;
            cur = parts[id.index()].parent;
        }
        d
    };
    parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.range.start <= at && at < p.range.end)
        .max_by_key(|&(i, _)| depth(i))
        .map(|(_, p)| p)
        .unwrap_or_else(|| panic!("no part at {at}:\n{inv}"))
}

fn raw16(seed: u16) -> Vec<u8> {
    (0..16u16)
        .flat_map(|i| (i * 3000 + seed).to_le_bytes())
        .collect()
}

/// A decodable 4x4 16-bit DNG, built around a raw SubIFD.
struct Dng {
    w: W,
    make: u32,
    model: u32,
}

impl Dng {
    fn new() -> Self {
        let mut w = W::new();
        let make = w.put(b"ZenRaw\0");
        w.pad();
        let model = w.put(b"SyntheticDng\0");
        w.pad();
        Self { w, make, model }
    }

    fn raw_ifd(&mut self, strips: &[E]) -> u32 {
        let mut e = vec![
            long(254, 0),
            short(256, 4),
            short(257, 4),
            short(258, 16),
            short(259, 1),
            short(262, 32803),
        ];
        e.extend_from_slice(strips);
        e.extend_from_slice(&[
            short(277, 1),
            inline(33422, 1, 4, &[0, 1, 1, 2]),
            short(50717, u16::MAX),
        ]);
        e.sort_by_key(|e| e.tag);
        self.w.ifd(&e, 0)
    }

    /// IFD0 with `extra` entries (an extra duplicate sorts ahead of the base
    /// entry with its tag).
    fn finish(mut self, raw_ifd: u32, extra: &[E]) -> Vec<u8> {
        let mut e = extra.to_vec();
        e.extend_from_slice(&[
            long(254, 1),
            at(271, 2, 7, self.make),
            at(272, 2, 13, self.model),
            long(330, raw_ifd),
            inline(50706, 1, 4, &[1, 4, 0, 0]),
        ]);
        e.sort_by_key(|e| e.tag);
        let ifd0 = self.w.ifd(&e, 0);
        self.w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
        self.w.b
    }
}

/// 10,000 one-byte strips and 450 values each covering all of them: before
/// round 1 the overlap budget ran out and the inventory failed `validate()`.
fn overlap_budget() -> Vec<u8> {
    let mut w = W::new();
    let n = 10_000u32;
    let region = w.put(&vec![0u8; (2 * n) as usize]);
    let offs: Vec<u8> = (0..n)
        .flat_map(|k| (region + 2 * k).to_le_bytes())
        .collect();
    let offs_at = w.put(&offs);
    let cnts: Vec<u8> = (0..n).flat_map(|_| 1u16.to_le_bytes()).collect();
    let cnts_at = w.put(&cnts);
    let big: Vec<E> = (0..450u16)
        .map(|i| at(60000 + i, 7, 2 * n, region))
        .collect();
    let ifd1 = w.ifd(&big, 0);
    let ifd0 = w.ifd(
        &[
            short(256, 4),
            short(257, 4),
            at(273, 4, n, offs_at),
            at(279, 3, n, cnts_at),
        ],
        ifd1,
    );
    w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    w.b
}

/// 4 SubIFDs entries x 1024 pointers to 4096 IFDs two bytes apart in a
/// 0xFF-filled region: before round 1 every IFD parsed ~len/12 junk entries
/// (2.08 GB at 128 KiB).
fn blowup(len: usize) -> Vec<u8> {
    let mut w = W::new();
    let arrays_at = (8 + 2 + 4 * 12 + 4) as u32;
    let region = arrays_at as usize + 4 * 4096;
    let entries: Vec<E> = (0..4u32)
        .map(|k| at(330, 4, 1024, arrays_at + k * 4096))
        .collect();
    let ifd0 = w.ifd(&entries, 0);
    w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    for k in 0..4096u32 {
        w.b.extend_from_slice(&(region as u32 + 2 * k).to_le_bytes());
    }
    w.b.resize(len, 0xFF);
    w.b
}

#[test]
fn overlap_and_blowup_inputs_stay_valid_and_bounded() {
    let inv = inventory(&overlap_budget());
    inv.validate().unwrap();
    let data = blowup(32_768);
    let inv = inventory(&data);
    inv.validate().unwrap();
    // Overlapping IFD tables are refused: one is walked, not 4096.
    let ifds = inv
        .parts()
        .iter()
        .filter(|p| p.kind == PartKind::Ifd)
        .count();
    assert!(ifds < 64, "{ifds} IFDs walked");
}

/// rawloader reads every row of an uncompressed DNG from StripOffsets[0]
/// (dng.rs `decode_uncompressed`); rawler reads each strip at its declared
/// length, or the strips as one buffer when they touch (`decode_strips`).
#[test]
fn multistrip_uncompressed_dng_follows_the_backend() {
    // Junk between two strips.
    let mut d = Dng::new();
    let raw = raw16(0);
    let s0 = d.w.put(&raw[..16]);
    let junk = d.w.put(b"JUNK-SECRET-0042");
    let s1 = d.w.put(&raw[16..]);
    let offs = d.w.put(&[s0.to_le_bytes(), s1.to_le_bytes()].concat());
    let cnts =
        d.w.put(&[16u32.to_le_bytes(), 16u32.to_le_bytes()].concat());
    let ifd = d.raw_ifd(&[at(273, 4, 2, offs), short(278, 2), at(279, 4, 2, cnts)]);
    let data = d.finish(ifd, &[]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let base = decode_summary(&data).expect("decodes");
    let junk_part = leaf_at(&inv, u64::from(junk));
    let s1_part = leaf_at(&inv, u64::from(s1));
    let junk_changes =
        decode_summary(&xor(&data, u64::from(junk)..u64::from(junk) + 16, 0x5A)).unwrap() != base;
    let s1_changes =
        decode_summary(&xor(&data, u64::from(s1)..u64::from(s1) + 16, 0x5A)).unwrap() != base;
    assert_eq!(junk_part.disposition.is_consumed(), junk_changes, "{inv}");
    assert_eq!(s1_part.disposition.is_consumed(), s1_changes, "{inv}");
    if cfg!(feature = "rawler") {
        assert_eq!(s1_part.disposition, Disposition::ImageData, "{inv}");
        assert!(!junk_part.disposition.is_consumed(), "{inv}");
    } else {
        assert_eq!(junk_part.disposition, Disposition::ImageData, "{inv}");
        assert_eq!(s1_part.disposition, Disposition::Skipped, "{inv}");
    }

    // Strip 0 declared over all four rows, strip 1 right after it.
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(7));
    let s1 = d.w.put(&raw16(9)[16..]);
    let offs = d.w.put(&[s0.to_le_bytes(), s1.to_le_bytes()].concat());
    let cnts =
        d.w.put(&[32u32.to_le_bytes(), 16u32.to_le_bytes()].concat());
    let ifd = d.raw_ifd(&[at(273, 4, 2, offs), short(278, 2), at(279, 4, 2, cnts)]);
    let data = d.finish(ifd, &[]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let base = decode_summary(&data).expect("decodes");
    let rows23 = u64::from(s0) + 16..u64::from(s0) + 32;
    assert_eq!(
        leaf_at(&inv, rows23.start).disposition,
        Disposition::ImageData,
        "{inv}"
    );
    assert_ne!(decode_summary(&xor(&data, rows23, 0x5A)).unwrap(), base);
    let s1r = u64::from(s1)..u64::from(s1) + 16;
    assert_eq!(
        leaf_at(&inv, s1r.start).disposition,
        Disposition::Skipped,
        "{inv}"
    );
    assert_eq!(decode_summary(&xor(&data, s1r, 0x5A)).unwrap(), base);
}

/// Both backends keep the last of duplicate tags.
#[test]
fn duplicate_tags_keep_the_last() {
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let ifd = d.raw_ifd(&[long(273, s0), long(279, 32)]);
    let secret = d.w.put(b"SN-SECRET-0042\0");
    d.w.pad();
    let data = d.finish(ifd, &[short(274, 3), short(274, 6), at(271, 2, 15, secret)]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let orient: Vec<&Part> = inv
        .parts()
        .iter()
        .filter(|p| p.tag == PartTag::Code(274) && p.kind == PartKind::Field)
        .collect();
    assert_eq!(orient.len(), 2);
    assert_eq!(orient[0].disposition, Disposition::Dropped, "{inv}");
    assert_eq!(
        orient[1].disposition,
        Disposition::Metadata(MetadataKind::Orientation),
        "{inv}"
    );
    let base = decode_summary(&data).expect("decodes");
    assert!(base.1.contains("Rotate90"), "{}", base.1);
    let first = orient[0].range.start + 8..orient[0].range.start + 10;
    assert_eq!(decode_summary(&xor(&data, first, 0x5A)).unwrap(), base);
    assert_eq!(
        leaf_at(&inv, u64::from(secret)).disposition,
        Disposition::Dropped,
        "{inv}"
    );
}

/// The SR2Private pointer, its IFD and the encrypted block are Sony ARW
/// structure only: in a DNG they are never read.
#[test]
fn sr2_rules_need_a_sony_arw() {
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let ifd = d.raw_ifd(&[long(273, s0), long(279, 32)]);
    let blob = d.w.put(b"SR2-SECRET-00042");
    let sr2 = d.w.ifd(&[long(0x7200, blob), long(0x7201, 16)], 0);
    let data = d.finish(ifd, &[long(0xC634, sr2)]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let b = leaf_at(&inv, u64::from(blob));
    assert!(!b.disposition.is_consumed(), "{inv}");
    assert!(
        !leaf_at(&inv, u64::from(sr2) + 2).disposition.is_consumed(),
        "{inv}"
    );
    let base = decode_summary(&data).expect("decodes");
    let m = xor(&data, u64::from(blob)..u64::from(blob) + 16, 0x5A);
    let m = xor(&m, u64::from(sr2)..u64::from(sr2) + 30, 0x5A);
    assert_eq!(decode_summary(&m).unwrap(), base);
}

fn raf_with_slack() -> (Vec<u8>, u64, u64, u64) {
    let mut b = vec![0u8; 148];
    b[..16].copy_from_slice(b"FUJIFILMCCD-RAW ");
    b[16..20].copy_from_slice(b"0201");
    let mut tiff = W::new();
    let ifd0 = tiff.ifd(&[short(274, 1)], 0);
    tiff.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let app1_len = (2 + 6 + tiff.b.len()) as u16;
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&app1_len.to_be_bytes());
    jpeg.extend_from_slice(b"Exif\0\0");
    jpeg.extend_from_slice(&tiff.b);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    let jpeg_off = b.len() as u32;
    b.extend_from_slice(&jpeg);
    let hdr_off = b.len() as u32;
    b.extend_from_slice(&1u32.to_be_bytes());
    b.extend_from_slice(&[0x01, 0x00, 0x00, 0x04, 0x00, 0x04, 0x00, 0x04]);
    let hdr_slack = b.len() as u64;
    b.extend_from_slice(b"HDR-SLACK-SECRET");
    let hdr_len = b.len() as u32 - hdr_off;
    let cfa_off = b.len() as u32;
    let mut cfa = W::new();
    let mid_slack = u64::from(cfa.put(b"CFA-MIDDLE-SLACK"));
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
    let end_slack = u64::from(cfa.put(b"CFA-TAIL-SECRET!"));
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
    (
        b,
        hdr_slack,
        u64::from(cfa_off) + mid_slack,
        u64::from(cfa_off) + end_slack,
    )
}

/// RAF sections are containers: bytes no record or TIFF part covers are
/// `Unreferenced`; the section lengths in the header are never read.
#[test]
fn raf_slack_is_unreferenced_and_lengths_unread() {
    let (data, hdr_slack, mid, tail) = raf_with_slack();
    let inv = inventory(&data);
    inv.validate().unwrap();
    for at in [hdr_slack, mid, tail] {
        assert_eq!(
            leaf_at(&inv, at).disposition,
            Disposition::Unreferenced,
            "{at}: {inv}"
        );
    }
    for at in [88, 96, 104] {
        assert_eq!(
            leaf_at(&inv, at).disposition,
            Disposition::Skipped,
            "{at}: {inv}"
        );
    }
    for at in [84, 92, 100] {
        assert_eq!(
            leaf_at(&inv, at).disposition,
            Disposition::Structure,
            "{at}: {inv}"
        );
    }
}

/// rawloader sizes RAF raw data from RafImageWidth/Length/BitsPerSample;
/// RafByteCounts (0xF008) is what rawler uses.
#[test]
fn raf_raw_length_follows_the_backend() {
    let (mut data, _, _, _) = raf_with_slack();
    let inv = inventory(&data);
    let raw = inv
        .parts()
        .iter()
        .find(|p| p.tag == PartTag::Code(0xF007) && p.disposition == Disposition::ImageData)
        .unwrap_or_else(|| panic!("{inv}"))
        .range
        .clone();
    let f008 = data
        .windows(4)
        .position(|w| w == [0x08, 0xF0, 0x04, 0x00])
        .unwrap();
    // rawloader: 48 declared, 32 read (4x4 16-bit). rawler: a view of the
    // declared count, 16 here.
    let (declared, expected): (u32, u64) = if cfg!(feature = "rawler") {
        (16, 16)
    } else {
        (48, 32)
    };
    data[f008 + 8..f008 + 12].copy_from_slice(&declared.to_le_bytes());
    let inv = inventory(&data);
    inv.validate().unwrap();
    let r = inv
        .parts()
        .iter()
        .find(|p| p.tag == PartTag::Code(0xF007) && p.disposition == Disposition::ImageData)
        .unwrap_or_else(|| panic!("{inv}"))
        .range
        .clone();
    assert_eq!((r.start, r.end - r.start), (raw.start, expected), "{inv}");
}

/// A child box whose header runs past its parent becomes a `Malformed` gap
/// to the parent's end.
#[test]
fn cr3_bad_child_box_is_malformed() {
    fn bx(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(fourcc);
        v.extend_from_slice(payload);
        v
    }
    let mut moov = bx(b"mvhd", &[0; 20]);
    moov.extend_from_slice(b"\0\0\0\x03junkSECRET-AFTER-BAD-BOX");
    let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
    f.extend(bx(b"moov", &moov));
    f.extend(bx(b"mdat", &[0x5A; 64]));
    let inv = inventory(&f);
    inv.validate().unwrap();
    let p = leaf_at(&inv, 24 + 8 + 28 + 10);
    assert_eq!(p.disposition, Disposition::Malformed, "{inv}");
}

/// zenraw's decode rejects BigTIFF (`is_raw_file`), so nothing is consumed.
#[test]
fn bigtiff_is_not_decoded() {
    let mut b = b"II\x2b\x00\x08\x00\x00\x00".to_vec();
    b.extend_from_slice(&16u64.to_le_bytes());
    let entry = |b: &mut Vec<u8>, tag: u16, typ: u16, count: u64, v: u64| {
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&typ.to_le_bytes());
        b.extend_from_slice(&count.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    };
    b.extend_from_slice(&4u64.to_le_bytes());
    let strip = 16 + 8 + 4 * 20 + 8;
    entry(&mut b, 256, 3, 1, 4);
    entry(&mut b, 257, 3, 1, 4);
    entry(&mut b, 273, 16, 1, strip as u64);
    entry(&mut b, 279, 16, 1, 16);
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(&[0x11; 16]);
    let inv = inventory(&b);
    inv.validate().unwrap();
    assert!(!zenraw::is_raw_file(&b));
    assert!(decode_summary(&b).is_err());
    assert!(
        !inv.parts().iter().any(|p| p.disposition.is_consumed()),
        "{inv}"
    );
}

/// `inventory` checks the job's stop token and `max_input_bytes` like
/// `decoder` does.
#[test]
fn inventory_honours_stop_and_input_limit() {
    let data = dng_fixture();
    let limited = RawDecoderConfig::new()
        .job()
        .with_limits(zencodec::ResourceLimits::none().with_max_input_bytes(16))
        .inventory(&data);
    assert!(limited.is_err());
    struct Cancelled;
    impl enough::Stop for Cancelled {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::Cancelled)
        }
    }
    let stopped = RawDecoderConfig::new()
        .job()
        .with_stop(zencodec::StopToken::new(Cancelled))
        .inventory(&data);
    assert!(stopped.is_err());
}

// ── rawloader backend ──────────────────────────────────────────────────

/// rawloader follows SubIFDs from the EXIF IFD too (tiff.rs `TiffIFD::new`,
/// any IFD to depth 10): an Orientation found that way is what it reports.
#[cfg(not(feature = "rawler"))]
#[test]
fn orientation_reached_through_the_exif_ifd() {
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let ifd = d.raw_ifd(&[long(273, s0), long(279, 32)]);
    let nested = d.w.ifd(&[short(274, 6)], 0);
    let exif = d.w.ifd(&[long(330, nested)], 0);
    let data = d.finish(ifd, &[long(34665, exif)]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let p = leaf_at(&inv, u64::from(nested) + 2 + 8);
    assert_eq!(
        p.disposition,
        Disposition::Metadata(MetadataKind::Orientation),
        "{inv}"
    );
    let base = decode_summary(&data).expect("decodes");
    assert!(base.1.contains("Rotate90"), "{}", base.1);
}

/// rawloader's `get_str` stops at the first NUL.
#[cfg(not(feature = "rawler"))]
#[test]
fn ascii_after_nul_is_dropped() {
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let ifd = d.raw_ifd(&[long(273, s0), long(279, 32)]);
    let soft = d.w.put(b"ZenRaw\0SERIAL-42\0");
    d.w.pad();
    let data = d.finish(ifd, &[at(0x0131, 2, 17, soft)]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let tail = leaf_at(&inv, u64::from(soft) + 7);
    assert_eq!(tail.disposition, Disposition::Dropped, "{inv}");
    assert_eq!(
        leaf_at(&inv, u64::from(soft)).disposition,
        Disposition::Structure,
        "{inv}"
    );
}

/// rawloader picks the raw IFD per Make (`get_decoder`): an ARW decodes the
/// first IFD with strips, a NEF the first with a CFA pattern.
#[cfg(not(feature = "rawler"))]
#[test]
fn raw_ifd_follows_the_make() {
    for (make, first_strip_is_raw) in [(&b"SONY\0"[..], true), (&b"NIKON CORPORATION\0"[..], false)]
    {
        let mut w = W::new();
        let mk = w.put(make);
        w.pad();
        let thumb = w.put(&[1u8; 12]);
        let raw = w.put(&raw16(0));
        let sub = w.ifd(
            &[
                short(256, 4),
                short(257, 4),
                short(258, 16),
                short(259, 1),
                long(273, raw),
                long(279, 32),
                inline(33422, 1, 4, &[0, 1, 1, 2]),
            ],
            0,
        );
        let ifd0 = w.ifd(
            &[
                short(256, 2),
                short(257, 2),
                at(271, 2, make.len() as u32, mk),
                long(273, thumb),
                long(279, 12),
                long(330, sub),
            ],
            0,
        );
        w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
        let inv = inventory(&w.b);
        inv.validate().unwrap();
        let t = leaf_at(&inv, u64::from(thumb)).disposition;
        let r = leaf_at(&inv, u64::from(raw)).disposition;
        if first_strip_is_raw {
            assert_eq!(
                (t, r),
                (Disposition::ImageData, Disposition::Skipped),
                "{inv}"
            );
        } else {
            assert_eq!(
                (t, r),
                (Disposition::Skipped, Disposition::ImageData),
                "{inv}"
            );
        }
    }
}

// ── rawler backend ─────────────────────────────────────────────────────

/// rawler sets orientation only for DNG, from IFD0 (dng.rs:41); elsewhere
/// it reports Normal (zenraw#22).
#[cfg(feature = "rawler")]
#[test]
fn rawler_orientation_only_for_dng_ifd0() {
    let data = dng_fixture();
    let inv = inventory(&data);
    assert!(
        inv.parts()
            .iter()
            .any(|p| p.disposition == Disposition::Metadata(MetadataKind::Orientation)),
        "{inv}"
    );
    let mut w = W::new();
    let mk = w.put(b"NIKON CORPORATION\0");
    w.pad();
    let ifd0 = w.ifd(&[at(271, 2, 18, mk), short(274, 6)], 0);
    w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let inv = inventory(&w.b);
    let o = inv
        .parts()
        .iter()
        .find(|p| p.tag == PartTag::Code(274))
        .unwrap();
    assert_eq!(o.disposition, Disposition::Dropped, "{inv}");
    assert!(o.detail.as_deref().unwrap().contains("zenraw#22"));
}

/// rawler reads only the boxes its BMFF reader knows; the payload of any
/// other box is seeked past.
#[cfg(feature = "rawler")]
#[test]
fn rawler_skips_cr3_boxes_it_does_not_parse() {
    fn bx(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(fourcc);
        v.extend_from_slice(payload);
        v
    }
    let mut moov = bx(b"mvhd", &[0; 20]);
    moov.extend(bx(b"zzzz", b"PRIVATE-SERIAL-0"));
    let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
    f.extend(bx(b"moov", &moov));
    let top = f.len() as u64;
    f.extend(bx(b"PRVW", b"PREVIEW-PAYLOAD!"));
    f.extend(bx(b"mdat", &[0x5A; 64]));
    let inv = inventory(&f);
    inv.validate().unwrap();
    let inner = (24 + 8 + 28 + 8) as u64;
    assert_eq!(
        leaf_at(&inv, inner).disposition,
        Disposition::Skipped,
        "{inv}"
    );
    assert_eq!(
        leaf_at(&inv, top + 8).disposition,
        Disposition::Skipped,
        "{inv}"
    );
    assert_eq!(
        leaf_at(&inv, top).disposition,
        Disposition::Structure,
        "{inv}"
    );
}

/// With the `xmp` feature a rawler decode surfaces the packet in the XMP
/// uuid; the inventory marks it there.
#[cfg(all(feature = "rawler", feature = "xmp"))]
#[test]
fn rawler_cr3_xmp_uuid_packet_is_metadata() {
    fn bx(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(fourcc);
        v.extend_from_slice(payload);
        v
    }
    let xmp_uuid: [u8; 16] = [
        0xbe, 0x7a, 0xcf, 0xcb, 0x97, 0xa9, 0x42, 0xe8, 0x9c, 0x71, 0x99, 0x94, 0x91, 0xe3, 0xaf,
        0xac,
    ];
    let mut payload = xmp_uuid.to_vec();
    payload.extend_from_slice(XMP_PACKET);
    payload.extend_from_slice(b"\n   ");
    let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
    f.extend(bx(b"moov", &bx(b"mvhd", &[0; 20])));
    let at = f.len() as u64 + 8 + 16;
    f.extend(bx(b"uuid", &payload));
    f.extend(bx(b"mdat", &[0x5A; 64]));
    let inv = inventory(&f);
    inv.validate().unwrap();
    let p = leaf_at(&inv, at);
    assert_eq!(
        p.disposition,
        Disposition::Metadata(MetadataKind::Xmp),
        "{inv}"
    );
    assert_eq!(
        p.range.end - p.range.start,
        XMP_PACKET.len() as u64,
        "{inv}"
    );
}

// ── Maker notes and CR3 CMT TIFFs ──────────────────────────────────────

/// A Nikon type-2 maker note (signature, embedded TIFF header, IFD with
/// offsets relative to that header) is walked the way rawloader's
/// `new_makernote` reads it: tags rawloader knows are parsed, others
/// skipped, and bytes nothing references inside the value are
/// `Unreferenced`.
#[cfg(not(feature = "rawler"))]
#[test]
fn nikon_maker_note_is_walked() {
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let ifd = d.raw_ifd(&[long(273, s0), long(279, 32)]);
    // Maker note: "Nikon\0\x02\x10\0\0" + "II*\0" + IFD offset 8, then an
    // IFD with WB levels (0x000C, known) and an owner name (0x00A9, not
    // known to rawloader), then the owner string and slack.
    let mut mn = b"Nikon\0\x02\x10\0\0II*\0\x08\0\0\0".to_vec();
    mn.extend_from_slice(&2u16.to_le_bytes());
    let owner_rel: u32 = 8 + 2 + 2 * 12 + 4;
    for (tag, typ, count, field) in [
        (0x000Cu16, 1u16, 4u32, [1u8, 2, 3, 4]),
        (0x00A9, 2, 11, owner_rel.to_le_bytes()),
    ] {
        mn.extend_from_slice(&tag.to_le_bytes());
        mn.extend_from_slice(&typ.to_le_bytes());
        mn.extend_from_slice(&count.to_le_bytes());
        mn.extend_from_slice(&field);
    }
    mn.extend_from_slice(&0u32.to_le_bytes());
    mn.extend_from_slice(b"OWNER-NAME\0");
    mn.extend_from_slice(b"SLACK!");
    let mn_at = d.w.put(&mn);
    d.w.pad();
    let exif = d.w.ifd(&[at(37500, 7, mn.len() as u32, mn_at)], 0);
    let data = d.finish(ifd, &[long(34665, exif)]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let base = u64::from(mn_at) + 10;
    let ifd_at = base + 8;
    let wb = leaf_at(&inv, ifd_at + 2 + 8);
    assert_eq!(wb.disposition, Disposition::Structure, "{inv}");
    // Not a tag rawloader knows, nor one the tag table names: Unknown.
    let owner = leaf_at(&inv, base + u64::from(owner_rel));
    assert_eq!(owner.disposition, Disposition::Unknown, "{inv}");
    let slack = leaf_at(&inv, base + u64::from(owner_rel) + 11);
    assert_eq!(slack.disposition, Disposition::Unreferenced, "{inv}");
    // The decode does not change when the skipped owner name or the slack
    // changes.
    let summary = decode_summary(&data).expect("decodes");
    let m = xor(&data, owner.range.clone(), 0x5A);
    let m = xor(&m, slack.range.clone(), 0x5A);
    assert_eq!(decode_summary(&m).unwrap(), summary);
}

/// CR3 CMT boxes are TIFFs: CMT2 holds the EXIF IFD (serial numbers live
/// there). rawler parses it into its own EXIF, which zenraw never reports;
/// rawloader builds do not decode CR3 at all.
#[test]
fn cr3_cmt_exif_is_never_surfaced() {
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
    // CMT2: a TIFF whose IFD0 holds BodySerialNumber (0xA431).
    let mut t = W::new();
    let serial = t.put(b"SN-CMT2-0042\0");
    t.pad();
    let ifd = t.ifd(&[at(0xA431, 2, 13, serial)], 0);
    t.b[4..8].copy_from_slice(&ifd.to_le_bytes());
    let mut uuid_payload = canon_uuid.to_vec();
    uuid_payload.extend(bx(b"CNCV", b"CanonCR3_001/00.09.00/00.00.00"));
    let cmt2_at = 8 + 16 + 8 + 8 + 16 + 38 + 8;
    uuid_payload.extend(bx(b"CMT2", &t.b));
    let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
    let mut moov = bx(b"uuid", &uuid_payload);
    moov.extend(bx(b"mvhd", &[0; 20]));
    f.extend(bx(b"moov", &moov));
    f.extend(bx(b"mdat", &[0x5A; 64]));
    let inv = inventory(&f);
    inv.validate().unwrap();
    let at_serial = cmt2_at as u64 + u64::from(serial);
    let p = leaf_at(&inv, at_serial);
    assert_eq!(&f[at_serial as usize..at_serial as usize + 7], b"SN-CMT2");
    let expected = if cfg!(feature = "rawler") {
        Disposition::Dropped
    } else {
        Disposition::Skipped
    };
    assert_eq!(p.disposition, expected, "{inv}");
}

// ── Review round 2 (zenraw) / round 3 (shared walker) ─────────────────

/// A container split around an overlapping part keeps its body in each
/// piece, so its slack stays `Unreferenced` (review R2-A): the CFA header
/// section's length runs 4 bytes into the CFA section.
#[test]
fn split_container_keeps_its_body() {
    let (mut data, hdr_slack, _, _) = raf_with_slack();
    let hdr_len = u32::from_be_bytes(data[96..100].try_into().unwrap()) + 4;
    data[96..100].copy_from_slice(&hdr_len.to_be_bytes());
    let inv = inventory(&data);
    inv.validate().unwrap();
    assert_eq!(
        leaf_at(&inv, hdr_slack).disposition,
        Disposition::Unreferenced,
        "{inv}"
    );
}

/// The real fuji_xt1.raf with a 23-byte XMP packet written into its CFA
/// section slack (review R2-A): the packet is placed first and splits the
/// CFA section; the rest of the slack stays `Unreferenced`.
#[cfg(feature = "xmp")]
#[test]
fn split_real_raf_keeps_its_slack_unreferenced() {
    let Some(dir) = std::env::var_os("ZENRAW_RAW_CONFORMANCE_DIR") else {
        eprintln!("ZENRAW_RAW_CONFORMANCE_DIR not set; corpus check not requested");
        return;
    };
    let mut data = std::fs::read(PathBuf::from(dir).join("valid/xtrans/fuji_xt1.raf")).unwrap();
    let packet = b"<x:xmpmeta></x:xmpmeta>";
    data[898200..898200 + packet.len()].copy_from_slice(packet);
    let inv = inventory(&data);
    inv.validate().unwrap();
    assert_eq!(
        leaf_at(&inv, 898210).disposition,
        Disposition::Metadata(MetadataKind::Xmp),
        "{inv}"
    );
    for at in [898080, 898600] {
        assert_eq!(
            leaf_at(&inv, at).disposition,
            Disposition::Unreferenced,
            "{at}: {inv}"
        );
    }
}

/// rawler follows SubIFDs only through a LONG pointer (formats/tiff/ifd.rs);
/// rawloader follows any integer pointer (review R2-B).
#[test]
fn rawler_ignores_a_short_subifd_pointer() {
    let mut w = W::new();
    let make = w.put(b"ZenRaw\0");
    w.pad();
    let model = w.put(b"SyntheticDng\0");
    w.pad();
    let s0 = w.put(&raw16(0));
    let secret = w.put(b"SUBIFD-SECRET-0042\0");
    w.pad();
    let sub = w.ifd(&[long(254, 1), at(270, 2, 19, secret)], 0);
    let mut e = vec![
        long(254, 0),
        short(256, 4),
        short(257, 4),
        short(258, 16),
        short(259, 1),
        short(262, 32803),
        at(271, 2, 7, make),
        at(272, 2, 13, model),
        long(273, s0),
        short(277, 1),
        long(279, 32),
        short(330, sub as u16),
        inline(33422, 1, 4, &[0, 1, 1, 2]),
        inline(50706, 1, 4, &[1, 4, 0, 0]),
        short(50717, u16::MAX),
    ];
    e.sort_by_key(|e| e.tag);
    let ifd0 = w.ifd(&e, 0);
    w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let data = w.b;
    let inv = inventory(&data);
    inv.validate().unwrap();
    let v = leaf_at(&inv, u64::from(secret));
    if cfg!(feature = "rawler") {
        assert!(!v.disposition.is_consumed(), "{inv}");
    }
    let base = decode_summary(&data).expect("decodes");
    let m = xor(&data, u64::from(secret)..u64::from(secret) + 19, 0x5A);
    assert_eq!(decode_summary(&m).unwrap(), base);
}

/// rawloader rejects an IFD of more than 4000 entries (decoders/tiff.rs:197):
/// a nested one is not followed, and a main-chain one fails the decode
/// (review R2-C).
#[test]
fn rawloader_rejects_ifds_over_4000_entries() {
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let ifd = d.raw_ifd(&[long(273, s0), long(279, 32)]);
    let mut e = vec![short(274, 6)];
    for k in 0..4000u16 {
        e.push(inline(40000 + k, 1, 1, &[0]));
    }
    let nested = d.w.ifd(&e, 0);
    let exif = d.w.ifd(&[long(330, nested)], 0);
    let data = d.finish(ifd, &[long(34665, exif)]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let o = leaf_at(&inv, u64::from(nested) + 2 + 8);
    assert!(!o.disposition.is_consumed(), "{inv}");
    let base = decode_summary(&data).expect("decodes");
    assert!(!base.1.contains("Rotate90"), "{}", base.1);

    // IFD0 itself over the cap.
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let ifd = d.raw_ifd(&[long(273, s0), long(279, 32)]);
    let extra: Vec<E> = (0..4001u16)
        .map(|k| inline(40000 + k, 1, 1, &[0]))
        .collect();
    let data = d.finish(ifd, &extra);
    let inv = inventory(&data);
    inv.validate().unwrap();
    if !cfg!(feature = "rawler") {
        assert!(decode_summary(&data).is_err());
        assert!(
            !inv.parts().iter().any(|p| p.disposition.is_consumed()),
            "{inv}"
        );
    }
}

/// Both backends follow every SubIFD pointer (review R2-D): 1029 pointers
/// to a thumbnail IFD, then the raw IFD as pointer 1030.
#[test]
fn every_subifd_pointer_is_followed() {
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let mut e = vec![
        long(254, 0),
        short(256, 4),
        short(257, 4),
        short(258, 16),
        short(259, 1),
        short(262, 32803),
        long(273, s0),
        short(274, 6),
        short(277, 1),
        long(279, 32),
        inline(33422, 1, 4, &[0, 1, 1, 2]),
        short(50717, u16::MAX),
    ];
    e.sort_by_key(|e| e.tag);
    let raw = d.w.ifd(&e, 0);
    let thumb = d.w.ifd(
        &[long(254, 1), short(256, 2), short(257, 2), short(259, 1)],
        0,
    );
    let mut ptrs = Vec::new();
    for _ in 0..1029 {
        ptrs.extend_from_slice(&thumb.to_le_bytes());
    }
    ptrs.extend_from_slice(&raw.to_le_bytes());
    let arr = d.w.put(&ptrs);
    let mut e0 = vec![
        long(254, 1),
        at(271, 2, 7, d.make),
        at(272, 2, 13, d.model),
        at(330, 4, 1030, arr),
        inline(50706, 1, 4, &[1, 4, 0, 0]),
    ];
    e0.sort_by_key(|e| e.tag);
    let ifd0 = d.w.ifd(&e0, 0);
    d.w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let data = d.w.b;
    let inv = inventory(&data);
    inv.validate().unwrap();
    assert_eq!(
        leaf_at(&inv, u64::from(s0)).disposition,
        Disposition::ImageData,
        "{inv}"
    );
}

/// When a budget or cap stops a walk (here more than 4096 IFDs), its gaps
/// are `Unknown` with a "may read" detail, not bare `Unreferenced`.
#[test]
fn budget_stopped_walk_leaves_unknown_gaps() {
    let mut d = Dng::new();
    let s0 = d.w.put(&raw16(0));
    let ifd = d.raw_ifd(&[long(273, s0), long(279, 32)]);
    let mut ptrs = Vec::new();
    for k in 0..5000u16 {
        let at = d.w.ifd(&[short(40000, k)], 0);
        ptrs.extend_from_slice(&at.to_le_bytes());
    }
    let arr = d.w.put(&ptrs);
    let exif = d.w.ifd(&[at(330, 4, 5000, arr)], 0);
    let data = d.finish(ifd, &[long(34665, exif)]);
    let inv = inventory(&data);
    inv.validate().unwrap();
    let gaps: Vec<&Part> = inv
        .parts()
        .iter()
        .filter(|p| {
            p.parent.is_none() && p.kind == PartKind::Gap && p.disposition != Disposition::Trailing
        })
        .collect();
    assert!(!gaps.is_empty(), "{inv}");
    for g in gaps {
        if g.disposition == Disposition::Padding {
            continue;
        }
        assert_eq!(g.disposition, Disposition::Unknown, "{inv}");
        assert!(g.detail.as_deref().unwrap().contains("may read"), "{inv}");
    }
}

/// Review round 4, R4-1: `n` SubIFDs entries each naming one shared array
/// of `c` pointers. The walk reads only the pointers it can still queue, so
/// the work is about `n + 4096` pointer reads, not `n * c`.
#[test]
fn wide_pointer_arrays_are_read_only_as_far_as_the_ifd_cap() {
    // Tests build at opt-level 2, so the input is the reviewer's 4 MB case
    // (39 s before the fix in a release build).
    let (n, c) = (65_535u32, 819_200u32);
    let arr = 8 + 2 + 12 * n + 4;
    let target = arr + 4 * c;
    let mut data = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    data.extend_from_slice(&(n as u16).to_le_bytes());
    for _ in 0..n {
        data.extend_from_slice(&330u16.to_le_bytes());
        data.extend_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(&c.to_le_bytes());
        data.extend_from_slice(&arr.to_le_bytes());
    }
    data.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..c {
        data.extend_from_slice(&target.to_le_bytes());
    }
    data.extend_from_slice(&[0; 6]);
    // 4,063,240 bytes: before the fix, 65,535 x 819,200 pointer reads.
    assert_eq!(data.len(), 4_063_240);
    let t = std::time::Instant::now();
    let inv = inventory(&data);
    let took = t.elapsed();
    inv.validate().unwrap();
    assert!(took.as_secs_f64() < 5.0, "took {took:?}");
    assert!(
        inv.parts().iter().any(|p| p
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("pointers not walked: more than 4096 IFDs"))),
        "{inv}"
    );
}

// ── Review round 3 (zenraw) ────────────────────────────────────────────

const RAWLER: bool = cfg!(feature = "rawler");

/// A DNG whose IFD0 is a preview (NewSubFileType 1) and whose raw IFD is
/// reached only through `ptrs`.
fn preview_ifd0(mut w: W, ptrs: &[E]) -> Vec<u8> {
    let make = w.put(b"ZenRaw\0");
    w.pad();
    let model = w.put(b"SyntheticDng\0");
    w.pad();
    let mut e = vec![
        long(254, 1),
        at(271, 2, 7, make),
        at(272, 2, 13, model),
        inline(50706, 1, 4, &[1, 4, 0, 0]),
    ];
    e.extend_from_slice(ptrs);
    e.sort_by_key(|e| e.tag);
    let ifd0 = w.ifd(&e, 0);
    w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    w.b
}

/// A 4x4 16-bit raw IFD's entries, strip at `s0` (StripOffsets is entry 6).
fn raw_entries(s0: u32) -> Vec<E> {
    let mut e = vec![
        long(254, 0),
        short(256, 4),
        short(257, 4),
        short(258, 16),
        short(259, 1),
        short(262, 32803),
        long(273, s0),
        short(277, 1),
        long(279, 32),
        inline(33422, 1, 4, &[0, 1, 1, 2]),
        short(50717, u16::MAX),
    ];
    e.sort_by_key(|e| e.tag);
    e
}

/// Whether the strip at `s0` is reported consumed, checked against the
/// decode: it is consumed exactly when flipping its bytes changes the
/// decode's outcome.
fn strip_consumed(label: &str, data: &[u8], s0: u32) -> bool {
    let inv = inventory(data);
    inv.validate().unwrap();
    let strip = leaf_at(&inv, u64::from(s0));
    let consumed = strip.disposition.is_consumed();
    let changes = decode_summary(data) != decode_summary(&xor(data, strip.range.clone(), 0xFF));
    assert_eq!(consumed, changes, "{label}: strip {strip:?}\n{inv}");
    consumed
}

/// rawler files sub-IFD offsets in a HashMap (formats/tiff/ifd.rs:105-165):
/// a LONG value under its tag, so of two SubIFDs entries only the later is
/// parsed; rawloader follows both.
#[test]
fn rawler_parses_only_the_last_of_duplicate_subifds() {
    for raw_first in [true, false] {
        let mut w = W::new();
        let thumb = w.ifd(&[long(254, 1), short(256, 2)], 0);
        let s0 = w.put(&raw16(0));
        let raw = w.ifd(&raw_entries(s0), 0);
        let (a, b) = if raw_first {
            (raw, thumb)
        } else {
            (thumb, raw)
        };
        let data = preview_ifd0(w, &[long(330, a), long(330, b)]);
        let consumed = strip_consumed(&format!("raw first {raw_first}"), &data, s0);
        assert_eq!(consumed, !(RAWLER && raw_first), "raw first {raw_first}");
    }
}

/// rawler files a pointer of a type it does not know (IFD = 13) under the
/// type code, so two IFD-typed pointers in one IFD collide and only the
/// later is parsed.
#[test]
fn rawler_unknown_type_pointers_collide_on_their_type() {
    for with_exif in [true, false] {
        let mut w = W::new();
        let raw = w.ifd(&raw_entries(0), 0);
        assert_eq!(raw, 8);
        let exif = w.ifd(&[long(0x9000, 0x3032_3330)], 0);
        let s0 = w.put(&raw16(0));
        let so = (raw + 2 + 6 * 12 + 8) as usize;
        w.b[so..so + 4].copy_from_slice(&s0.to_le_bytes());
        let mut ptrs = vec![inline(330, 13, 1, &[raw as u8, 0, 0, 0])];
        if with_exif {
            ptrs.push(inline(34665, 13, 1, &[exif as u8, 0, 0, 0]));
        }
        let data = preview_ifd0(w, &ptrs);
        let consumed = strip_consumed(&format!("with EXIF {with_exif}"), &data, s0);
        if RAWLER {
            assert_eq!(consumed, !with_exif, "with EXIF {with_exif}");
        }
    }
}

/// rawler reads an IFD-typed value as one byte per count (inline up to 4)
/// and follows one IFD, at the first byte's value.
#[test]
fn rawler_unknown_type_pointer_follows_one_ifd_at_its_first_byte() {
    for count in [2u32, 5] {
        let mut w = W::new();
        let a = w.ifd(&[long(254, 1), short(256, 2)], 0);
        assert_eq!(a, 8);
        w.b.resize(264, 0);
        let s0 = w.put(&raw16(0));
        let raw = w.ifd(&raw_entries(s0), 0);
        let mut ptrs = vec![a, raw];
        while ptrs.len() < count as usize {
            ptrs.push(raw);
        }
        w.pad();
        while w.pos() % 256 != 8 {
            w.b.push(0);
        }
        let p = w.put(
            &ptrs
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let data = preview_ifd0(w, &[at(330, 13, count, p)]);
        let consumed = strip_consumed(&format!("count {count}"), &data, s0);
        if RAWLER {
            assert!(!consumed, "count {count}");
        }
    }
}

/// rawler parses at most 4096 entries of an IFD (formats/tiff/ifd.rs:124);
/// rawloader rejects more than 4000. A StripOffsets at index 4099 is read
/// by neither.
#[test]
fn entries_past_rawlers_4096_stop_are_not_read() {
    let mut w = W::new();
    let make = w.put(b"ZenRaw\0");
    w.pad();
    let model = w.put(b"SyntheticDng\0");
    w.pad();
    let s0 = w.put(&raw16(0));
    let mut e: Vec<E> = raw_entries(s0)
        .into_iter()
        .filter(|e| e.tag != 273)
        .collect();
    e.extend_from_slice(&[
        at(271, 2, 7, make),
        at(272, 2, 13, model),
        inline(50706, 1, 4, &[1, 4, 0, 0]),
    ]);
    e.sort_by_key(|e| e.tag);
    let mut k = 0u16;
    while e.len() < 4096 {
        e.push(inline(60000 + k, 1, 1, &[0]));
        k += 1;
    }
    e.push(E {
        tag: 0,
        typ: 0,
        count: 0,
        field: [0; 4],
    });
    e.push(inline(65000, 1, 1, &[0]));
    e.push(inline(65001, 1, 1, &[0]));
    e.push(long(273, s0));
    let ifd0 = w.ifd(&e, 0);
    w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let data = w.b;
    assert!(!strip_consumed("4100 entries", &data, s0));
    let inv = inventory(&data);
    let so = leaf_at(&inv, u64::from(ifd0) + 2 + 4099 * 12 + 4);
    assert!(!so.disposition.is_consumed(), "{so:?}");
    if RAWLER {
        assert!(
            so.detail
                .as_deref()
                .is_some_and(|d| d.contains("4096-entry stop")),
            "{so:?}"
        );
        let ifd = inv
            .parts()
            .iter()
            .find(|p| p.kind == PartKind::Ifd && p.range.start == u64::from(ifd0))
            .unwrap();
        assert!(
            ifd.detail
                .as_deref()
                .is_some_and(|d| d.contains("rawler's chain ends here")),
            "{ifd:?}"
        );
    }
}

/// rawloader parses the RAF JPEG's EXIF TIFF with `new_root(..)?`
/// (decoders/tiff.rs:128): a 4001-entry IFD0 there fails the whole decode.
#[test]
fn raf_jpeg_exif_chain_rejection_fails_the_file() {
    let Some(dir) = std::env::var_os("ZENRAW_RAW_CONFORMANCE_DIR") else {
        eprintln!("ZENRAW_RAW_CONFORMANCE_DIR not set; corpus check not requested");
        return;
    };
    let data = std::fs::read(PathBuf::from(dir).join("valid/xtrans/fuji_xt1.raf")).unwrap();
    let jpeg = u32::from_be_bytes(data[84..88].try_into().unwrap()) as usize;
    let tiff = jpeg + 12;
    let le = &data[tiff..tiff + 2] == b"II";
    let off: [u8; 4] = data[tiff + 4..tiff + 8].try_into().unwrap();
    let ifd0 = tiff
        + if le {
            u32::from_le_bytes(off)
        } else {
            u32::from_be_bytes(off)
        } as usize;
    let mut m = data.clone();
    let n: u16 = 4001;
    m[ifd0..ifd0 + 2].copy_from_slice(&if le { n.to_le_bytes() } else { n.to_be_bytes() });
    let decodes = decode_summary(&m).is_ok();
    assert_eq!(decodes, RAWLER);
    let inv = inventory(&m);
    inv.validate().unwrap();
    let big: Vec<&Part> = inv
        .parts()
        .iter()
        .filter(|p| p.disposition.is_consumed() && p.range.end - p.range.start > 1000)
        .collect();
    assert_eq!(big.is_empty(), !RAWLER, "{big:?}");
}

/// A CR3 CMT1 TIFF whose walk stops at the IFD cap, and an mdat after a
/// box-budget stop: their unlisted bytes may still be read, so they are
/// `Unknown`, not `Unreferenced`.
#[test]
fn cr3_budget_stops_leave_cmt_and_mdat_bytes_unknown() {
    fn bx(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(fourcc);
        v.extend_from_slice(payload);
        v
    }
    let ftyp = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
    let may_read = |p: &Part| {
        p.disposition == Disposition::Unknown
            && p.detail.as_deref().is_some_and(|d| d.contains("may read"))
    };
    // CMT1: SubIFDs naming 5000 one-entry IFDs.
    const CANON_UUID: [u8; 16] = [
        0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a,
        0x48,
    ];
    let mut w = W::new();
    let subs: Vec<u32> = (0..5000u32).map(|k| w.ifd(&[long(254, k)], 0)).collect();
    let arr = w.put(
        &subs
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    );
    let ifd0 = w.ifd(&[at(330, 4, 5000, arr)], 0);
    w.b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    let mut canon = CANON_UUID.to_vec();
    canon.extend_from_slice(&bx(b"CMT1", &w.b));
    let mut data = ftyp.clone();
    let moov_at = data.len();
    data.extend_from_slice(&bx(b"moov", &bx(b"uuid", &canon)));
    let tiff_at = (moov_at + 8 + 8 + 16 + 8) as u64;
    let inv = inventory(&data);
    inv.validate().unwrap();
    let last = leaf_at(&inv, tiff_at + u64::from(*subs.last().unwrap()));
    assert!(may_read(last), "{last:?}");
    // A moov of 4100 boxes stops the box budget before the mdat.
    let mut moov = Vec::new();
    for _ in 0..4100 {
        moov.extend_from_slice(&bx(b"free", &[]));
    }
    let mut data = ftyp;
    data.extend_from_slice(&bx(b"moov", &moov));
    let mdat_at = data.len() as u64;
    data.extend_from_slice(&bx(b"mdat", b"MDAT-PAYLOAD-0042"));
    let inv = inventory(&data);
    inv.validate().unwrap();
    let payload = leaf_at(&inv, mdat_at + 8);
    assert!(may_read(payload), "{payload:?}");
}

/// rawler files an unknown-type pointer as `vec![offsets[0]]`, which panics
/// for count 0 (formats/tiff/ifd.rs:157); rawler catches the panic and the
/// decode fails. rawloader ignores the entry.
#[test]
fn rawler_count0_unknown_type_pointer_fails_the_decode() {
    let mut w = W::new();
    let s0 = w.put(&raw16(0));
    let raw = w.ifd(&raw_entries(s0), 0);
    let data = preview_ifd0(w, &[long(330, raw), at(34665, 13, 0, 0)]);
    let consumed = strip_consumed("count-0 IFD-typed ExifIFD", &data, s0);
    assert_eq!(consumed, !RAWLER);
}
