// Fixture builders shared by `tests/inventory.rs` and the crate's own
// pre-gate tests (`src/inventory/pregate_tests.rs`), pulled in with
// `include!`. Each includer supplies its own `inventory(&[u8]) -> Inventory`
// (the zencodec job, or the walk before the backend's container parse) and
// the imports these items use.

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

/// The makes of `make_order_fixture`, each with whether its decoder takes
/// the first IFD with strips (SONY) or the first with a CFA pattern (NIKON).
const MAKE_ORDER: [(&[u8], bool); 2] = [(b"SONY\0", true), (b"NIKON CORPORATION\0", false)];

/// A TIFF with `make`: IFD0 holds a 12-byte thumbnail strip, its SubIFD a
/// 4x4 16-bit CFA strip. Returns the file and both strip offsets.
fn make_order_fixture(make: &[u8]) -> (Vec<u8>, u32, u32) {
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
    (w.b, thumb, raw)
}

/// One ISOBMFF box.
fn bx(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(fourcc);
    v.extend_from_slice(payload);
    v
}

/// A CR3-shaped file whose moov holds an mvhd and a private box, followed
/// by a top-level PRVW box and an mdat. Returns the file and the PRVW
/// box's offset.
fn cr3_unparsed_boxes_fixture() -> (Vec<u8>, u64) {
    let mut moov = bx(b"mvhd", &[0; 20]);
    moov.extend(bx(b"zzzz", b"PRIVATE-SERIAL-0"));
    let mut f = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
    f.extend(bx(b"moov", &moov));
    let top = f.len() as u64;
    f.extend(bx(b"PRVW", b"PREVIEW-PAYLOAD!"));
    f.extend(bx(b"mdat", &[0x5A; 64]));
    (f, top)
}

/// A CR3-shaped file with an XMP packet in Adobe's XMP uuid box. Returns
/// the file and the packet's offset.
fn cr3_xmp_uuid_fixture() -> (Vec<u8>, u64) {
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
    (f, at)
}
