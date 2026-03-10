# Lens Corrections

Three types of optical corrections, in priority order.

## 1. Vignetting (Biggest Visual Impact, Simplest)

Light falloff from center to edges. Per-pixel multiply — no resampling needed.

### DNG GainMap (opcode 9)
Smartphones embed spatially-varying gain maps in OpcodeList2. These are camera-manufacturer calibrated and should be preferred when present.

Parameters: bounds, grid dimensions, gain values per channel. Bilinear interpolation between grid points.

### Lensfun PA Model
```
C_corrected = C_source / (1 + k1×r² + k2×r⁴ + k3×r⁶)
```
Where r = normalized distance from image center. Depends on focal length, aperture, focus distance.

Example (12mm f/4.5):
```
k1 = -0.19267, k2 = 0.09379, k3 = -0.38938
```

### Implementation
```rust
fn correct_vignetting(pixel: f32, r_squared: f32, k: [f32; 3]) -> f32 {
    let r2 = r_squared;
    let r4 = r2 * r2;
    let r6 = r4 * r2;
    let gain = 1.0 / (1.0 + k[0] * r2 + k[1] * r4 + k[2] * r6);
    pixel * gain
}
```

## 2. Distortion (Required for Geometry)

Barrel/pincushion distortion. Requires resampling (bilinear or bicubic interpolation of source pixels).

### DNG WarpRectilinear (opcode 1)
Brown-Conrady model with 6 coefficients per plane: `[kr0, kr1, kr2, kr3, kt0, kt1]` plus optical center `(cx, cy)`.

### Lensfun PTLens Model (most common)
```
r_distorted = r × (a×r³ + b×r² + c×r + (1 - a - b - c))
```

Correction: for each output pixel, find its distorted source position and sample. This is a backward mapping — iterate from output to input.

Example (12mm ultra-wide):
```
a = 0, b = -0.01919, c = 0  (barrel distortion)
```

## 3. Chromatic Aberration (Subtle but Visible at Edges)

Color fringing from wavelength-dependent refraction. Requires per-channel resampling.

### Pre-demosaic (preferred)
Shift red and blue CFA rows by fractional amounts before demosaic. Avoids demosaic artifacts at high-contrast color boundaries.

### Post-demosaic
Independently warp red and blue channels:
```
r_R = r × (br×r² + cr×r + vr)
r_B = r × (bb×r² + cb×r + vb)
```
Green stays at reference position.

## DNG vs. Lensfun

| Source | When to Use | Accuracy |
|--------|------------|----------|
| DNG OpcodeList2 | Always when present | Best — manufacturer calibrated |
| Lensfun database | No DNG opcodes | Good — community calibrated |
| None | Unknown lens | Skip corrections |

DNG opcodes are embedded per-image. Lensfun requires EXIF matching (make, model, focal length, aperture).

## Implementation Priority

1. **DNG GainMap** — biggest impact, smartphone DNGs look wrong without it
2. **Vignetting** (lensfun fallback) — simple multiply, high visual impact
3. **Distortion** — needs resampling infrastructure, important for architecture/landscapes
4. **TCA** — subtle, low priority

## Lensfun Database

XML files organized by manufacturer. Ships with darktable and RawTherapee.

Matching from EXIF: `make + model + focal_length + aperture + focus_distance`.

The `lensfun` crate (if one exists) or manual XML parsing could provide lens profiles. Alternatively, DNG opcodes cover the most important cases (smartphones are the majority of photos).
