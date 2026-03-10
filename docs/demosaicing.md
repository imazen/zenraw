# Demosaicing Algorithms

Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG) sample one color per pixel. Demosaicing reconstructs the missing two channels at each site.

## Algorithm Comparison

| Algorithm | Quality | Speed | Memory | Patent-Free | Best For |
|-----------|---------|-------|--------|-------------|----------|
| Bilinear | Poor | Fastest | Minimal | Yes | Thumbnails |
| Malvar-He-Cutler | Good | Very fast | Minimal | **NO** (expires 2027-01-25) | — |
| VNG4 | Good | Moderate | Low | Yes | Smooth areas |
| AHD | Good | Moderate | Moderate | Yes | General purpose |
| DCB | Good+ | Moderate | Moderate | Yes | No-AA-filter cameras |
| **RCD** | **Very good** | **Fast** | Moderate | **Yes (MIT)** | **Default/general** |
| AMaZE | Excellent | Slow | High | Yes (GPL3) | Low-ISO max quality |
| LMMSE | Very good | Slow | High | Yes | High-ISO noise |

### Defaults in Major Software
- **darktable**: RCD (since 3.6)
- **RawTherapee**: AMaZE
- **LibRaw/dcraw**: AHD

## Current zenraw Status

zenraw implements:
- **Bilinear** — fast, severe zipper artifacts at edges
- **Malvar-He-Cutler** — default, good quality, 5x5 gradient-corrected kernels

**Patent warning:** MHC is patented (US 7502505) until January 25, 2027. Consider replacing with RCD as default before any commercial release.

## RCD — Recommended Next Algorithm

RCD (Ratio Corrected Demosaicing) is MIT-licensed, fast, high quality, and darktable's default. It should replace MHC as zenraw's default.

### Algorithm Overview

1. Compute directional gradients on raw Bayer data (H, V, diagonal P, Q)
2. Estimate green using ratio-corrected low-pass filter:
   ```
   G_est = G_neighbor × (1 + (LPF_center - LPF_neighbor) / (LPF_center + LPF_neighbor))
   ```
3. Adaptively select interpolation direction based on gradient strength
4. Reconstruct R and B from local color differences using the complete green channel

### Key Properties
- Ratio-based correction avoids division-by-zero artifacts in dark regions
- Adaptive directional selection preserves edges better than MHC's fixed kernels
- MIT license — safe for commercial use immediately
- Performance comparable to MHC (same O(n) complexity, slightly larger stencil)

### Reference Implementation
https://github.com/LuisSR/RCD-Demosaicing (MIT license)
Also in darktable source: `src/iop/demosaic/rcd.c`

## Dual Demosaic Strategy

Both darktable and RawTherapee support hybrid approaches (e.g., "RCD+VNG4"):
- Use RCD for high-contrast regions (better edge detail)
- Use VNG4 for flat/smooth regions (fewer moire artifacts)
- Blend based on local contrast metric

This is worth implementing as a quality option after RCD is working.

## MHC Kernel Reference

For reference, the Malvar-He-Cutler kernels used in zenraw's current implementation:

**Green at R/B site** (÷8):
```
 0  0 -1  0  0
 0  0  2  0  0
-1  2  4  2 -1
 0  0  2  0  0
 0  0 -1  0  0
```

**R at B / B at R** (÷8):
```
 0    0  -1.5  0    0
 0    2   0    2    0
-1.5  0   6    0  -1.5
 0    2   0    2    0
 0    0  -1.5  0    0
```

**R at G in R-row / B at G in B-row** (÷8):
```
 0    0  0.5  0    0
 0   -1   0  -1    0
-1    4   5   4   -1
 0   -1   0  -1    0
 0    0  0.5  0    0
```

## SIMD Opportunities

Demosaicing is embarrassingly parallel per-row. Key optimization targets:

1. **Row-parallel processing**: Each output row depends only on ±2 input rows
2. **SIMD green interpolation**: The 5×5 kernel is a weighted sum — 8 pixels at once with f32x8
3. **Pre-compute CFA color map**: Avoid per-pixel CFA lookups by pre-computing a row-phase indicator
4. **Tile-based processing**: Process in cache-friendly tiles (e.g., 64×64) to avoid L2 misses on large images

The current scalar implementation processes ~20 MP/s. SIMD should reach ~100+ MP/s.

## X-Trans Support

Fujifilm X-Trans sensors use a 6×6 CFA pattern instead of 2×2 Bayer. rawloader supports X-Trans CFA descriptions. Standard Bayer demosaicing algorithms don't apply — X-Trans needs dedicated algorithms (Markesteijn, Frank Markesteijn's 3-pass or 1-pass).

X-Trans is lower priority since it's Fuji-only, but the architecture should not hard-code 2×2 CFA assumptions.
