# ABLATION-zenraw.md — conservative public-API ablation report

**Date:** 2026-06-11
**Snapshot commit:** a95a5011 (main@origin before ablation change)
**Snapshot file:** `docs/public-api/zenraw.txt` (500 items default, 764 all-features)
**Grep template (run from `/home/lilith/work`, exclude target/.jj/zenraw):**
```
grep -r --include="*.rs" "<symbol>" /home/lilith/work/ \
  --exclude-dir="target" --exclude-dir=".jj" --exclude-dir="zenraw"
```

---

## Summary

| Snapshot items | Flagged A | Flagged B | % flagged |
|----------------|-----------|-----------|-----------|
| 500 (default) | 0 | 2 groups | ~1.5% |
| +264 (all-features diff) | 0 | 1 group | ~0.5% |

**Conservative stance:** 493 of 500 default items are KEEP. The two default-features B groups are a raw color pipeline module and the low-level demosaic function — internal processing steps with zero external consumers. The one all-features B group is the Apple MakerNote tag constants module — IFD tag numbers used only within the `apple::extract_*` parsing functions.

---

## Known consumers (evidence gathered this scan)

| Consumer | Items used |
|----------|-----------|
| `zencodecs/src/dyn_dispatch.rs` | `RawDecoderConfig`, `RawDecodeConfig` |
| `zencodecs/src/codecs/raw.rs` | `DNG_FORMAT`, `RAW_FORMAT`, `is_raw_file` |
| `zencodecs/src/exif.rs` | `exif::ExifMetadata` (all fields) |
| `zencodecs/src/decode.rs` | `exif::ExifMetadata` via `read_raw_metadata` |
| `zencodecs/src/lib.rs` | `pub use zenraw::{RawDecodeConfig, RawDecoderConfig}` |
| `zencodecs/tests/raw_capability.rs` | `DNG_FORMAT`, `exif::ExifMetadata` |
| `zenfilters/examples/mobile_parity.rs` | `decode`, `is_raw_file`, `exif::read_metadata`, `exif::extract_dng_preview`, `exif::is_ampf`, `exif::ExifMetadata`, `RawDecodeConfig`, `apple::extract_dng_profile`, `apple::extract_profile_gain_table_map`, `dng_render::DngPipeline::from_metadata` |
| `zenfilters/Cargo.toml` | Commented-out dep (rawler/darktable broken); `delete` feature references it |

---

## Flagged items

### B — `pub(crate)` candidates (zero external consumers, raw processing internals)

**Group 1: `zenraw::color` module — 3 functions**

```
pub fn zenraw::color::apply_color_pipeline(&mut [f32], [f32; 4], [[f32; 3]; 4], OutputPrimaries)
pub fn zenraw::color::apply_srgb_gamma(&mut [f32])
pub fn zenraw::color::f32_to_u8_srgb(&[f32]) -> Vec<u8>
```

These are the raw pixel processing helpers called from the internal decode pipeline (`decode.rs`). They operate on flat `f32` slices, not the zenpixels `PixelBuffer` type used in the public API. No external caller uses them directly — callers use `zenraw::decode()` which applies these internally and returns a `PixelBuffer`. Zero external grep hits (scan 2026-06-11).

**B proposal:** Make `pub mod color` into `pub(crate)`. Eliminates 3 items. The high-level `decode`, `probe`, `RawDecodeConfig`, `DngPipeline` APIs remain public.

**Conservative note:** `f32_to_u8_srgb` could theoretically be useful to callers doing custom f32→u8 conversion, but it is a trivial gamma-aware clamp-and-cast; callers should use `zenpixels`/`linear-srgb` for that purpose. No current consumer warrants keeping it public.

---

**Group 2: `zenraw::demosaic::demosaic_to_rgb_f32` — 1 function**

```
pub fn zenraw::demosaic::demosaic_to_rgb_f32(
    &[f32], usize, usize, &rawloader::decoders::cfa::CFA, DemosaicMethod
) -> Vec<f32>
```

This function takes a `rawloader::decoders::cfa::CFA` reference — a type from the rawloader crate. This leaks a rawloader dependency into callers who use this function. The `DemosaicMethod` enum (Bilinear, MalvarHeCutler) stays public because it is part of `RawDecodeConfig`. But the `demosaic_to_rgb_f32` function itself is an internal step in the decode pipeline that should not be callable externally — it requires the caller to construct CFA objects from the rawloader crate, which is not intended as a public zenraw dependency surface.

Zero external grep hits (scan 2026-06-11).

**B proposal:** Make `demosaic_to_rgb_f32` into `pub(crate)`. Keeps `DemosaicMethod` enum (used by `RawDecodeConfig`) fully public. Eliminates 1 function from the demosaic module public surface.

---

**Group 3 (all-features only): `zenraw::apple::makernote_tags` module — 20 constants**

```
pub const ACCELERATION_VECTOR: u16
pub const AE_AVERAGE: u16
pub const AE_STABLE: u16
pub const AE_TARGET: u16
pub const AF_CONFIDENCE: u16
pub const AF_MEASURED_DEPTH: u16
pub const AF_PERFORMANCE: u16
pub const AF_STABLE: u16
pub const BURST_UUID: u16
pub const HDR_IMAGE_TYPE: u16
pub const IMAGE_CAPTURE_TYPE: u16
pub const IMAGE_UNIQUE_ID: u16
pub const LENS_ID: u16
pub const LIVE_PHOTO_ID: u16
pub const MAKERNOTE_VERSION: u16
pub const MEDIA_GROUP_UUID: u16
pub const OIS_MODE: u16
pub const PHOTO_TRANSCODING_GAIN: u16
pub const SEMANTIC_COMPONENTS: u16
pub const SIGNAL_TO_NOISE_RATIO: u16
```

