# zenraw Architecture

## Module Structure

```
src/
├── lib.rs              # Public API: decode(), probe(), is_raw_file()
├── decode.rs           # Common types (RawDecodeConfig, RawInfo) + rawloader backend
├── rawler_backend.rs   # rawler-based backend (CR3, X-Trans, JXL DNG, 500+ cameras)
├── darktable.rs        # darktable-cli backend (900+ cameras, shells out to darktable-cli)
├── demosaic.rs         # Bayer CFA → RGB (bilinear, MHC)
├── color.rs            # WB + color matrix + gamma
├── dng_render.rs       # Full DNG rendering pipeline (PGTM, ForwardMatrix, ToneCurve, HSV LUT)
├── dt_sigmoid.rs       # darktable-compatible sigmoid tone mapper (scene-referred)
├── orient.rs           # EXIF orientation transforms (rotation/flip)
├── simd.rs             # SIMD-accelerated inner loops via archmage (AVX2+FMA, NEON)
├── classify.rs         # File format detection (DNG, APPLEDNG, AMPF, vendor RAW)
├── tiff_ifd.rs         # Minimal TIFF IFD walker (SubIFDs, DNG profile tags)
├── exif.rs             # EXIF + DNG metadata extraction via kamadak-exif (feature: exif)
├── xmp.rs              # XMP packet extraction from any file format (feature: xmp)
├── apple.rs            # Apple APPLEDNG/AMPF metadata: MakerNote, gain map, semantic mattes (feature: apple)
├── error.rs            # RawError enum
└── zencodec_impl.rs    # Optional zencodec trait integration (feature: zencodec)
```

## Data Flow

```
Raw file bytes (&[u8])
       │
       ├─── probe() ──→ RawInfo (metadata only, no pixel decode)
       │
       └─── decode() ──→ RawDecodeOutput { pixels: PixelBuffer, info: RawInfo }
                │
                ├── Backend A: rawloader (feature: rawloader, default)
                │       [1] rawloader::decode() → RawImage
                │           (parses container, decompresses, extracts metadata)
                │       [2] normalize_raw_data() → Vec<f32> [0,1]
                │           (black level subtract, white level scale)
                │       [3] demosaic_to_rgb_f32() → Vec<f32> [R,G,B,R,G,B,...]
                │           (CFA pattern → interleaved RGB, SIMD-accelerated)
                │       [4] apply_color_pipeline() → in-place
                │           (WB × cam_to_srgb matrix, clamp [0,1])
                │           or DngPipeline (PGTM, ForwardMatrix, ToneCurve)
                │       [5] apply_crop() + apply_orientation()
                │
                ├── Backend B: rawler (feature: rawler)
                │       [1] rawler::decode() → RawImage
                │           (CR3, X-Trans, LJPEG, JXL-compressed DNG, 500+ cameras)
                │       [2–5] Same normalize → demosaic → color → crop pipeline
                │
                └── Backend C: darktable (feature: darktable)
                        [1] Shell out to darktable-cli
                            (900+ cameras, full scene-referred pipeline)
                        [2] Read PFM interchange format (linear f32)
                        [3] Wrap into PixelBuffer (RGBF32_LINEAR)

                Final output conversion (RawDecodeConfig::output):
                    OutputMode::Develop   → RGB16_SRGB u16 (default; sRGB gamma + quantise)
                    OutputMode::Linear    → RGBF32_LINEAR f32 (scene-referred, unclamped)
                    OutputMode::CameraRaw → RGBF32_LINEAR f32, primaries Unknown (no WB/matrix)
                    (darktable backend: RGB8_SRGB or RGBF32_LINEAR, whatever darktable-cli wrote)
```

## Public API

```rust
// Full decode
pub fn decode(data: &[u8], config: &RawDecodeConfig, stop: &dyn Stop)
    -> Result<RawDecodeOutput>;

// Metadata only (fast)
pub fn probe(data: &[u8], stop: &dyn Stop) -> Result<RawInfo>;

// Format detection
pub fn is_raw_file(data: &[u8]) -> bool;
```

### Configuration

```rust
#[non_exhaustive]                        // build with the with_* builders, not a literal
pub struct RawDecodeConfig {
    pub demosaic: DemosaicMethod,        // Bilinear or MalvarHeCutler
    pub max_pixels: u64,                 // resource cap (default 200M) → LimitExceeded(Pixels)
    pub max_decode_bytes: u64,           // RGB f32 working-set cap (default 1 GiB) → LimitExceeded(Memory)
    pub output: OutputMode,              // Develop (u16 sRGB, default) | Linear (f32) | CameraRaw (f32)
    pub target: OutputPrimaries,         // Srgb | DisplayP3 | Bt2020 (Develop/Linear only)
    pub exposure_ev: f32,                // exposure compensation in stops (Develop/Linear only)
    pub apply_crop: bool,                // apply camera crop metadata
    pub apply_orientation: bool,         // apply EXIF orientation (default true)
    pub wb_override: Option<[f32; 3]>,   // replace camera as-shot WB with custom multipliers
}
```

