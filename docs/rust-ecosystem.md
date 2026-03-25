# Rust Raw Processing Ecosystem

What exists, what's missing, and where zenraw fits.

## Existing Crates

### rawloader (zenraw's current dependency)
- **License:** LGPL-2.1
- **Status:** Maintained, v0.37, focused and stable
- **Provides:** Raw data extraction, camera ID, crop, black/white levels, WB multipliers, color matrix, CFA pattern
- **Does NOT provide:** Demosaicing, color pipeline, DNG writing, advanced DNG metadata
- **Good for:** Building your own pipeline on extracted raw data
- **repo:** https://github.com/pedrocr/rawloader

### rawler / dnglab
- **License:** LGPL-2.1
- **Status:** Active development, alpha API, 500+ cameras
- **Provides:** Full DNG reading AND writing, CR3/NEF/ARW/RAF/ORF/RW2 decode, LJPEG/JXL compression, camera database with TOML config, X-Trans support
- **Caveats:** Pinned at 0.7.2 in zenraw (API stabilizing but not semver-guaranteed), LGPL viral for static linking
- **repo:** https://github.com/dnglab/dnglab

### bayer / libbayer
- **License:** MIT/Apache-2.0
- **Provides:** Basic demosaic (bilinear, nearest-neighbor) for 8/16-bit Bayer
- **Caveats:** No advanced algorithms, small utility crate
- **repo:** https://github.com/wangds/libbayer

### quickraw
- **License:** MIT
- **Status:** Last update ~2023, possibly unmaintained
- **Provides:** Pure Rust decode and render, integer math for speed
- **repo:** https://github.com/RawLabo/quickraw

### rsraw
- **License:** Wrapper around LibRaw (FFI)
- **Provides:** Safe Rust bindings to LibRaw's full pipeline
- **Caveats:** C++ dependency, not pure Rust
- **crate:** https://crates.io/crates/rsraw

### dng (write-only)
- **Provides:** DNG file writing utilities
- **crate:** https://lib.rs/crates/dng

## The Gap

No single pure-Rust crate provides:
- Modern demosaicing (RCD, AMaZE)
- Full DNG color model (ForwardMatrix, dual-illuminant interpolation)
- Noise profiling from DNG metadata
- Lens correction (DNG opcodes or lensfun)
- SIMD-accelerated pipeline

rawler/dnglab comes closest but is focused on format conversion, not being a processing library with clean API.

## Where zenraw Fits

zenraw fills the gap: a focused raw processing codec with:
- Clean public API (decode/probe/is_raw_file)
- Scene-referred linear output by default
- Integration with zenpixels/zenfilters ecosystem
- No unsafe code
- Cooperative cancellation

The split is: rawloader handles container parsing (LGPL), zenraw handles everything from normalized sensor data onward (processing pipeline owned by us).

## LGPL Considerations

rawloader is LGPL-2.1. For static linking in a non-LGPL product:
- Dynamic linking is fine (no copyleft trigger)
- Static linking requires either LGPL compliance or replacing rawloader

Replacement path: zentiff + custom DNG tag parser for DNG files. Keep rawloader for vendor formats (CR2, NEF, ARW) where the format complexity makes reimplementation impractical.

rawler (dnglab) is also LGPL-2.1. Same considerations apply.
