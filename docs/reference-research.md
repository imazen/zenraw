# Raw Image Processing: Technical Research for Rust Codec Implementation

## 1. DNG Format Specifics

### File Structure

DNG is built on TIFF/EP. A DNG file contains:

- **IFD0**: Typically a reduced-resolution preview/thumbnail (NewSubFileType = 1)
- **SubIFD(s)**: The full-resolution raw data (NewSubFileType = 0), accessed via the SubIFDs tag (330)
- Optional additional SubIFDs for alternate previews (NewSubFileType = 0x10001)

Data can be stored as **strips** (StripOffsets + StripByteCounts + RowsPerStrip) or **tiles** (TileOffsets + TileByteCounts + TileWidth + TileLength). Compression: 1 = uncompressed, 7 = JPEG (baseline DCT for previews, lossless JPEG/LJPEG for raw data), 52546 = JPEG-XL.

### Critical DNG Tags (Tag Number : Name : Purpose)

**Identity & Version:**
| Tag | Name | Purpose |
|-----|------|---------|
| 50706 | DNGVersion | 4 bytes: e.g., `[1, 6, 0, 0]` |
| 50707 | DNGBackwardVersion | Minimum reader version needed |
| 50708 | UniqueCameraModel | Camera identifier string |

**Geometry & Cropping:**
| Tag | Name | Purpose |
|-----|------|---------|
| 50829 | ActiveArea | `[top, left, bottom, right]` of valid pixel region within full sensor |
| 50830 | MaskedAreas | Optically black pixel regions (for black level estimation) |
| 50719 | DefaultCropOrigin | `[x, y]` of crop within ActiveArea |
| 50720 | DefaultCropSize | `[width, height]` of default crop |
| 50718 | DefaultScale | `[h_scale, v_scale]` for non-square pixels |

**Sensor Calibration:**
| Tag | Name | Purpose |
|-----|------|---------|
| 50713 | BlackLevelRepeatDim | Repeat pattern size for BlackLevel (usually `[2, 2]` for Bayer) |
| 50714 | BlackLevel | Per-channel or per-CFA-position black level values |
| 50715 | BlackLevelDeltaH | Per-column black level delta |
| 50716 | BlackLevelDeltaV | Per-row black level delta |
| 50717 | WhiteLevel | Saturation level per channel (e.g., 4095 for 12-bit, 16383 for 14-bit) |
| 50712 | LinearizationTable | LUT mapping stored values to linear values (uncommon) |

**Color Filter Array:**
| TIFF Tag | Name | Purpose |
|----------|------|---------|
| 33421 | CFARepeatPatternDim | `[rows, cols]` of CFA pattern (usually `[2, 2]`) |
| 33422 | CFAPattern | CFA color codes: 0=Red, 1=Green, 2=Blue |
| 50710 | CFAPlaneColor | Maps CFA color codes to output plane indices |
| 50711 | CFALayout | 1=rectangular, 2=staggered A, etc. |

**Color Matrices & Calibration:**
| Tag | Name | Purpose |
|-----|------|---------|
| 50778 | CalibrationIlluminant1 | EXIF LightSource enum value for matrix set 1 |
| 50779 | CalibrationIlluminant2 | EXIF LightSource enum value for matrix set 2 |
| 50721 | ColorMatrix1 | 3x3 SRATIONAL: XYZ-to-CameraRGB under illuminant 1 |
| 50722 | ColorMatrix2 | 3x3 SRATIONAL: XYZ-to-CameraRGB under illuminant 2 |
| 50964 | ForwardMatrix1 | 3x3 SRATIONAL: WB'd CameraRGB-to-XYZ D50 under illuminant 1 |
| 50965 | ForwardMatrix2 | 3x3 SRATIONAL: WB'd CameraRGB-to-XYZ D50 under illuminant 2 |
| 50723 | CameraCalibration1 | Per-camera fine-tuning matrix under illuminant 1 |
| 50724 | CameraCalibration2 | Per-camera fine-tuning matrix under illuminant 2 |
| 50725 | ReductionMatrix1 | Dimensionality reduction for >3 color sensors |
| 50726 | ReductionMatrix2 | Same, for illuminant 2 |
| 50727 | AnalogBalance | Per-channel gain (diagonal matrix) |

**White Balance:**
| Tag | Name | Purpose |
|-----|------|---------|
| 50728 | AsShotNeutral | Camera-native neutral color (e.g., `[0.473, 1.0, 0.627]`) |
| 50729 | AsShotWhiteXY | xy chromaticity of scene illuminant |

**Image Quality:**
| Tag | Name | Purpose |
|-----|------|---------|
| 50730 | BaselineExposure | EV compensation to apply |
| 50731 | BaselineNoise | Relative noise level (1.0 = baseline) |
| 50732 | BaselineSharpness | Relative sharpness (1.0 = baseline) |
| 50733 | BayerGreenSplit | Non-zero if the two greens differ (sensor defect indicator) |
| 50738 | AntiAliasStrength | 0.0 = no AA filter, 1.0 = standard |
| 51041 | NoiseProfile | Per-channel `(S, O)` noise model pairs |

**Opcodes (Lens/Sensor Corrections):**
| Tag | Name | Purpose |
|-----|------|---------|
| 51008 | OpcodeList1 | Applied to stored raw values before mapping to linear |
| 51009 | OpcodeList2 | Applied after linearization, before demosaic |
| 51022 | OpcodeList3 | Applied to final demosaiced image |

