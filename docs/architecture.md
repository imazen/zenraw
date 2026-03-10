# zenraw Architecture

## Module Structure

```
src/
├── lib.rs              # Public API: decode(), probe(), is_raw_file()
├── decode.rs           # Main pipeline: parse → normalize → demosaic → color → crop
├── demosaic.rs         # Bayer CFA → RGB (bilinear, MHC)
├── color.rs            # WB + color matrix + gamma
├── error.rs            # RawError enum
└── zencodec_impl.rs    # Optional zencodec trait integration
```

## Data Flow

```
Raw file bytes (&[u8])
       │
       ├─── probe() ──→ RawInfo (metadata only, no pixel decode)
       │
       └─── decode() ──→ RawDecodeOutput { pixels: PixelBuffer, info: RawInfo }
                │
                ├── [1] rawloader::decode() → RawImage
                │       (parses container, decompresses, extracts metadata)
                │
                ├── [2] normalize_raw_data() → Vec<f32> [0,1]
                │       (black level subtract, white level scale)
                │
                ├── [3] demosaic_to_rgb_f32() → Vec<f32> [R,G,B,R,G,B,...]
                │       (CFA pattern → interleaved RGB)
                │
                ├── [4] apply_color_pipeline() → in-place
                │       (WB × cam_to_srgb matrix, clamp [0,1])
                │
                ├── [5] apply_crop() → cropped Vec<f32>
                │       (camera-recommended crop from metadata)
                │
                └── [6] Output conversion
                        ├── apply_gamma=true  → RGB8 sRGB (PixelBuffer)
                        └── apply_gamma=false → RGBF32 linear (PixelBuffer)
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
pub struct RawDecodeConfig {
    pub demosaic: DemosaicMethod,    // Bilinear or MalvarHeCutler
    pub max_pixels: u64,             // DoS protection (default 200M)
    pub apply_gamma: bool,           // false = linear f32, true = sRGB u8
    pub apply_crop: bool,            // apply camera crop metadata
}
```

### Output

```rust
pub struct RawDecodeOutput {
    pub pixels: PixelBuffer,         // RGB8_SRGB or RGBF32_LINEAR
    pub info: RawInfo,
}

pub struct RawInfo {
    pub width: u32, pub height: u32,
    pub make: String, pub model: String,
    pub sensor_width: u32, pub sensor_height: u32,
    pub cfa_pattern: String,
    pub is_dng: bool,
    pub orientation: u16,
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
5. **Scene-referred default** — linear f32 output by default, gamma opt-in

## Dependencies

| Crate | Purpose | License |
|-------|---------|---------|
| rawloader 0.37 | Raw format parsing (~30 formats) | LGPL-2.1 |
| zenpixels | PixelBuffer, PixelDescriptor | Local |
| enough | Cooperative cancellation | Local |
| whereat | Location-aware errors | Local |
| thiserror | Error derives | MIT |

## Non-Bayer Path

For sensors with cpp > 1 (Foveon, some embedded RGB DNGs):
- Skip demosaicing
- Extract first 3 channels as RGB
- Apply same color pipeline
