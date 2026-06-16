# zenraw [![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenraw/ci.yml?style=flat-square)](https://github.com/imazen/zenraw/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/zenraw.svg?style=flat-square)](https://crates.io/crates/zenraw) [![docs.rs](https://img.shields.io/docsrs/zenraw?style=flat-square)](https://docs.rs/zenraw) [![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg?style=flat-square)](https://github.com/imazen/zenraw#license) [![MSRV: 1.93](https://img.shields.io/badge/MSRV-1.93-blue.svg?style=flat-square)](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field)

Camera RAW and DNG decoder in safe Rust. Display-ready sRGB output by default
(`OutputMode::Develop`, u16); scene-referred linear f32 is opt-in
(`OutputMode::Linear`). Three swappable backends for different camera coverage
vs. dependency tradeoffs.

## Quick start

```rust
use zenraw::{decode, RawDecodeConfig};
use enough::Unstoppable;

let data: &[u8] = &[/* RAW file bytes */];
let output = decode(data, &RawDecodeConfig::default(), &Unstoppable)?;
println!("{}x{} {} {}", output.info.width, output.info.height,
    output.info.make, output.info.model);
// output.pixels is a `zenpixels::PixelBuffer`. The default OutputMode::Develop
// produces display-ready u16 sRGB (3×u16 per pixel) — NOT 8-bit and NOT linear.
```

Pick the output representation with `with_output` (`RawDecodeConfig` is
`#[non_exhaustive]`, so configure it with the `with_*` builders, not a struct
literal):

```rust
use zenraw::OutputMode;

// Scene-referred linear f32 (white-balanced, color-matrixed):
let config = RawDecodeConfig::default().with_output(OutputMode::Linear);
let output = decode(data, &config, &Unstoppable)?;
// output.pixels is now f32 linear RGB. (OutputMode::Develop = u16 sRGB [default],
//  OutputMode::CameraRaw = raw camera values as f32, no color processing.)
```

## Reading the pixels out