**DNG 1.6+ Tags (Illuminant 3):**
| Tag | Name | Purpose |
|-----|------|---------|
| 52529 | CalibrationIlluminant3 | Third illuminant for triple-illuminant profiles |
| 52530 | CameraCalibration3 | Calibration matrix for illuminant 3 |
| 52531 | ColorMatrix3 | Color matrix for illuminant 3 |
| 52532 | ForwardMatrix3 | Forward matrix for illuminant 3 |
| 52533-52535 | IlluminantData1/2/3 | Custom illuminant spectral data |

### EXIF LightSource Enum (CalibrationIlluminant values)

```
 0 = Unknown
 1 = Daylight
 2 = Fluorescent
 3 = Tungsten (incandescent)
 4 = Flash
 9 = Fine weather
10 = Cloudy
11 = Shade
12 = Daylight fluorescent (D 5700-7100K)
13 = Day white fluorescent (N 4600-5500K)
14 = Cool white fluorescent (W 3800-4500K)
15 = White fluorescent (WW 3250-3800K)
16 = Warm white fluorescent (L 2600-3250K)
17 = Standard light A       (~2856K)
18 = Standard light B       (~4874K)
19 = Standard light C       (~6774K)
20 = D55                    (~5503K)
21 = D65                    (~6504K)
22 = D75                    (~7504K)
23 = D50                    (~5003K)
24 = ISO studio tungsten    (~3200K)
255 = Other (DNG 1.6+: requires IlluminantData tag)
```

**Typical pairing:** CalibrationIlluminant1 = 17 (StdA, ~2856K), CalibrationIlluminant2 = 21 (D65, ~6504K).

### DNG Color Model: How Matrices Interact

The DNG color model uses up to 6 matrices that compose together. Here is the exact pipeline:

**Step 1: Determine scene white point CCT.**
If `AsShotNeutral` is provided (more common), iteratively solve for the xy chromaticity:
```
initial_xy = (1/3, 1/3)
loop until converged (delta < 1e-4):
    M = AB * CC(xy) * CM(xy)     // compose matrices at current xy
    XYZ = M^(-1) * neutral       // invert to get XYZ
    xy = XYZ_to_xy(XYZ)          // project to chromaticity
```
If `AsShotWhiteXY` is provided directly, use it.

**Step 2: Interpolate matrices in mired space.**
Convert the CCT to mireds: `mired = 1,000,000 / CCT_kelvin`. Then linearly interpolate:
```
mired_1 = 1e6 / CCT_illuminant1
mired_2 = 1e6 / CCT_illuminant2
t = (mired_target - mired_1) / (mired_2 - mired_1)
t = clamp(t, 0.0, 1.0)
CM = lerp(CM1, CM2, t)
CC = lerp(CC1, CC2, t)
FM = lerp(FM1, FM2, t)
```

**Step 3: Compose the transform.** Two paths depending on whether ForwardMatrix exists:

*Path A: ForwardMatrix present (preferred):*
```
// Camera RGB → XYZ D50
AB = diag(AnalogBalance)          // usually identity
CC = interpolated CameraCalibration
reference_neutral = inv(AB * CC) * camera_neutral
D  = inv(diag(reference_neutral)) // diagonal white balance
FM = interpolated ForwardMatrix

M_camera_to_XYZ = FM * D * inv(AB * CC)
```

*Path B: ForwardMatrix absent:*
```
CM = interpolated ColorMatrix     // maps XYZ → camera native
M_XYZ_to_camera = AB * CC * CM
M_camera_to_XYZ = inv(M_XYZ_to_camera)
// Apply Bradford adaptation from scene white to D50
```

### DNG Opcode IDs

```
 1 = WarpRectilinear       (lens distortion, Brown-Conrady model)
 2 = WarpFisheye
 3 = FixVignetteRadial
 4 = FixBadPixelsConstant
 5 = FixBadPixelsList
 6 = TrimBounds
 7 = MapTable              (16-bit LUT)
 8 = MapPolynomial
 9 = GainMap               (per-channel spatially-varying gain)
10 = DeltaPerRow
11 = DeltaPerColumn
12 = ScalePerRow
13 = ScalePerColumn
14 = (reserved/newer)
```

**WarpRectilinear parameters** (per plane): `[kr0, kr1, kr2, kr3, kt0, kt1]` plus `(cx, cy)` optical center (normalized 0-1).

**GainMap parameters**: top/left/bottom/right bounds, mapPointsV/H, mapSpacingV/H, mapOriginV/H, mapPlanes, + float array of gain values. Used heavily by smartphones for lens shading correction.

### NoiseProfile Tag

Stores per-channel noise model: `variance(x) = S*x + O` where x is the normalized signal [0, 1].
- S = shot noise (signal-dependent, Poisson) coefficient
- O = read noise (signal-independent, Gaussian) variance

Tag stores `N*2` doubles for N color channels: `[S_0, O_0, S_1, O_1, ..., S_n, O_n]`.

---

## 2. Demosaicing Algorithms: Ranked by Quality/Speed

### Summary Table

