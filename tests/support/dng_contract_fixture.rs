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