`output.pixels` is a
[`zenpixels::PixelBuffer`](https://docs.rs/zenpixels/latest/zenpixels/struct.PixelBuffer.html).
To get at the samples, ask it for the contiguous backing bytes and reinterpret
them for the channel type the `OutputMode` produced. For `OutputMode::Linear`
(and `CameraRaw`) that's interleaved **f32 RGB**; for the default `Develop` it's
interleaved **u16 sRGB**.

```rust
use zenraw::{decode, OutputMode, RawDecodeConfig};
use enough::Unstoppable;

let data: &[u8] = &[/* RAW file bytes */];
let config = RawDecodeConfig::default().with_output(OutputMode::Linear);
let output = decode(data, &config, &Unstoppable)?;

let w = output.pixels.width() as usize;
let h = output.pixels.height() as usize;

// The RGB-f32 buffer is tightly packed (stride == width * 12 bytes), so the
// zero-copy `as_contiguous_bytes()` always returns `Some` for this format.
let bytes: &[u8] = output.pixels.as_contiguous_bytes().unwrap();
let rgb: &[f32] = bytemuck::cast_slice(bytes); // 3 floats per pixel, R,G,B

assert_eq!(rgb.len(), w * h * 3);

// Pixel (x, y), channel order R, G, B:
let pixel = |x: usize, y: usize| {
    let i = (y * w + x) * 3;
    (rgb[i], rgb[i + 1], rgb[i + 2])
};
let (r, g, b) = pixel(0, 0);
```

Key facts about the layout and value range (verified against the decode path):

- **Channel order is interleaved `R, G, B`** — three samples per pixel, no
  alpha, in `width * height * 3` order (row-major, top-to-bottom). The pixel
  format is `PixelDescriptor::RGBF32_LINEAR` (`ChannelType::F32`,
  `ChannelLayout::Rgb`, `TransferFunction::Linear`).
- **`OutputMode::Linear` is scene-referred** — white-balanced and
  colour-matrixed, but **not** clamped to `[0, 1]`. Highlights routinely exceed
  `1.0`; expect to tone-map or clip yourself before display. `CameraRaw` is also
  f32 but carries raw camera values with no colour processing.
- **`OutputMode::Develop` (the default)** is display-ready **u16 sRGB**: read it
  the same way but `bytemuck::cast_slice::<u8, u16>(bytes)` for 3×`u16` per pixel
  in `[0, 65535]`.

If you'd rather own the bytes (e.g. to hand off to a thread or FFI), use
`output.pixels.copy_to_contiguous_bytes()` for a fresh `Vec<u8>` with stride
padding stripped, or `output.pixels.into_vec()` to consume the buffer. To walk
row by row, `output.pixels.as_slice().row(y)` returns one row's
`width * bytes_per_pixel` bytes. (`width()` / `height()` / `stride()` /
`descriptor()` / `as_contiguous_bytes()` / `copy_to_contiguous_bytes()` /
`as_slice()` are all on `PixelBuffer`.)

## Cancellation (`Stop`)

`decode`'s third argument is a
[`&dyn enough::Stop`](https://docs.rs/enough/latest/enough/trait.Stop.html) — the
cooperative-cancellation / deadline hook. Pass the no-op when you don't need it:

```rust
use enough::Unstoppable;
let output = decode(data, &RawDecodeConfig::default(), &Unstoppable)?;
```

For a token you can cancel from another thread (timeout, request abort, etc.),
use [`almost_enough::Stopper`](https://docs.rs/almost-enough). `Stopper` itself
implements `Stop` (so you pass `&stopper` straight to `decode`) and is a cheap
`Arc`-backed handle — clone it to share the cancellation state across threads,
then `cancel()` from any clone:

```rust
use almost_enough::Stopper;

let stopper = Stopper::new();
let watcher = stopper.clone();      // hand the clone to a timeout/abort task

// e.g. on another thread / after a deadline: `watcher.cancel();`

match decode(data, &RawDecodeConfig::default(), &stopper) {
    Ok(output) => { /* … */ }
    Err(e) => match e.error() {
        // cancellation surfaces as `RawError::Stopped(enough::StopReason)`
        zenraw::RawError::Stopped(_) => { /* cancelled / deadline hit */ }
        _ => return Err(e),
    },
}
```

`decode` checks the token between pipeline stages, so cancellation is bounded by
how long a single stage runs. Errors are `whereat::At<RawError>`; reach the
underlying `RawError` with `.error()`. Add `almost-enough = "0.4.4"` for the
cancellable `Stopper`; `enough` (the `Unstoppable` no-op and the `Stop` trait)
comes in via zenraw, which depends on `enough` 0.4.

## Decoding untrusted input (panic safety)

`decode` returns `Result`, and the **default `rawloader` backend** wraps the
underlying parser in `std::panic::catch_unwind` and converts a backend panic
into `RawError::Decode(...)` (it also rejects inputs shorter than 64 bytes up
front). So with the default features a malformed file is *expected* to come back
as `Err`, not a panic.

That guard is not total, and you should not rely on it alone for hostile uploads:

- The **`rawler` backend does not** wrap its decode in `catch_unwind` — a panic
  inside rawler propagates to your caller.
- `catch_unwind` cannot stop an *abort* (a `panic = "abort"` profile, a
  double-panic, or an allocation failure under that profile), and the broader
  decode path has not been exhaustively proven panic-free on adversarial input.

For a server decoding untrusted RAW, wrap the call so a panic can't take down the
worker — run it on an isolated thread (a panicking thread unwinds without killing
the process) or in your own `std::panic::catch_unwind`, and keep the resource
caps (`with_max_pixels` / `with_max_decode_bytes`) tight:

```rust
let result = std::thread::spawn(move || {
    decode(&data, &config, &Unstoppable)
})
.join(); // `Err(_)` here means the decode thread panicked
```

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

`RawDecodeConfig` is `#[non_exhaustive]`; build it with the `with_*` builders:

```rust
use zenraw::{RawDecodeConfig, DemosaicMethod, OutputMode};

let config = RawDecodeConfig::default()
    .with_demosaic(DemosaicMethod::MalvarHeCutler) // or Bilinear
    .with_output(OutputMode::Linear)               // Develop (u16 sRGB, default) | Linear (f32) | CameraRaw (f32)
    .with_crop(true)                               // use the camera's crop / active area
    .with_orientation(true)                        // apply the EXIF rotation/flip
    .with_max_pixels(120_000_000)                  // reject images above this (width × height)
    .with_max_decode_bytes(1024 * 1024 * 1024);    // cap the intermediate RGB-f32 working set (server DoS guard)
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

## Image tech I maintain

| | |
|:--|:--|
| State of the art codecs* | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] ([rav1d-safe] · [zenrav1e] · [zenavif-parse] · [zenavif-serialize]) · [zenjxl] ([jxl-encoder] · [zenjxl-decoder]) · [zentiff] · [zenbitmaps] · [heic] · **zenraw** · [zenpdf] · [ultrahdr] · [mozjpeg-rs] · [webpx] |
| Compression | [zenflate] · [zenzop] |
| Processing | [zenresize] · [zenfilters] · [zenquant] · [zenblend] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [resamplescope-rs] · [codec-eval] · [codec-corpus] |
| Pixel types & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] |
| ImageResizer | [ImageResizer] (C#) — 24M+ NuGet downloads across all packages |
| [Imageflow][] | Image optimization engine (Rust) — [.NET][imageflow-dotnet] · [node][imageflow-node] · [go][imageflow-go] — 9M+ NuGet downloads across all packages |
| [Imageflow Server][] | [The fast, safe image server](https://www.imazen.io/) (Rust+C#) — 552K+ NuGet downloads, deployed by Fortune 500s and major brands |

<sub>* as of 2026</sub>

### General Rust awesomeness

[archmage] · [magetypes] · [enough] · [whereat] · [zenbench] · [cargo-copter]

[And other projects](https://www.imazen.io/open-source) · [GitHub @imazen](https://github.com/imazen) · [GitHub @lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith) · [NuGet](https://www.nuget.org/profiles/imazen) (over 30 million downloads / 87 packages)

[zenjpeg]: https://crates.io/crates/zenjpeg
[zenpng]: https://crates.io/crates/zenpng
[zenwebp]: https://crates.io/crates/zenwebp
[zengif]: https://crates.io/crates/zengif
[zenavif]: https://crates.io/crates/zenavif
[rav1d-safe]: https://crates.io/crates/rav1d-safe
[zenrav1e]: https://crates.io/crates/zenrav1e
[zenavif-parse]: https://crates.io/crates/zenavif-parse
[zenavif-serialize]: https://crates.io/crates/zenavif-serialize
[zenjxl]: https://crates.io/crates/zenjxl
[jxl-encoder]: https://crates.io/crates/jxl-encoder
[zenjxl-decoder]: https://crates.io/crates/zenjxl-decoder
[zentiff]: https://crates.io/crates/zentiff
[zenbitmaps]: https://crates.io/crates/zenbitmaps
[heic]: https://crates.io/crates/heic
[zenpdf]: https://crates.io/crates/zenpdf
[ultrahdr]: https://crates.io/crates/ultrahdr
[mozjpeg-rs]: https://crates.io/crates/mozjpeg-rs
[webpx]: https://crates.io/crates/webpx
[zenflate]: https://crates.io/crates/zenflate
[zenzop]: https://crates.io/crates/zenzop
[zenresize]: https://crates.io/crates/zenresize
[zenfilters]: https://crates.io/crates/zenfilters
[zenquant]: https://crates.io/crates/zenquant
[zenblend]: https://crates.io/crates/zenblend
[zensim]: https://crates.io/crates/zensim
[fast-ssim2]: https://crates.io/crates/fast-ssim2
[butteraugli]: https://crates.io/crates/butteraugli
[resamplescope-rs]: https://crates.io/crates/resamplescope-rs
[codec-eval]: https://crates.io/crates/codec-eval
[codec-corpus]: https://crates.io/crates/codec-corpus
[zenpixels]: https://crates.io/crates/zenpixels
[zenpixels-convert]: https://crates.io/crates/zenpixels-convert
[linear-srgb]: https://crates.io/crates/linear-srgb
[garb]: https://crates.io/crates/garb
[zenpipe]: https://crates.io/crates/zenpipe
[zencodec]: https://crates.io/crates/zencodec
[zencodecs]: https://crates.io/crates/zencodecs
[zenlayout]: https://crates.io/crates/zenlayout
[zennode]: https://crates.io/crates/zennode
[ImageResizer]: https://imageresizing.net
[Imageflow]: https://github.com/imazen/imageflow
[imageflow-dotnet]: https://www.nuget.org/packages/Imageflow.AllPlatforms
[imageflow-node]: https://www.npmjs.com/package/@imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
[Imageflow Server]: https://github.com/imazen/imageflow-dotnet-server
[archmage]: https://crates.io/crates/archmage
[magetypes]: https://crates.io/crates/magetypes
[enough]: https://crates.io/crates/enough
[whereat]: https://crates.io/crates/whereat
[zenbench]: https://crates.io/crates/zenbench
[cargo-copter]: https://crates.io/crates/cargo-copter

## License

Dual-licensed: [AGPL-3.0](LICENSE-AGPL3) or [commercial](LICENSE-COMMERCIAL).

I've maintained and developed open-source image server software — and the 40+
library ecosystem it depends on — full-time since 2011. Fifteen years of
continual maintenance, backwards compatibility, support, and the (very rare)
security patch. That kind of stability requires sustainable funding, and
dual-licensing is how we make it work without venture capital or rug-pulls.
Support sustainable and secure software; swap patch tuesday for patch leap-year.

[Our open-source products](https://www.imazen.io/open-source)

**Your options:**

- **Startup license** — $1 if your company has under $1M revenue and fewer
  than 5 employees. [Get a key →](https://www.imazen.io/pricing)
- **Commercial subscription** — Governed by the Imazen Site-wide Subscription
  License v1.1 or later. Apache 2.0-like terms, no source-sharing requirement.
  Sliding scale by company size.
  [Pricing & 60-day free trial →](https://www.imazen.io/pricing)
- **AGPL v3** — Free and open. Share your source if you distribute.

See [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.