| Algorithm | Quality | Speed | Memory | Patent-Free | Best For |
|-----------|---------|-------|--------|-------------|----------|
| Bilinear | Poor | Fastest | Minimal | Yes | Thumbnails only |
| Malvar-He-Cutler | Good | Very fast | Minimal | **NO** (US 7502505, expires 2027-01-25) | N/A until expired |
| VNG4 | Good | Moderate | Low | Yes | Smooth areas, crosstalk |
| AHD | Good | Moderate | Moderate | Yes | General purpose |
| DCB | Good+ | Moderate | Moderate | Yes | No-AA-filter cameras |
| RCD | Very good | Fast | Moderate | **Yes** (MIT) | Default/general, astro |
| AMaZE | Excellent | Slow | High | Yes (GPL3) | Low-ISO, max quality |
| LMMSE | Very good | Slow | High | Yes | High-ISO, noisy images |
| IGV | Very good | Moderate | Moderate | Yes | Moire suppression |

### Defaults in Major Software

- **darktable**: RCD (since 3.6, previously PPG)
- **RawTherapee**: AMaZE
- **LibRaw/dcraw**: AHD (user_qual = 3)

### Algorithm Details

#### Bilinear
Average the 2 or 4 nearest same-color neighbors. Produces severe zipper artifacts at edges. Only useful for fast previews.

```
G at R location: G = (G_top + G_bottom + G_left + G_right) / 4
R at G in R row: R = (R_left + R_right) / 2
R at B location: R = (R_tl + R_tr + R_bl + R_br) / 4
```

#### Malvar-He-Cutler (MHC)
Linear 5x5 filters with gradient correction. Adds Laplacian of the known channel to bilinear interpolation of the missing channel. All filters use divisor of **16** and integer arithmetic.

**G at R (or B) locations, scaled by 1/16:**
```
 0   0  -1   0   0
 0   0   2   0   0
-1   2   4   2  -1
 0   0   2   0   0
 0   0  -1   0   0
```

**R at G in R row (B at G in B row), scaled by 1/16:**
```
 0   0   0.5  0   0
 0  -1   0   -1   0
-1   4   5    4  -1
 0  -1   0   -1   0
 0   0   0.5  0   0
```

**R at G in B row (B at G in R row), scaled by 1/16:**
```
 0   0  -1   0   0
 0  -1   4  -1   0
 0.5 0   5   0   0.5
 0  -1   4  -1   0
 0   0  -1   0   0
```

**R at B (B at R), scaled by 1/16:**
```
 0   0  -1.5  0   0
 0   2   0    2   0
-1.5 0   6    0  -1.5
 0   2   0    2   0
 0   0  -1.5  0   0
```

**STATUS: PATENTED** until January 25, 2027 (US 7502505). Do not ship in a product until then.

#### AHD (Adaptive Homogeneity-Directed)
1. Interpolate green channel in horizontal direction (using directional linear filter)
2. Interpolate green channel in vertical direction
3. Complete R and B for both directions using color-difference interpolation
4. Convert both results to CIELab
5. For each pixel, compute homogeneity metric in a 3x3 neighborhood (count of pixels where L and C differences are below threshold)
6. Choose direction with higher homogeneity count
7. Apply median filter on color differences to reduce artifacts

Moderate speed, good quality. The CIELab conversion step is the primary performance bottleneck.

#### DCB (Double Corrected Bilinear)
Similar approach to AHD but uses a different correction strategy. Good for cameras without optical low-pass filters (anti-alias filters). Can show background artifacts similar to AHD.

#### RCD (Ratio Corrected Demosaicing)
1. Compute directional gradients on raw Bayer data (H, V, diagonal P, diagonal Q)
2. Estimate green using ratio-corrected low-pass filter:
   ```
   G_est = G_neighbor * (1 + (LPF_center - LPF_neighbor) / (LPF_center + LPF_neighbor))
   ```
3. Adaptively select interpolation direction based on gradient strength
4. Reconstruct R and B from local color differences using complete green channel

**License: MIT.** This is the recommended default algorithm for a new implementation. Fast, high quality, patent-free, darktable's default.

#### AMaZE (Aliasing Minimization and Zipper Elimination)
Complex algorithm by Emil Martinec. Excellent at low ISO but slow and memory-hungry. Uses adaptive color-difference interpolation with sophisticated edge detection. Licensed under GPL3 in RawTherapee's implementation.

#### LMMSE (Linear Minimum Mean Square Error)
Statistical approach minimizing mean squared error. Excellent noise handling. Very computationally expensive and memory-heavy. Best for high-ISO images where noise suppression during demosaic matters.

#### Dual Demosaic Strategy
Both darktable and RawTherapee support hybrid approaches (e.g., "RCD+VNG4"):
- Use RCD/AMaZE for high-contrast regions (better detail)
- Use VNG4 for flat/smooth regions (fewer artifacts)
- Blend based on local contrast metric

### Recommendation for a Codec

1. **Bilinear** for thumbnail generation
2. **RCD** as default (MIT, fast, high quality)
3. **Optional LMMSE** for high-ISO denoising path
4. Consider dual RCD+VNG4 for maximum quality

---

## 3. Color Pipeline: Sensor to sRGB

### Complete Pipeline (in order)

