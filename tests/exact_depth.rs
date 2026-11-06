#![cfg(all(feature = "zencodec", any(feature = "rawloader", feature = "rawler")))]
#[path = "support/dng_builder.rs"]
mod dng_builder;
use dng_builder::*;
use std::borrow::Cow;
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
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
        if mode == OutputMode::Develop {
            let exact = native
                .pixels
                .exact_depth()
                .expect("developed integer samples have evidence");
            assert_eq!(
                exact.color_bits(),
                16,
                "demosaicing/color processing do not inherit the sensor grid"
            );
            assert!(exact.holds_for(&native.pixels.as_slice()).unwrap());
        } else {
            assert!(native.pixels.exact_depth().is_none());
        }
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
            if mode == OutputMode::Develop {
                let exact = pixels
                    .exact_depth()
                    .expect("orientation retains integer evidence");
                assert_eq!(exact.color_bits(), 16);
                assert!(exact.holds_for(&pixels).unwrap());
            } else {
                assert!(pixels.exact_depth().is_none());
            }
        }
    }
}
