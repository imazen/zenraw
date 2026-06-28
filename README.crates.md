<!-- GENERATED FROM README.md by zenutils gen-readme-crates.sh — DO NOT EDIT. -->

# zenraw

Camera RAW and DNG decoder in pure Rust (`#![forbid(unsafe_code)]`).
Display-ready sRGB output by default (`OutputMode::Develop`, u16); scene-referred
linear f32 is opt-in (`OutputMode::Linear`). Three swappable backends trade camera
coverage against dependency weight.

## Quick start

```toml
[dependencies]
zenraw = "0.2.0"
```

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

`decode` returns `Result`, and **both backends are panic-isolated**: each wraps
its underlying parser in `std::panic::catch_unwind` and converts a backend panic
into `RawError::Decode(...)` (and the decode path also rejects inputs shorter
than 64 bytes up front). So with either backend a malformed file is *expected* to
come back as `Err`, not a host crash.

That guard is not total, and you should not rely on it alone for hostile uploads:

- `catch_unwind` cannot stop an *abort* (a `panic = "abort"` profile, a
  double-panic, or an allocation failure under that profile), and the broader
  decode path has not been exhaustively proven panic-free on adversarial input.

The big sensor-sized allocations (the normalized sensor buffer, the demosaiced
`RGB f32` buffer, and the crop copy) go through a **fallible** allocation path:
an out-of-memory allocation returns `RawError::LimitExceeded` rather than
aborting. Combined with the up-front `with_max_pixels` / `with_max_decode_bytes`
caps, a crafted header that demands gigabytes is rejected before allocating, or
fails gracefully if it slips past. (With the `zencodec` feature, the fallibility
is driven by `ResourceLimits::prefer_fallible_allocations`.)

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
use zenraw::{RawDecodeConfig, DemosaicMethod, OutputMode, OutputPrimaries};

let config = RawDecodeConfig::default()
    .with_demosaic(DemosaicMethod::MalvarHeCutler) // or Bilinear
    .with_output(OutputMode::Linear)               // Develop (u16 sRGB, default) | Linear (f32) | CameraRaw (f32)
    .with_target(OutputPrimaries::DisplayP3)       // Srgb (default) | DisplayP3 | Bt2020 (Develop/Linear only)
    .with_exposure_ev(0.0)                          // exposure compensation in stops (2^ev); Develop/Linear only
    .with_wb([1.0, 1.0, 1.0])                       // override the as-shot white balance (RGB multipliers)
    .with_crop(true)                               // use the camera's crop / active area
    .with_orientation(true)                        // apply the EXIF rotation/flip
    .with_max_pixels(120_000_000)                  // reject images above this (width × height); default 200 MP
    .with_max_decode_bytes(1024 * 1024 * 1024);    // cap the intermediate RGB-f32 working set; default 1 GiB
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

The zencodec integration populates `ImageInfo` with orientation, bit depth, and
XMP metadata (when the `xmp` feature is enabled), honors `OrientationHint`
(default `Preserve`), and routes allocations through
`ResourceLimits::prefer_fallible_allocations`. The adapter also implements
`estimate_decode_resources`, predicting peak memory (≈ 3× the `RGB f32` working
set plus fixed overhead), serial threading, and wall-time for resource-aware
schedulers.


## AI-Generated Code Notice

Developed with Claude (Anthropic). Not all code manually reviewed.
Review critical paths before production use.

## License

Dual-licensed: [AGPL-3.0](https://github.com/imazen/zenraw/blob/main/LICENSE-AGPL3)
or [commercial](https://github.com/imazen/zenraw/blob/main/LICENSE-COMMERCIAL).

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

See [LICENSE-COMMERCIAL](https://github.com/imazen/zenraw/blob/main/LICENSE-COMMERCIAL)
for details.

## Image tech I maintain

| | |
|:--|:--|
| **Codecs** ¹ | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] · [zenjxl] · [zenbitmaps] · [heic] · [zentiff] · [zenpdf] · [zensvg] · [zenjp2] · **zenraw** · [ultrahdr] |
| Codec internals | [zenjxl-decoder] · [jxl-encoder] · [zenrav1e] · [rav1d-safe] · [zenavif-parse] · [zenavif-serialize] |
| Compression | [zenflate] · [zenzop] · [zenzstd] |
| Processing | [zenresize] · [zenquant] · [zenblend] · [zenfilters] · [zensally] · [zentone] |
| Pixels & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline & framework | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] · [zenwasm] · [zentract] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [zenmetrics] · [resamplescope-rs] |
| Pickers & ML | [zenanalyze] · [zenpredict] · [zenpicker] |
| Products | [Imageflow] image engine ([.NET][imageflow-dotnet] · [Node][imageflow-node] · [Go][imageflow-go]) · [Imageflow Server] · [ImageResizer] (C#) |

<sub>¹ pure-Rust, `#![forbid(unsafe_code)]` codecs, as of 2026</sub>

### General Rust awesomeness

[zenbench] · [archmage] · [magetypes] · [enough] · [whereat] · [cargo-copter]

[Open source](https://www.imazen.io/open-source) · [@imazen](https://github.com/imazen) · [@lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith)

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zenjxl]: https://github.com/imazen/zenjxl
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic
[zentiff]: https://github.com/imazen/zentiff
[zenpdf]: https://github.com/imazen/zenpdf
[zensvg]: https://github.com/imazen/zenextras
[zenjp2]: https://github.com/imazen/zenextras
[ultrahdr]: https://github.com/imazen/ultrahdr
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenrav1e]: https://github.com/imazen/zenrav1e
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenavif-serialize]: https://github.com/imazen/zenavif-serialize
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenzstd]: https://github.com/imazen/zenzstd
[zenresize]: https://github.com/imazen/zenresize
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zenfilters]: https://github.com/imazen/zenfilters
[zensally]: https://github.com/imazen/zensally
[zentone]: https://github.com/imazen/zentone
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zencodecs
[zenlayout]: https://github.com/imazen/zenlayout
[zennode]: https://github.com/imazen/zennode
[zenwasm]: https://github.com/imazen/zenwasm
[zentract]: https://github.com/imazen/zentract
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenmetrics]: https://github.com/imazen/zenmetrics
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[zenanalyze]: https://github.com/imazen/zenanalyze
[zenpredict]: https://github.com/imazen/zenanalyze
[zenpicker]: https://github.com/imazen/zenanalyze
[zenbench]: https://github.com/imazen/zenbench
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[cargo-copter]: https://github.com/imazen/cargo-copter
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-dotnet-server
[ImageResizer]: https://github.com/imazen/resizer
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