```
Raw sensor ADU values
    |
    v
[1] Black Level Subtraction
    |
    v
[2] Linearization (if LinearizationTable exists)
    |
    v
[3] OpcodeList2 processing (GainMap, lens corrections on linear data)
    |
    v
[4] White Balance (diagonal scale)
    |
    v
[5] Demosaic (Bayer → 3-channel RGB)
    |
    v
[6] OpcodeList3 processing (post-demosaic corrections)
    |
    v
[7] Color Matrix (Camera RGB → XYZ D50)
    |
    v
[8] Optional: Chromatic Adaptation (if not targeting D50)
    |
    v
[9] XYZ → linear sRGB (or other output space)
    |
    v
[10] Tone curve / Gamma encoding
    |
    v
Output sRGB image
```

### Step 1: Black Level Subtraction

```rust
// BlackLevel can be per-CFA-position (2x2 for Bayer)
// BlackLevelRepeatDim tells you the pattern size
for each pixel (row, col):
    cfa_row = row % black_level_repeat_dim[0]
    cfa_col = col % black_level_repeat_dim[1]
    bl = black_level[cfa_row * repeat_cols + cfa_col]
    // Add per-row and per-column deltas if present
    bl += black_level_delta_v[row] + black_level_delta_h[col]
    pixel = max(0, raw_pixel - bl)
```

### Step 2: Normalize to [0, 1]

```rust
// After black subtraction, scale to [0, 1] using WhiteLevel
for each pixel (row, col):
    channel = cfa_color_at(row, col)
    wl = white_level[channel] - black_level_for_position(row, col)
    pixel_normalized = clamp(pixel / wl, 0.0, 1.0)
```

### Step 3: White Balance

Apply per-channel multipliers. `AsShotNeutral` gives the neutral point in camera space. White balance multipliers are the inverse, normalized so the smallest is 1.0:

```rust
// AsShotNeutral = [r_neutral, g_neutral, b_neutral]
// e.g., [0.473, 1.0, 0.627]
// WB multipliers = 1/neutral, then normalized
let wb_r = 1.0 / as_shot_neutral[0]; // e.g., 2.114
let wb_g = 1.0 / as_shot_neutral[1]; // e.g., 1.000
let wb_b = 1.0 / as_shot_neutral[2]; // e.g., 1.595
// Normalize so min = 1.0
let min_wb = wb_r.min(wb_g).min(wb_b);
let wb = [wb_r / min_wb, wb_g / min_wb, wb_b / min_wb];

// Apply before demosaic (to raw Bayer data):
for each pixel (row, col):
    channel = cfa_color_at(row, col)
    pixel *= wb[channel]
```

### Step 4: Color Matrix (Camera RGB to XYZ D50)

**With ForwardMatrix (preferred path):**
```rust
// ForwardMatrix maps white-balanced camera RGB → XYZ D50
// Already incorporates adaptation to D50

// Compose: M = FM * D * inv(AB * CC)
// Where D = diag(1/reference_neutral) normalizes white point

// In practice, if AB=I and CC=I (common):
// M = FM * diag(wb_multipliers)
// But the wb is already applied to pixels, so:
// xyz = FM * camera_rgb_white_balanced

let xyz = forward_matrix * camera_rgb;
```

**Without ForwardMatrix (fallback):**
```rust
// ColorMatrix maps XYZ → camera RGB
// Invert to get camera RGB → XYZ
// Then apply Bradford adaptation from scene white to D50

let m_xyz_to_cam = color_matrix; // 3x3, stored row-major
let m_cam_to_xyz = m_xyz_to_cam.inverse();

// The color matrix embeds white balance, so apply to
// NON-white-balanced camera RGB:
let xyz = m_cam_to_xyz * camera_rgb_no_wb;

// Then adapt from scene illuminant to D50 using Bradford:
let xyz_d50 = bradford_adapt(scene_white_xy, D50_xy) * xyz;
```

### Step 5: XYZ D50 to Output Color Space

**XYZ D65 to linear sRGB matrix** (IEC 61966-2-1):
```
M_XYZ_to_sRGB = [
    [ 3.2404541621141054, -1.5371385940306089, -0.49853140955601579],
    [-0.96926603050518312,  1.8760108454466942,  0.041556017530349834],
    [ 0.055643430959114726,-0.20397695987305730, 1.0572251882231791]
]
```

**Linear sRGB to XYZ D65 matrix:**
```
M_sRGB_to_XYZ = [
    [0.41245643908969243, 0.35757607764390886, 0.18043748326639894],
    [0.21267285140562253, 0.71515215528781773, 0.072174993306559740],
    [0.019333895582329317, 0.11919202588130297, 0.95030407853636773]
]
```

If your source is XYZ D50 (from ForwardMatrix), you need Bradford adaptation D50 to D65 first:

**Bradford D50 → D65:**
```
M_D50_to_D65 = [
    [ 0.9555766, -0.0230393,  0.0631636],
    [-0.0282895,  1.0099416,  0.0210077],
    [ 0.0122982, -0.0204830,  1.3299098]
]
```

**Bradford D65 → D50:**
```
M_D65_to_D50 = [
    [ 1.0478112,  0.0228866, -0.0501270],
    [ 0.0295424,  0.9904844, -0.0170491],
    [-0.0092345,  0.0150436,  0.7521316]
]
```

**Complete chain (ForwardMatrix path):**
```rust
let xyz_d50 = forward_matrix * wb_camera_rgb;
let xyz_d65 = M_D50_TO_D65 * xyz_d50;
let linear_srgb = M_XYZ_TO_SRGB * xyz_d65;
```

