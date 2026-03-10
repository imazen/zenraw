# zenraw Roadmap

Current state: v0.1.0 — functional decoder with bilinear/MHC demosaic, basic color pipeline via rawloader.

## Phase 1: Correct Colors (High Impact)

### Replace MHC with RCD demosaic
MHC is patented until 2027-01-25. RCD is MIT-licensed, same speed tier, better quality, and darktable's default.
- Port from https://github.com/LuisSR/RCD-Demosaicing or darktable's `rcd.c`
- Keep bilinear for thumbnails
- Keep MHC as option (mark patent status in docs)

### Pre-demosaic white balance
Currently WB is folded into the post-demosaic color matrix. Applying WB before demosaic improves edge color accuracy — the demosaic algorithm sees balanced channels and makes better interpolation decisions.

### ForwardMatrix path
When DNG ForwardMatrix is available (common), use it instead of inverting ColorMatrix. More numerically stable and what the DNG spec recommends.

### Dual-illuminant interpolation
Most DNG profiles have two illuminant calibrations. Interpolate in mired space based on scene CCT from AsShotNeutral. Without this, tungsten-lit scenes render with wrong colors.

## Phase 2: Smartphone DNGs (High Impact)

### DNG OpcodeList2: GainMap
Smartphones embed spatially-varying gain maps for lens shading correction. Without GainMap processing, smartphone DNG renders have dark corners and uneven color.

This is the single most impactful missing feature for real-world DNG processing.

### DNG OpcodeList2: WarpRectilinear
Lens distortion correction. Less critical than GainMap but needed for wide-angle smartphone lenses.

## Phase 3: Performance (SIMD)

### SIMD demosaicing
The demosaic inner loops are embarrassingly parallel per-row. Process 8 pixels at once with f32x8 using archmage dispatch.

Target: 100+ MP/s (5× current scalar speed).

### SIMD color matrix
The 3×3 matrix multiply in the color pipeline processes one pixel at a time. Batch 8 pixels with SIMD for the matrix-vector multiply.

### Pre-demosaic WB with SIMD
WB is a per-pixel multiply on Bayer data — ideal for SIMD. Process 8 sensor values at once.

## Phase 4: Quality

### Expose NoiseProfile in RawInfo
DNG NoiseProfile tag provides per-channel noise model. Expose this for:
- zenfilters' AdaptiveSharpen noise_floor
- Auto-tune noise-aware parameter selection
- Future denoiser integration

### Expose more EXIF metadata
Currently only orientation is extracted. Add: ISO, exposure time, aperture, focal length, flash, GPS. These inform auto-tuning (ISO → noise level, focal length → lens correction selection).

### BaselineExposure compensation
DNG BaselineExposure tag specifies an EV offset. Apply it to normalize brightness across different camera models.

### Highlight recovery
Clipped highlights (sensor values at WhiteLevel) can be partially recovered by leveraging unclipped channels. The green channel clips last due to higher sensitivity — use it to reconstruct missing R/B in clipped regions.

## Phase 5: Advanced Features

### Vignetting correction
- DNG FixVignetteRadial (opcode 3) when present
- Lensfun database fallback

### Distortion correction
- DNG WarpRectilinear (opcode 1)
- Lensfun PTLens model
- Requires resampling infrastructure (bilinear/bicubic)

### Streaming decode
Currently loads entire raw file into memory. For very large files (100+ MP medium format), streaming decode with row-by-row processing would reduce peak memory.

### Linear DNG output
Write processed images back to DNG format (linear DNG). Useful for archival workflows where the raw data is processed but stored losslessly.

## Rust Ecosystem

### rawloader (current dependency)
- LGPL-2.1, maintained, v0.37
- Handles raw parsing for ~30 camera formats
- Provides black/white levels, WB, color matrix, CFA, crop
- Does NOT provide: demosaic, color pipeline, DNG metadata beyond basics

### rawler / dnglab
- LGPL-2.1, active development, 500+ cameras
- Full DNG reading AND writing
- Could replace rawloader for broader format support
- API not yet stable

### Potential future: own TIFF/DNG parser
zentiff already handles TIFF. Extending it for DNG-specific tags would eliminate the rawloader dependency for DNG files while keeping rawloader for vendor RAW formats (CR2, NEF, ARW, etc.).

## Test Data

`/mnt/v/input/fivek/dng/` — 5,000 DNG files from MIT-Adobe FiveK dataset covering Canon, Nikon, Leica, Olympus, Fuji, Panasonic, and more. Expert-retouched versions in `/mnt/v/input/fivek/expert_c/` for quality comparison.