These are Apple MakerNote IFD tag numbers used internally by `extract_dng_profile`, `extract_gain_map`, and `extract_profile_gain_table_map` to look up specific tags. They are implementation constants, not stable ABI — Apple can change these values without notice and no external caller should be dispatching on them directly. The public surface that external callers need is `extract_dng_profile(&[u8]) -> Option<DngProfile>` (confirmed consumed by zenfilters), not the IFD tag numbers.

Zero external grep hits on any of these 20 constants (scan 2026-06-11).

**B proposal:** Make `pub mod makernote_tags` into `pub(crate)`. Eliminates 20 items. `DngProfile`, `GainMapInfo`, `ProfileGainTableMap`, `extract_dng_profile`, `extract_gain_map`, `extract_profile_gain_table_map` all remain public.

**Note:** `zenraw::apple::makernote_tags::HDR_IMAGE_TYPE` has the same name as `ultrahdr_core::metadata::apple::tags::HDR_IMAGE_TYPE` — these are different crates, different tags (zenraw uses Apple DNG makernote TIFF tags, ultrahdr-core uses Apple HEIC EXIF tags). Neither is used externally; both are flagged B in their respective ablation reports.

---

## Items reviewed and explicitly kept

**Core decode API** (all): `zenraw::decode`, `zenraw::probe`, `zenraw::classify`, `zenraw::is_raw_file`, `Result`, `RawError`, `RawDecodeConfig` (with all pub fields), `RawDecodeOutput`, `RawInfo` (with all pub fields), `OutputMode`, `SensorLayout`. All confirmed consumed by zencodecs and zenfilters. KEEP.

**Root-level type re-exports** (`FileFormat`, `DemosaicMethod`, `OutputMode`, `OutputPrimaries`, `RawDecodeConfig`, `RawDecodeOutput`, `RawInfo`, `SensorLayout`): Re-exported from submodules for caller convenience. Consumed by zencodecs via `pub use zenraw::{RawDecodeConfig, RawDecoderConfig}`. KEEP.

**`DemosaicMethod` enum** (`Bilinear`, `MalvarHeCutler`): Field type of `RawDecodeConfig::demosaic`. KEEP.

**`classify::FileFormat`** (with all methods `has_gain_map`, `is_apple`, `is_raw`): Consumed by the zenraw pipeline. KEEP.

**`dng_render` module** (`DngPipeline`, `OutputPrimaries`): `DngPipeline::from_metadata` confirmed consumed by zenfilters/examples/mobile_parity.rs. `OutputPrimaries` is a field type of `RawDecodeConfig`. KEEP.

**`DngPipeline` pub fields** (`analog_balance`, `baseline_exposure`, `camera_to_output`, `wb_mult`, `output_primaries`, `tone_curve`, `height`, `width`, and in all-features: `gain_table_map`): All fields are `pub` for callers constructing custom DNG pipelines. `baseline_exposure` is the key field accessed by zenfilters (via `ExifMetadata.baseline_exposure` — a separate struct, but the pipeline needs it). KEEP.

**`exif` module** (`ExifMetadata` with all 27 pub fields, `extract_dng_preview`, `is_ampf`, `read_metadata`): `ExifMetadata` confirmed consumed by zencodecs/src/exif.rs (all fields read) and zenfilters (`baseline_exposure`, `analog_balance` via DngPipeline). `extract_dng_preview` and `is_ampf` confirmed consumed by zenfilters. KEEP.

**`xmp` module** (`XmpMetadata` with all pub fields, `extract_xmp`, `read_xmp_metadata`): Behind `xmp` feature flag. Consumed by zencodecs (feature `raw-decode-xmp`). KEEP.

**`darktable` module** (`DtConfig`, `DtColorProfile`, `decode_bytes`, `decode_file`, `is_available`, `version`): Behind `darktable` feature. Consumed by zenfilters/examples. KEEP.

**`apple` module** (`DngProfile`, `GainMapInfo`, `ProfileGainTableMap` with all pub fields, `extract_dng_profile`, `extract_gain_map`, `extract_profile_gain_table_map`): Behind `apple` feature. `extract_dng_profile` and `extract_profile_gain_table_map` confirmed consumed by zenfilters. KEEP.

**`RawDecoderConfig`** (all-features, zencodec adapter): Consumed by zencodecs. KEEP.

**`DNG_FORMAT`, `RAW_FORMAT`** statics (all-features): Consumed by zencodecs. KEEP.

---

## Queued breaking changes (for next minor bump)

```
### QUEUED BREAKING CHANGES
- `zenraw::color` module: make `pub(crate)` — 3 items (`apply_color_pipeline`, `apply_srgb_gamma`, `f32_to_u8_srgb`)
- `zenraw::demosaic::demosaic_to_rgb_f32`: make `pub(crate)` — 1 item (keeps `DemosaicMethod` enum public)
- `zenraw::apple::makernote_tags` module (all-features): make `pub(crate)` — 20 constants
```