### Step 6: sRGB Gamma Encoding

The sRGB transfer function is NOT a simple gamma 2.2. It is a piecewise function (IEC 61966-2-1):

```rust
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
```

### Key Illuminant Chromaticity Coordinates (CIE 1931 2-degree)

| Illuminant | CCT (K) | x | y |
|------------|---------|-------|-------|
| A (tungsten) | 2856 | 0.44758 | 0.40745 |
| D50 | 5003 | 0.34567 | 0.35851 |
| D55 | 5503 | 0.33243 | 0.34744 |
| D65 | 6504 | 0.31272 | 0.32903 |
| D75 | 7504 | 0.29903 | 0.31488 |

### Bradford Cone Response Matrix

```
M_Bradford = [
    [ 0.8951,  0.2664, -0.1614],
    [-0.7502,  1.7135,  0.0367],
    [ 0.0389, -0.0685,  1.0296]
]
```

General chromatic adaptation from source white (xs, ys) to destination white (xd, yd):
```
S_src = M_Bradford * XYZ_from_xy(xs, ys)
S_dst = M_Bradford * XYZ_from_xy(xd, yd)
M_adapt = inv(M_Bradford) * diag(S_dst / S_src) * M_Bradford
```

### Chromatic Aberration Correction

Lateral (transverse) CA manifests as radial color fringing. Correction approach:

1. **Pre-demosaic** (on Bayer data): Shift red and blue CFA rows/columns by fractional amounts using the DNG WarpRectilinear opcode or lensfun TCA parameters
2. **Post-demosaic** (on RGB data): Independently warp red and blue channels using radial polynomial:
   ```
   r_corrected_R = r * (kr * r^2 + cr * r + vr)   // lensfun poly3 TCA model
   r_corrected_B = r * (kb * r^2 + cb * r + vb)
   ```
   Green channel stays at reference position.

Pre-demosaic correction on Bayer data is preferable because it avoids demosaic artifacts at high-contrast color boundaries.

---

## 4. Noise Characteristics of Raw Data

### Noise Model

Total noise variance at a pixel with signal level x:

```
Var(x) = a * x + b
```

Where:
- **a** = shot noise coefficient (Poisson component, signal-dependent)
- **b** = read noise variance (Gaussian component, signal-independent)
- **x** = signal level (in ADU or normalized to [0, 1])

Standard deviation: `sigma(x) = sqrt(max(0, a * x + b))`

This is exactly what the DNG NoiseProfile tag stores per channel.

### Shot Noise (Photon Noise)

Follows Poisson statistics. For a pixel collecting N photons:
- Mean = N
- Variance = N
- StdDev = sqrt(N)

In ADU (after analog gain g = electrons/ADU):
- Variance_ADU = g^2 * N_electrons = g * signal_ADU
- So the 'a' coefficient ≈ analog gain

Shot noise dominates at medium to high signal levels. SNR improves with the square root of signal: `SNR_shot = sqrt(N)`.

### Read Noise

Fixed noise floor from sensor electronics. Independent of signal level.
- Typical values: 1-10 electrons RMS for modern sensors
- Scales with ISO: higher ISO = more analog gain = read noise amplified in ADU
- The 'b' coefficient in the noise model

### How Noise Varies with ISO

```
At higher ISO:
  - Analog gain (g) increases
  - Shot noise coefficient 'a' increases proportionally to g
  - Read noise 'b' increases proportionally to g^2
  - But in electron-referred terms, read noise stays ~constant
  - Dynamic range decreases because white_level / noise_floor shrinks
```

### Three Noise Regimes

1. **Shadows (low signal):** Read noise dominates. SNR limited by electronics.
2. **Midtones (medium signal):** Shot noise dominates. SNR proportional to sqrt(signal).
3. **Highlights (high signal):** PRNU (pixel response non-uniformity) dominates. SNR saturates.

### Estimating Noise from Raw Data

**Method 1: From dark frames**
Capture with lens cap on. The variance of pixel values = read noise variance.

**Method 2: From uniform patches**
In a uniformly-lit region:
- Compute local mean and local variance
- Plot variance vs. mean
- Fit linear model: `var = a * mean + b`
- Slope = shot noise coefficient, intercept = read noise variance

