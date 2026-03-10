# DNG Format Reference

DNG (Digital Negative) is Adobe's open RAW format, built on TIFF/EP. This documents the tags and structures most relevant to zenraw.

## File Structure

- **IFD0**: Reduced-resolution preview (NewSubFileType = 1)
- **SubIFD(s)**: Full-resolution raw data (NewSubFileType = 0), via SubIFDs tag (330)
- Optional additional SubIFDs for alternate previews

Data stored as **strips** (StripOffsets + StripByteCounts + RowsPerStrip) or **tiles** (TileOffsets + TileByteCounts + TileWidth + TileLength).

Compression: 1 = uncompressed, 7 = JPEG (lossless JPEG for raw data), 52546 = JPEG-XL.

## Critical Tags

### Identity & Version

| Tag | Name | Purpose |
|-----|------|---------|
| 50706 | DNGVersion | 4 bytes: `[1, 6, 0, 0]` |
| 50707 | DNGBackwardVersion | Minimum reader version |
| 50708 | UniqueCameraModel | Camera identifier |

### Geometry & Cropping

| Tag | Name | Purpose |
|-----|------|---------|
| 50829 | ActiveArea | `[top, left, bottom, right]` valid pixel region |
| 50830 | MaskedAreas | Optically black regions (black level estimation) |
| 50719 | DefaultCropOrigin | `[x, y]` crop within ActiveArea |
| 50720 | DefaultCropSize | `[width, height]` default crop |
| 50718 | DefaultScale | `[h, v]` for non-square pixels |

### Sensor Calibration

| Tag | Name | Purpose |
|-----|------|---------|
| 50713 | BlackLevelRepeatDim | Pattern size, usually `[2, 2]` for Bayer |
| 50714 | BlackLevel | Per-CFA-position black level |
| 50715 | BlackLevelDeltaH | Per-column black level delta |
| 50716 | BlackLevelDeltaV | Per-row black level delta |
| 50717 | WhiteLevel | Saturation level per channel |
| 50712 | LinearizationTable | LUT for non-linear sensors (uncommon) |

### Color Filter Array

| Tag | Name | Purpose |
|-----|------|---------|
| 33421 | CFARepeatPatternDim | `[rows, cols]`, usually `[2, 2]` |
| 33422 | CFAPattern | Color codes: 0=R, 1=G, 2=B |
| 50710 | CFAPlaneColor | Maps CFA codes to output planes |
| 50711 | CFALayout | 1=rectangular (standard) |

### Color Matrices

| Tag | Name | Purpose |
|-----|------|---------|
| 50778 | CalibrationIlluminant1 | EXIF LightSource enum for matrix set 1 |
| 50779 | CalibrationIlluminant2 | Same for matrix set 2 |
| 50721 | ColorMatrix1 | 3×3 SRATIONAL: XYZ→CameraRGB under illuminant 1 |
| 50722 | ColorMatrix2 | Same under illuminant 2 |
| 50964 | ForwardMatrix1 | 3×3: WB'd CameraRGB→XYZ D50 under illuminant 1 |
| 50965 | ForwardMatrix2 | Same under illuminant 2 |
| 50723 | CameraCalibration1 | Per-camera fine-tuning (usually identity) |
| 50724 | CameraCalibration2 | Same for illuminant 2 |
| 50727 | AnalogBalance | Per-channel gain diagonal (usually identity) |

### White Balance

| Tag | Name | Purpose |
|-----|------|---------|
| 50728 | AsShotNeutral | Camera-native neutral, e.g., `[0.473, 1.0, 0.627]` |
| 50729 | AsShotWhiteXY | xy chromaticity of scene illuminant |

### Image Quality

| Tag | Name | Purpose |
|-----|------|---------|
| 50730 | BaselineExposure | EV compensation to apply |
| 50731 | BaselineNoise | Relative noise (1.0 = baseline) |
| 50732 | BaselineSharpness | Relative sharpness (1.0 = baseline) |
| 50738 | AntiAliasStrength | 0.0 = no AA filter, 1.0 = standard |
| 51041 | NoiseProfile | Per-channel `(S, O)` noise model pairs |

### Opcodes (Lens/Sensor Corrections)

| Tag | Name | When Applied |
|-----|------|-------------|
| 51008 | OpcodeList1 | Before linearization (rare) |
| 51009 | OpcodeList2 | After linearization, before demosaic |
| 51022 | OpcodeList3 | After demosaic |

Opcode IDs:
```
 1 = WarpRectilinear       (Brown-Conrady lens distortion)
 2 = WarpFisheye
 3 = FixVignetteRadial
 4 = FixBadPixelsConstant
 5 = FixBadPixelsList
 6 = TrimBounds
 7 = MapTable              (16-bit LUT)
 8 = MapPolynomial
 9 = GainMap               (spatially-varying gain — heavy smartphone use)
10 = DeltaPerRow
11 = DeltaPerColumn
12 = ScalePerRow
13 = ScalePerColumn
```

**GainMap** (opcode 9) is critical for smartphone DNGs — it corrects lens shading/vignetting with a spatially-varying gain grid. Without it, smartphone DNG renders will have dark corners and uneven color.

## NoiseProfile

Tag 51041 stores per-channel linear noise model: `variance(x) = S*x + O`
- S = shot noise coefficient (signal-dependent, Poisson)
- O = read noise variance (signal-independent, Gaussian)
- Stored as `N×2` doubles: `[S_0, O_0, S_1, O_1, ..., S_n, O_n]`

```rust
fn noise_std_dev(signal: f64, shot: f64, read: f64) -> f64 {
    (shot * signal + read).max(0.0).sqrt()
}
```

## EXIF LightSource Values

```
 0 = Unknown           17 = Standard light A (~2856K)
 1 = Daylight          18 = Standard light B (~4874K)
 9 = Fine weather      19 = Standard light C (~6774K)
10 = Cloudy            20 = D55 (~5503K)
11 = Shade             21 = D65 (~6504K)
                       22 = D75 (~7504K)
                       23 = D50 (~5003K)
                       24 = ISO studio tungsten (~3200K)
```

Typical pairing: Illuminant1 = 17 (StdA), Illuminant2 = 21 (D65).

## Minimum Viable DNG Reader

To decode a DNG and produce a viewable image:

1. TIFF parser (IFD chains, byte order, tag types)
2. Navigate to raw SubIFD (NewSubFileType = 0)
3. Decompress raw data (uncompressed or lossless JPEG)
4. Extract: BlackLevel, WhiteLevel, CFA, AsShotNeutral, ColorMatrix1
5. Process: normalize → WB → demosaic → color matrix → sRGB gamma

### Can Skip Initially
- OpcodeList1/3 (rare)
- ProfileHueSatMap, ProfileLookTable, ProfileToneCurve
- CameraCalibration (usually identity)
- AnalogBalance (usually identity)
- JPEG-XL compressed raw data
- DNG 1.6 semantic masks

### Should Implement Soon
- OpcodeList2 GainMap (smartphone DNGs need this)
- Dual-illuminant interpolation (most profiles have two matrices)
- ForwardMatrix path (more accurate than inverted ColorMatrix)

## Test Data

5,000 DNG files from the MIT-Adobe FiveK dataset are available at:
`/mnt/v/input/fivek/dng/`

These cover a wide range of cameras (Canon, Nikon, Leica, Olympus, Fuji, etc.) and are ideal for testing format support and color accuracy.
