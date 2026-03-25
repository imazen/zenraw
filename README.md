# zenraw

[![CI](https://github.com/imazen/zenraw/actions/workflows/ci.yml/badge.svg)](https://github.com/imazen/zenraw/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/zenraw.svg?style=for-the-badge)](https://crates.io/crates/zenraw)
[![docs.rs](https://img.shields.io/docsrs/zenraw?style=for-the-badge)](https://docs.rs/zenraw)
[![License: AGPL-3.0-or-later](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg?style=for-the-badge)](LICENSE)
[![MSRV: 1.93](https://img.shields.io/badge/MSRV-1.93-blue.svg?style=for-the-badge)](https://blog.rust-lang.org/)

Camera RAW and DNG decoder in safe Rust. Scene-referred linear f32 output by default,
sRGB u8 opt-in. Three swappable backends for different camera coverage vs. dependency tradeoffs.

## Quick start

```rust
use zenraw::{decode, RawDecodeConfig};
use enough::Unstoppable;

let data: &[u8] = &[/* RAW file bytes */];
let output = decode(data, &RawDecodeConfig::default(), &Unstoppable)?;
println!("{}x{} {} {}", output.info.width, output.info.height,
    output.info.make, output.info.model);
// output.pixels is a PixelBuffer<RGBF32_LINEAR>
```

For display-referred sRGB u8 output:

```rust
let config = RawDecodeConfig::default().with_gamma(true);
let output = decode(data, &config, &Unstoppable)?;
// output.pixels is a PixelBuffer<RGB8_SRGB>
```

Cancellation and deadlines use [`enough::Stop`](https://docs.rs/enough) — pass
`&Unstoppable` when you don't need either.

## Decode pipeline

1. Parse camera RAW/DNG file (via rawloader or rawler)
2. Normalize sensor values using per-channel black/white levels
3. Demosaic Bayer CFA → RGB (Malvar-He-Cutler by default, bilinear available)
4. Demosaic X-Trans 6×6 CFA → RGB (bilinear, rawler backend only)
5. Apply white balance coefficients
6. Apply camera → XYZ → sRGB color matrix
7. Crop to active area (from camera metadata)
8. Apply EXIF orientation (rotation/flip)
9. Optionally apply sRGB gamma curve

## Backends

| Backend | Feature | Cameras | Formats | Notes |
|---------|---------|---------|---------|-------|
| **rawloader** | `rawloader` (default) | ~200 | Bayer only | Lightweight, LGPL-2.1 |
| **rawler** | `rawler` | ~300+ | Bayer + X-Trans, CR3, JXL DNG | Broader support, LGPL-2.1 |
| **darktable** | `darktable` | 900+ | Everything darktable handles | Shells out to darktable-cli |

When both `rawloader` and `rawler` are enabled, `rawler` takes priority.
The darktable backend is independent — it delegates the entire pipeline to
darktable-cli and returns its processed output.

## Supported formats

| Format | Extensions | Backend |
|--------|-----------|---------|
| Adobe DNG | `.dng` | rawloader, rawler |
| Canon | `.cr2`, `.cr3` | rawloader (CR2), rawler (CR2+CR3) |
| Nikon | `.nef`, `.nrw` | rawloader, rawler |
| Sony | `.arw`, `.srf`, `.sr2` | rawloader, rawler |
| Fujifilm | `.raf` | rawler (X-Trans + Bayer) |
| Panasonic/Leica | `.rw2` | rawloader, rawler |
| Pentax | `.pef` | rawloader, rawler |
| Olympus | `.orf` | rawloader, rawler |
| Hasselblad | `.3fr` | rawloader, rawler |
| Phase One | `.iiq` | rawloader, rawler |
| Epson | `.erf` | rawloader, rawler |

Plus many more via rawler. Detection works on file content, not extension.

## Features

| Feature | Default | Description |
|---------|---------|-------------|
| `std` | yes | Enable std (required for darktable, rawler) |
| `rawloader` | yes | rawloader decode backend |
| `ultrahdr` | yes | UltraHDR gain map support via ultrahdr-core |
| `rawler` | no | rawler decode backend (broader camera support) |
| `darktable` | no | darktable-cli backend (requires darktable installed) |
| `exif` | no | EXIF metadata extraction via kamadak-exif |
| `xmp` | no | XMP metadata extraction |
| `apple` | no | Apple APPLEDNG/AMPF metadata (implies `exif` + `xmp`) |
| `zencodec` | no | zencodec trait integration (DecoderConfig, ImageInfo) |

## Configuration

```rust
use zenraw::{RawDecodeConfig, DemosaicMethod};

let config = RawDecodeConfig {
    demosaic: DemosaicMethod::MalvarHeCutler, // or Bilinear
    apply_gamma: false,       // true → sRGB u8, false → linear f32
    apply_crop: true,         // use camera's crop/active area
    apply_orientation: true,  // apply EXIF rotation/flip
    max_pixels: 300_000_000,  // reject images above this
    ..Default::default()
};
```

## zencodec integration

With the `zencodec` feature, zenraw implements `DecoderConfig` for use in
format-agnostic decode pipelines. Two format definitions are exported:
`DNG_FORMAT` and `RAW_FORMAT`.

```rust
use zenraw::RawDecoderConfig;
use zencodec::decode::DecoderConfig;

let config = RawDecoderConfig::new();
let job = config.job();
let info = job.probe(data)?;
println!("{}x{}, orientation={}", info.width, info.height, info.orientation());
```

The zencodec integration populates `ImageInfo` with orientation, bit depth,
and XMP metadata (when the `xmp` feature is enabled).

## AI-Generated Code Notice

Developed with Claude (Anthropic). Not all code manually reviewed.
Review critical paths before production use.

## License

Licensed under the GNU Affero General Public License v3.0 or later ([LICENSE](LICENSE) or <https://www.gnu.org/licenses/agpl-3.0.html>).

Commercial licenses are available from [Imazen](https://imazen.io).

Note: the `rawloader` and `rawler` backends are LGPL-2.1 licensed. When you
enable either feature, your binary links against LGPL code.
