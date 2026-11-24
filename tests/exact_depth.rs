#![cfg(any(feature = "rawloader", feature = "rawler"))]
#[path = "support/dng_builder.rs"]
mod dng_builder;
use dng_builder::*;
#[cfg(feature = "zencodec")]
use std::borrow::Cow;
#[cfg(feature = "zencodec")]
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
#[cfg(feature = "zencodec")]
use zencodec::{Orientation, OrientationHint};
use zenraw::{OutputMode, RawDecodeConfig};

fn ten_bit_sensor() -> Vec<u8> {
    let entries = vec![
        short(0x100, 8),
        short(0x101, 8),
        short(0x102, 16),
        short(0x103, 1),
        short(0x106, 32803),
        ascii(0x10f, "ZenRaw"),
        ascii(0x110, "PrecisionFixture"),
        long(0x111, 0),
        short(0x115, 1),
        long(0x116, 8),
        long(0x117, 128),
        short(0x11c, 1),
        bytes(0xc612, &[1, 4, 0, 0]),
        long(0xc61d, 1023),
        Entry {
            tag: 0x828d,
            kind: SHORT,
            count: 2,
            value: [2u16, 2].iter().flat_map(|v| v.to_le_bytes()).collect(),
        },
        bytes(0x828e, &[0, 1, 1, 2]),
    ];
    let mut strip: Vec<u8> = (0..64u16)
        .flat_map(|i| ((i * 61 + 17) % 1024).to_le_bytes())
        .collect();
    strip.extend_from_slice(&[0; 64]);
    build_tiff(entries, &strip, 0x111)
}

#[test]
fn developed_grid_is_measured_and_float_modes_carry_no_integer_statement() {
    let data = ten_bit_sensor();
    for mode in [
        OutputMode::Develop,
        OutputMode::Linear,
        OutputMode::CameraRaw,
    ] {
        let config = RawDecodeConfig::new().with_output(mode).with_crop(false);
        let native = zenraw::decode(&data, &config, None).unwrap();
        assert_eq!(native.info.bit_depth, Some(10));
        let exact = native.pixels.facts().exact;
        if mode == OutputMode::Develop {
            assert_eq!(
                exact.effective_bits, 16,
                "demosaicing/color processing do not inherit the sensor grid"
            );
            assert!(exact.all_opaque);
            let view = native.pixels.as_view();
            let samples = view.as_contiguous_as::<u16>().unwrap();
            // An independent full-range reconstruction check rejects every
            // smaller candidate depth for the delivered samples.
            for bits in 1..exact.effective_bits {
                let max = f64::from((1u16 << bits) - 1);
                assert!(samples.iter().any(|&v| {
                    let code = (f64::from(v) * max / 65535.0).round();
                    (code * 65535.0 / max).round() as u16 != v
                }));
            }
        } else {
            assert_eq!(exact.effective_bits, 0);
        }
        #[cfg(feature = "zencodec")]
        for hint in [
            OrientationHint::Preserve,
            OrientationHint::ExactTransform(Orientation::FlipH),
        ] {
            let decoded = zenraw::RawDecoderConfig::from_config(config.clone())
                .job()
                .with_orientation(hint)
                .decoder(Cow::Borrowed(&data), &[])
                .unwrap()
                .decode()
                .unwrap();
            let pixels = decoded.pixels();
            assert_eq!(decoded.info().source_color.bit_depth, Some(10));
            assert_eq!(
                pixels.facts(),
                native.pixels.facts(),
                "orientation retains sample evidence"
            );
            assert_eq!(pixels.spec(), native.pixels.spec());
            if hint == OrientationHint::Preserve {
                assert_eq!(
                    pixels.try_contiguous_bytes().unwrap(),
                    native.pixels.as_view().try_contiguous_bytes().unwrap(),
                );
            }
        }
    }
}
