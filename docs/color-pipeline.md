# Color Pipeline: Sensor to sRGB

The complete pipeline from raw sensor ADU values to display-ready sRGB.

> **Note:** This document predates the DngPipeline implementation (dng_render.rs). The current pipeline supports PGTM, WB-baked ColorMatrix, and Apple ToneCurve paths.

## Current Implementation

zenraw currently uses rawloader's color data and a simplified pipeline:

```
Raw ADU → black/white normalize → demosaic → WB × inv(xyz_to_cam) × xyz_to_srgb → clamp → gamma
```

This works but misses several DNG features that improve color accuracy.

## Full DNG Pipeline

```
[1] Black level subtraction (per-CFA-position, with row/column deltas)
[2] Linearization (LinearizationTable LUT, rare)
[3] OpcodeList2 (GainMap for lens shading, pre-demosaic corrections)
[4] White balance (diagonal scale on Bayer data)
[5] Demosaic (Bayer → 3-channel RGB)
[6] OpcodeList3 (post-demosaic corrections, rare)
[7] Color matrix (camera RGB → XYZ D50)
[8] Chromatic adaptation (D50 → D65 if targeting sRGB)
[9] XYZ → linear sRGB
[10] Tone curve / gamma encoding
```

### Step 1: Black Level Subtraction

```rust
// BlackLevel can be per-CFA-position (2x2 for Bayer)
for each pixel (row, col):
    cfa_row = row % black_level_repeat_dim[0]
    cfa_col = col % black_level_repeat_dim[1]
    bl = black_level[cfa_row * repeat_cols + cfa_col]
    bl += black_level_delta_v[row] + black_level_delta_h[col]  // if present
    pixel = max(0, raw_pixel - bl)
```

zenraw already does this via rawloader's black/white levels.

### Step 2: Normalize to [0, 1]

```rust
for each pixel (row, col):
    channel = cfa_color_at(row, col)
    wl = white_level[channel] - black_level_for(row, col)
    pixel_normalized = clamp(pixel / wl, 0.0, 1.0)
```

### Step 3: White Balance

AsShotNeutral gives the camera's measured neutral. Multipliers are the inverse, normalized so the smallest is 1.0:

```rust
// AsShotNeutral = [0.473, 1.0, 0.627]
// WB multipliers = [1/0.473, 1/1.0, 1/0.627] = [2.114, 1.0, 1.595]
// Normalized: [2.114, 1.0, 1.595]
let wb_r = 1.0 / as_shot_neutral[0];
let wb_g = 1.0 / as_shot_neutral[1];
let wb_b = 1.0 / as_shot_neutral[2];
let min_wb = wb_r.min(wb_g).min(wb_b);
let wb = [wb_r / min_wb, wb_g / min_wb, wb_b / min_wb];

// Apply BEFORE demosaic (to raw Bayer data):
for each pixel (row, col):
    channel = cfa_color_at(row, col)
    pixel *= wb[channel]
```

**Current zenraw status:** WB is applied after demosaic in the combined matrix. Moving it pre-demosaic would improve edge color accuracy.

### Step 4: Color Matrix

Two paths depending on whether ForwardMatrix exists in the DNG:

**Path A — ForwardMatrix (preferred):**
```rust
// ForwardMatrix maps white-balanced camera RGB → XYZ D50 directly
let xyz_d50 = forward_matrix * wb_camera_rgb;
```

**Path B — ColorMatrix (fallback, current zenraw path):**
```rust
// ColorMatrix maps XYZ → camera RGB. Invert it.
let cam_to_xyz = invert(color_matrix);
// Then adapt from scene illuminant to D50 using Bradford
let xyz_d50 = bradford_adapt(scene_xy, D50_xy) * cam_to_xyz * camera_rgb;
```

rawloader provides `xyz_to_cam` (ColorMatrix). zenraw inverts it. This works but doesn't handle dual-illuminant interpolation.

### Step 5: XYZ → linear sRGB

**XYZ D65 → linear sRGB** (IEC 61966-2-1):
```
[  3.2404542  -1.5371385  -0.4985314 ]
[ -0.9692660   1.8760108   0.0415560 ]
[  0.0556434  -0.2040259   1.0572252 ]
```

If the source is XYZ D50 (from ForwardMatrix), apply Bradford D50→D65 first:
```
[  0.9555766  -0.0230393   0.0631636 ]
[ -0.0282895   1.0099416   0.0210077 ]
[  0.0122982  -0.0204830   1.3299098 ]
```

### Step 6: sRGB Gamma

NOT a simple gamma 2.2. It's piecewise (IEC 61966-2-1):
```rust
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}
```

## Dual-Illuminant Interpolation

Most DNG profiles have two illuminants (typically StdA ~2856K and D65 ~6504K). The matrices must be interpolated in mired space:

```rust
let mired = 1_000_000.0 / scene_cct;
let mired_1 = 1_000_000.0 / cct_illuminant1;
let mired_2 = 1_000_000.0 / cct_illuminant2;
let t = ((mired - mired_1) / (mired_2 - mired_1)).clamp(0.0, 1.0);
let color_matrix = lerp(color_matrix_1, color_matrix_2, t);
let forward_matrix = lerp(forward_matrix_1, forward_matrix_2, t);
```

## Key Constants

### Illuminant CCT Values (EXIF LightSource enum)

| Value | Illuminant | CCT (K) |
|-------|-----------|---------|
| 17 | Standard light A | 2856 |
| 20 | D55 | 5503 |
| 21 | D65 | 6504 |
| 23 | D50 | 5003 |

### Illuminant Chromaticity (CIE 1931 2-degree)

| Illuminant | x | y |
|-----------|-------|-------|
| A (tungsten) | 0.44758 | 0.40745 |
| D50 | 0.34567 | 0.35851 |
| D65 | 0.31272 | 0.32903 |

### Bradford Cone Response Matrix

```
[  0.8951   0.2664  -0.1614 ]
[ -0.7502   1.7135   0.0367 ]
[  0.0389  -0.0685   1.0296 ]
```

General chromatic adaptation from source white (xs, ys) to dest white (xd, yd):
```
S_src = M_Bradford × XYZ_from_xy(xs, ys)
S_dst = M_Bradford × XYZ_from_xy(xd, yd)
M_adapt = inv(M_Bradford) × diag(S_dst / S_src) × M_Bradford
```

## Integration with zenfilters

zenfilters works in Oklab space. The handoff:

```
zenraw decode (linear sRGB f32)
    → zenfilters scatter_to_oklab (using BT.709 GamutMatrix)
    → filter pipeline (Oklab L, a, b planes)
    → zenfilters gather_from_oklab
    → encode (sRGB u8 or other output)
```

For wide-gamut output (Display P3, BT.2020), pass the appropriate ColorPrimaries to `rgb_to_lms_matrix()` in zenpixels-convert. The GamutMatrix adapts the Oklab conversion to the working primaries.