**Method 3: Wavelet-based (darktable's approach)**
- Apply wavelet decomposition to the image
- Estimate noise from the MAD (median absolute deviation) of finest-scale wavelet coefficients
- `sigma = MAD / 0.6745` (robust estimator)
- Fit the noise curve sigma(signal) = sqrt(a*x + b) across brightness levels

### darktable Noise Profile Format

Per-ISO preset: `{a_red, a_green, a_blue, b_red, b_green, b_blue}`

Example (Canon 5D Mark II, ISO 3200):
```
a = [4.494e-05, 4.494e-05, 4.494e-05]
b = [-1.063e-06, -1.063e-06, -1.063e-06]
```

The negative b value indicates measurement noise in the calibration; clamp to 0 in practice.

---

## 5. Existing Rust Crates for Raw Processing

### rawler (dnglab)

- **Repo:** https://github.com/dnglab/dnglab
- **License:** LGPL-2.1
- **Status:** Active development (alpha), 500+ camera models
- **Provides:**
  - Full DNG reading and writing
  - Decoding of CR3 (Canon), NEF (Nikon), ARW (Sony), RAF (Fuji), ORF (Olympus), RW2 (Panasonic), and many more
  - LJPEG and JPEG-XL compression for DNG output
  - Camera database with color matrices, black/white levels, crop areas
  - TOML-based camera config with `color_matrix`, `xyz_to_cam`, `whitelevel`, `blacklevel`, `active_area`
  - Lens metadata resolution
  - Bit-level decompression (Huffman, CRX, etc.)
  - X-Trans CFA support (Fuji)
- **API Warning:** Not yet stable, not following semver
- **Key types:** `RawImage`, `Camera`, `RawSource`, `BitPump` trait, `RawlerError`

### rawloader

- **Repo:** https://github.com/pedrocr/rawloader
- **License:** LGPL-2.1
- **Status:** Maintained, v0.37.x, focused and stable
- **Provides:**
  - Raw data extraction (pixel values exactly as encoded)
  - Camera identification (EXIF + cleaned-up name)
  - Crop margins (top, right, bottom, left)
  - Black/white levels per channel
  - White balance multipliers
  - Color matrix (camera → XYZ)
  - CFA/Bayer pattern description
- **Does NOT provide:** Demosaicing, color pipeline, DNG writing
- **Good for:** Building your own pipeline on top of extracted raw data

### bayer (libbayer)

- **Repo:** https://github.com/wangds/libbayer
- **License:** MIT/Apache-2.0
- **Status:** Small utility crate
- **Provides:** Demosaicing algorithms for 8-bit and 16-bit Bayer images
- **Algorithms:** Bilinear, nearest-neighbor
- **Good for:** Simple demosaic needs, no advanced algorithms

### quickraw

- **Repo:** https://github.com/RawLabo/quickraw
- **License:** MIT
- **Status:** Last update ~2023, possibly unmaintained
- **Provides:** Pure Rust decode and render from raw files
- **Note:** Uses integer math for speed over precision

### rsraw

- **Crate:** https://crates.io/crates/rsraw
- **Status:** Wrapper around LibRaw (FFI), recent activity 2025
- **Provides:** Safe Rust bindings to LibRaw's full processing pipeline

### dng (crate)

- **Crate:** https://lib.rs/crates/dng
- **Provides:** DNG file writing utilities

### Summary: What's Missing in the Ecosystem

No single Rust crate provides a complete, high-quality, pure-Rust raw processing pipeline with:
- Modern demosaicing (RCD, AMaZE)
- Full DNG color model implementation
- Noise profiling and denoising
- Lens correction

rawler/dnglab comes closest but is focused on format conversion rather than being a processing library. rawloader provides clean extraction but no processing. There's a clear gap for a focused raw processing codec library.

---

## 6. Lens Correction

### Lensfun Database

**Format:** XML files organized by manufacturer. Database ships with darktable, RawTherapee, and other tools.

**Matching:** Camera and lens are matched from EXIF metadata (make, model, focal length, aperture, focus distance).

### Distortion Models

All models use normalized radius `r` (distance from image center, normalized so corner = 1 or half-diagonal = 1, depending on implementation).

**Poly3:**
```
r_d = r_u * (1 - k1 + k1 * r_u^2)
```

**Poly5:**
```
r_d = r_u * (1 + k1 * r_u^2 + k2 * r_u^4)
```

**PTLens (most common in practice):**
```
r_d = r_u * (a * r_u^3 + b * r_u^2 + c * r_u + (1 - a - b - c))
```
Where `r_d` = distorted radius, `r_u` = undistorted radius.

Typical coefficient values (12mm ultra-wide):
```
focal=12: a=0, b=-0.01919, c=0   (barrel distortion)
focal=24: a=0, b=0.00061, c=0    (nearly rectilinear)
```

**Correction direction:** These map undistorted→distorted. To correct, you iterate backwards: for each output pixel, find its distorted source position and sample.

### Vignetting Model (PA)

```
C_corrected = C_source / (1 + k1*r^2 + k2*r^4 + k3*r^6)
```

Depends on focal length, aperture, and focus distance. Lensfun interpolates between stored calibration points. Typical values:
```
focal=12, aperture=4.5, distance=100m: k1=-0.19267, k2=0.09379, k3=-0.38938
```

### Transverse Chromatic Aberration (TCA)

**Linear model:**
```
r_d_R = r_u * kr
r_d_B = r_u * kb
```
Green is the reference channel (unchanged).

**Poly3 model:**
```
r_d_R = r_u * (br * r_u^2 + cr * r_u + vr)
r_d_B = r_u * (bb * r_u^2 + cb * r_u + vb)
```

### DNG Embedded Corrections vs. Lensfun

DNG files can embed lens corrections via OpcodeList2:
- **WarpRectilinear** (opcode 1): Brown-Conrady model with 6 coefficients per plane
- **FixVignetteRadial** (opcode 3): Radial vignetting correction
- **GainMap** (opcode 9): Arbitrary spatially-varying gain (smartphones use this heavily)

When present, DNG opcodes should be preferred over lensfun because they are camera-manufacturer calibrated for the specific lens/body combination.

### Implementation Priority

1. **Vignetting correction** - biggest visual impact, simplest to implement (per-pixel multiply)
2. **Distortion correction** - required for architectural/geometric accuracy (requires resampling)
3. **TCA correction** - subtle but visible at edges (requires per-channel resampling)

---

## 7. Adobe DNG SDK

### What It Provides

The DNG SDK is Adobe's reference implementation in C++. Key components:

- **DNG reading:** Complete TIFF/IFD parser with all DNG tags
- **DNG writing:** Generating valid DNG files
- **Color processing:** Full implementation of the DNG color model (matrix interpolation, forward/color matrix paths, chromatic adaptation)
- **Opcode processing:** WarpRectilinear, FixVignetteRadial, GainMap, etc.
- **Lossless JPEG codec:** LJPEG compression/decompression
- **Linearization:** LinearizationTable and black level handling
- **Validation:** DNG file validation tools

### Key Source Files (from the SDK)

- `dng_color_spec.cpp` - The complete DNG color pipeline implementation
- `dng_opcode.cpp` / `dng_opcode_list.cpp` - Opcode processing
- `dng_negative.cpp` - The main raw image container
- `dng_image_writer.cpp` - DNG file writing
- `dng_lossless_jpeg.cpp` - LJPEG codec
- `dng_camera_profile.cpp` - Camera profile (matrices, tone curves, HSL maps)

### Minimum Viable DNG Reader

To read a DNG and produce a viewable image, you need at minimum:

1. **TIFF parser:** Read IFD chains, handle byte order (little/big endian), parse tag types (BYTE, SHORT, LONG, RATIONAL, SRATIONAL, etc.)

2. **Navigate to raw SubIFD:** Follow SubIFDs tag, find IFD with NewSubFileType=0

3. **Read raw data:** Handle strip/tile layout, decompress (uncompressed or LJPEG)

4. **Extract metadata:**
   - BlackLevel, WhiteLevel (or defaults: 0 and 2^bps - 1)
   - CFARepeatPatternDim + CFAPattern (or default RGGB)
   - AsShotNeutral (or AsShotWhiteXY)
   - ColorMatrix1 (required for non-monochrome)
   - ForwardMatrix1 (optional but preferred)
   - ActiveArea, DefaultCropOrigin, DefaultCropSize

5. **Process:**
   - Subtract black level, normalize by white level
   - Apply white balance from AsShotNeutral
   - Demosaic (even bilinear works for MVP)
   - Apply ForwardMatrix (or inverted ColorMatrix + Bradford adaptation)
   - Convert XYZ D50 → XYZ D65 → linear sRGB
   - Apply sRGB gamma

6. **Optional but important:**
   - OpcodeList2 processing (GainMap for smartphones)
   - CalibrationIlluminant interpolation for dual-illuminant profiles
   - Tone curve (BaselineToneCurve or default DNG tone curve)
   - Highlight recovery / clipping

### What You Can Skip Initially

- OpcodeList1 (pre-linearization corrections, rare)
- OpcodeList3 (post-demosaic corrections, rare)
- ProfileHueSatMap (HSL-based profile adjustments)
- ProfileLookTable (3D LUT adjustments)
- ProfileToneCurve (custom tone curves)
- Semantic masks and depth data (DNG 1.6+)
- CameraCalibration matrices (per-unit calibration, usually identity)
- AnalogBalance (usually identity)
- JPEG-XL compressed raw data (newer feature)

---

## Appendix: Key Data Structures for Rust Implementation

### Core Types

```rust
/// Bayer CFA pattern
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CfaColor {
    Red = 0,
    Green = 1,
    Blue = 2,
}

/// 2x2 Bayer pattern (top-left first, row-major)
/// RGGB = [R, G, G, B]
#[derive(Clone, Copy, Debug)]
pub struct CfaPattern {
    pub width: u32,   // usually 2
    pub height: u32,  // usually 2
    pub colors: [CfaColor; 4], // for 2x2; extend for X-Trans (6x6)
}

impl CfaPattern {
    pub fn color_at(&self, row: u32, col: u32) -> CfaColor {
        let r = (row % self.height) as usize;
        let c = (col % self.width) as usize;
        self.colors[r * self.width as usize + c]
    }
}

/// 3x3 matrix for color transforms
#[derive(Clone, Copy, Debug)]
pub struct Matrix3x3(pub [[f64; 3]; 3]);

impl Matrix3x3 {
    pub fn multiply_vec(&self, v: [f64; 3]) -> [f64; 3] {
        [
            self.0[0][0]*v[0] + self.0[0][1]*v[1] + self.0[0][2]*v[2],
            self.0[1][0]*v[0] + self.0[1][1]*v[1] + self.0[1][2]*v[2],
            self.0[2][0]*v[0] + self.0[2][1]*v[1] + self.0[2][2]*v[2],
        ]
    }

    pub fn multiply(&self, other: &Matrix3x3) -> Matrix3x3 {
        let mut result = [[0.0f64; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                for k in 0..3 {
                    result[i][j] += self.0[i][k] * other.0[k][j];
                }
            }
        }
        Matrix3x3(result)
    }
}

/// DNG image metadata
pub struct DngMetadata {
    // Geometry
    pub width: u32,
    pub height: u32,
    pub active_area: [u32; 4],           // [top, left, bottom, right]
    pub default_crop_origin: [f64; 2],   // [x, y]
    pub default_crop_size: [f64; 2],     // [w, h]

    // Sensor
    pub bits_per_sample: u16,
    pub black_level: [f64; 4],           // per CFA position
    pub white_level: [u32; 4],           // per channel
    pub cfa_pattern: CfaPattern,

    // Color
    pub color_matrix_1: Matrix3x3,       // XYZ → camera (required)
    pub color_matrix_2: Option<Matrix3x3>,
    pub forward_matrix_1: Option<Matrix3x3>, // WB'd camera → XYZ D50
    pub forward_matrix_2: Option<Matrix3x3>,
    pub calibration_illuminant_1: u16,   // EXIF LightSource enum
    pub calibration_illuminant_2: Option<u16>,
    pub as_shot_neutral: [f64; 3],       // camera-native neutral

    // Noise
    pub noise_profile: Option<Vec<(f64, f64)>>, // (S, O) per channel

    // Baseline
    pub baseline_exposure: f64,
    pub baseline_noise: f64,
    pub baseline_sharpness: f64,
}

/// Noise model: variance = S * signal + O
pub struct NoiseModel {
    pub shot_noise: [f64; 3],  // S per channel (signal-dependent)
    pub read_noise: [f64; 3],  // O per channel (signal-independent variance)
}

impl NoiseModel {
    pub fn std_dev(&self, channel: usize, signal: f64) -> f64 {
        (self.shot_noise[channel] * signal + self.read_noise[channel])
            .max(0.0)
            .sqrt()
    }
}
```

### Illuminant Temperature Lookup

```rust
fn illuminant_to_cct(illuminant: u16) -> Option<f64> {
    match illuminant {
        17 => Some(2856.0),  // Standard light A
        18 => Some(4874.0),  // Standard light B
        19 => Some(6774.0),  // Standard light C
        20 => Some(5503.0),  // D55
        21 => Some(6504.0),  // D65
        22 => Some(7504.0),  // D75
        23 => Some(5003.0),  // D50
        24 => Some(3200.0),  // ISO studio tungsten
        _ => None,           // Unknown or Other
    }
}

fn interpolation_weight(cct: f64, cct_1: f64, cct_2: f64) -> f64 {
    let mired = 1e6 / cct;
    let mired_1 = 1e6 / cct_1;
    let mired_2 = 1e6 / cct_2;
    ((mired - mired_1) / (mired_2 - mired_1)).clamp(0.0, 1.0)
}
```

---

## Sources

- [DNG Specification 1.6.0.0 (PDF)](https://paulbourke.net/dataformats/dng/dng_spec_1_6_0_0.pdf)
- [DNG Specification 1.4.0.0 (PDF)](https://www.kronometric.org/phot/processing/DNG/dng_spec_1.4.0.0.pdf)
- [Developing a RAW Photo by Hand Part 2](https://www.odelama.com/photo/Developing-a-RAW-Photo-by-hand/Developing-a-RAW-Photo-by-hand_Part-2/)
- [Colour-HDRI DNG Module (Python reference)](https://colour-hdri.readthedocs.io/en/develop/_modules/colour_hdri/models/dng.html)
- [Adobe DNG SDK Source](https://github.com/aizvorski/dng_sdk)
- [RawPedia Demosaicing](https://rawpedia.rawtherapee.com/Demosaicing)
- [darktable Demosaic Manual](https://docs.darktable.org/usermanual/4.8/en/module-reference/processing-modules/demosaic/)
- [RCD Demosaicing (MIT, GitHub)](https://github.com/LuisSR/RCD-Demosaicing)
- [Malvar-He-Cutler IPOL Article](http://www.ipol.im/pub/art/2011/g_mhcd/revisions/2011-08-14/g_mhcd.htm)
- [US Patent 7502505 (Malvar-He-Cutler)](https://patents.google.com/patent/US7502505)
- [A Simple DSLR Camera Sensor Noise Model](https://www.odelama.com/photo/A-Simple-DSLR-Camera-Sensor-Noise-Model/)
- [darktable Noise Profiling](https://www.darktable.org/2012/12/profiling-sensor-and-photon-noise/)
- [Lensfun Calibration Format](https://lensfun.github.io/manual/v0.3.2/elem_calibration.html)
- [Lensfun Lens API](https://lensfun.github.io/manual/latest/group__Lens.html)
- [Lensfun Distortion Tutorial](https://lensfun.github.io/calibration-tutorial/lens-distortion.html)
- [rawler DNG Tags (docs.rs)](https://docs.rs/rawler/latest/rawler/tags/enum.DngTag.html)
- [dnglab Architecture (DeepWiki)](https://deepwiki.com/dnglab/dnglab)
- [rawloader (GitHub)](https://github.com/pedrocr/rawloader)
- [DNG TIFF Tags Source](https://graphics.stanford.edu/papers/fcam/html/_t_i_f_f_tags_8cpp_source.html)
- [DNG Opcodes Editor](https://github.com/electro-logic/DngOpcodesEditor)
- [Android DngUtils.h](https://android.googlesource.com/platform/frameworks/av/+/master/media/img_utils/include/img_utils/DngUtils.h)
- [sRGB Wikipedia](https://en.wikipedia.org/wiki/SRGB)
- [ICC D65 to D50 Chad Tag](https://www.color.org/chadtag.xalter)
- [LibRaw Demosaic Analysis](https://www.libraw.org/node/2306)
- [Imatest Sensor Noise](https://www.imatest.com/imaging/image-sensor-noise/)
