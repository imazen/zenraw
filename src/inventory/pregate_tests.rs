//! Structural tests whose fixtures the backends' own container parse
//! rejects (an unknown RAF camera, a TIFF without Model, CR3 files without
//! CMT boxes). Their assertions are about the walk's classification, so they
//! run against the walk before that gate (`inventory_with(.., false)`); the
//! gate's verdict on the same files is tested in `tests/inventory.rs`
//! (`the_gate_rejects_undecodable_fixtures`).
#![allow(dead_code)]

use zencodec::inventory::{Disposition, Inventory, MetadataKind, Part, PartKind, PartTag};

use crate::RawDecoderConfig;

/// The inventory before the backend's container parse decides rejection.
fn inventory(data: &[u8]) -> Inventory {
    super::inventory_with(
        data,
        crate::zencodec_impl::detect_format(data),
        &enough::Unstoppable,
        false,
    )
    .unwrap_or_else(|_| panic!("inventory failed"))
}

include!("../../tests/support/inventory_fixtures.rs");

/// `zencodec_testkit::check_inventory`'s checks, on the pre-gate walk: the
/// testkit takes a decoder config, so it always sees the gated inventory.
/// The inventory covers the input, is in a declared format, validates and
/// attributes some part to image data; appended junk stays unconsumed and
/// every truncation still validates (`check_inventory_without_image_data`).
fn check_inventory(data: &[u8]) -> Result<(), String> {
    use zencodec::decode::DecoderConfig;
    let inv = inventory(data);
    if !RawDecoderConfig::formats().contains(&inv.format()) {
        return Err(format!("format {} is not declared", inv.format()));
    }
    if !inv
        .parts()
        .iter()
        .any(|p| p.disposition == Disposition::ImageData)
    {
        return Err(format!("no part is attributed to image data\n{inv}"));
    }
    check_inventory_without_image_data(data)
}

#[test]
fn raf_fixture_passes_check_inventory() {
    let data = raf_fixture();
    check_inventory(&data).unwrap();
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
        check_inventory(&data).unwrap();
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

/// rawloader picks the raw IFD per Make (`get_decoder`): an ARW decodes the
/// first IFD with strips, a NEF the first with a CFA pattern.
#[cfg(not(feature = "rawler"))]
#[test]
fn raw_ifd_follows_the_make() {
    for (make, first_strip_is_raw) in MAKE_ORDER {
        let (data, thumb, raw) = make_order_fixture(make);
        let inv = inventory(&data);
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

/// rawler reads only the boxes its BMFF reader knows; the payload of any
/// other box is seeked past.
#[cfg(feature = "rawler")]
#[test]
fn rawler_skips_cr3_boxes_it_does_not_parse() {
    let (f, top) = cr3_unparsed_boxes_fixture();
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
    let (f, at) = cr3_xmp_uuid_fixture();
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