### Output

```rust
pub struct RawDecodeOutput {
    pub pixels: PixelBuffer,         // untyped: RGB16_SRGB (Develop) or RGBF32_LINEAR (Linear/CameraRaw)
    pub info: RawInfo,
}

pub struct RawInfo {
    // Image geometry
    pub width: u32, pub height: u32,       // after crop and orientation
    pub sensor_width: u32, pub sensor_height: u32,
    pub cfa_pattern: String,               // e.g. "RGGB"
    pub sensor_layout: SensorLayout,       // Bayer, XTrans, LinearRaw, Unknown
    pub orientation: u16,                  // EXIF orientation 1–8
    pub bit_depth: Option<u8>,             // 10, 12, 14, etc.
    // Camera identity
    pub make: String, pub model: String,
    pub is_dng: bool,
    // Raw pipeline metadata
    pub wb_coeffs: [f32; 4],               // as-shot WB multipliers (RGBE)
    pub color_matrix: [[f32; 3]; 4],       // camera→XYZ (4×3 row-major)
    pub black_levels: [f32; 4],            // per-channel black level (RGBE, DN)
    pub white_levels: [f32; 4],            // per-channel white level (RGBE, DN)
    pub crop_rect: Option<[u32; 4]>,       // [top, right, bottom, left]
    pub active_area: Option<[u32; 4]>,     // [x, y, width, height]
    pub baseline_exposure: Option<f64>,    // DNG BaselineExposure in EV
}
```

## Integration with Zen Ecosystem

### As input to zenfilters

```rust
// Decode raw to linear RGB
let output = zenraw::decode(&raw_bytes, &config, &stop)?;

// Convert to Oklab for filtering
let (w, h) = (output.info.width as usize, output.info.height as usize);
let linear_rgb: &[f32] = output.pixels.as_slice_f32();
let mut planes = OklabPlanes::new(w, h);
let m1 = rgb_to_lms_matrix(ColorPrimaries::BT709);
scatter_to_oklab(linear_rgb, &mut planes, 3, &m1, 1.0);

// Apply filter pipeline
pipeline.apply(&mut planes, &mut ctx);

// Gather back to linear RGB
let mut out_rgb = vec![0.0f32; w * h * 3];
let m1_inv = lms_to_rgb_matrix(ColorPrimaries::BT709);
gather_from_oklab(&planes, &mut out_rgb, 3, &m1_inv, 1.0);
```

### Via zencodec (optional feature)

With the `zencodec` feature, zenraw implements codec traits for automatic format negotiation in the zenpipe pipeline system.

## Design Principles

1. **`#![forbid(unsafe_code)]`** — no exceptions
2. **`no_std + alloc`** — std opt-in for I/O (rawloader requires std)
3. **Cooperative cancellation** — `Stop` tokens throughout for responsive cancellation
4. **Non-exhaustive types** — future-proof public API
5. **Display-ready default, scene-referred opt-in** — `OutputMode::Develop` (u16 sRGB) by default; `OutputMode::Linear` for unclamped linear f32

## Dependencies

| Crate | Purpose | License |
|-------|---------|---------|
| rawloader 0.37.1 | Raw format parsing (~200 cameras, default backend) | LGPL-2.1 |
| rawler 0.7.2 | Broader camera support: CR3, X-Trans, JXL DNG (optional) | LGPL-2.1 |
| archmage 0.9.12 | SIMD dispatch (AVX2+FMA, NEON) | MIT OR Apache-2.0 |
| magetypes 0.9.12 | Shared SIMD vector types | MIT OR Apache-2.0 |
| bytemuck 1.25.0 | Safe bit-cast for pixel buffer reinterpretation | MIT OR Apache-2.0 |
| ultrahdr-core 0.2.0 | UltraHDR gain map support (optional) | Apache-2.0 |
| kamadak-exif 0.6.1 | EXIF/DNG metadata reading (feature: exif) | BSD-2-Clause |
| zenpixels 0.2.0 | PixelBuffer, PixelDescriptor | Apache-2.0 OR MIT |
| enough | Cooperative cancellation | Local |
| whereat 0.1.4 | Location-aware errors | MIT OR Apache-2.0 |
| thiserror 2.0.18 | Error derives | MIT OR Apache-2.0 |

## Non-Bayer Path

For sensors with cpp > 1 (Foveon, some embedded RGB DNGs):
- Skip demosaicing
- Extract first 3 channels as RGB
- Apply same color pipeline
