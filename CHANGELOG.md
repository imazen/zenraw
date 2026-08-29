# Changelog

## [Unreleased]

### QUEUED BREAKING CHANGES
<!-- Breaking changes that will ship together in the next major (or minor for 0.x) release.
     Add items here as you discover them. Do NOT ship these piecemeal — batch them. -->
- `RawError`'s variant set was reshaped for the zencodec `ErrorCategory` taxonomy (see the
  `### Changed` entry below): `Decode`, `InvalidInput`, `Unsupported(String)`, and the
  single-field `LimitExceeded(String)` no longer exist — matching code must be updated to the
  new variant names. No known in-workspace consumer pattern-matched `RawError` directly.

### Fixed

- **Every non-Bayer CFA was demosaiced with the 2×2 Bayer kernel, corrupting
  colour on the default backend.** `demosaic_malvar` precomputed a 2×2
  `cfa_tile` and the interior loop read it as `cfa_tile[row & 1][col & 1]`,
  while the border path used the sensor's true `cfa.color_at(row, col)`. On a
  Fujifilm X-Trans sensor (a 6×6 CFA) the two halves of one image therefore
  disagreed: interior sites were assigned the colour a Bayer sensor would have
  had at that parity, so the measured sample was written into the wrong output
  channel. The default feature set is `["std", "rawloader", "ultrahdr"]` and
  `decode::decode` routed every `cpp == 1` image into the Bayer kernels with no
  CFA-dimension check, so the correct pattern-agnostic kernel was reachable only
  from the non-default `rawler` backend. `CfaColorAt` now carries
  `tile_dims()`, and both demosaic entry points dispatch on it: `(2, 2)` keeps
  the Bayer kernels (bit-identical output for every Bayer sensor — pinned by
  `bayer_dispatch_still_selects_the_requested_kernel`), any other described tile
  routes to the same same-colour-neighbour kernel the rawler backend has always
  used for X-Trans, and a CFA the backend could not describe at all is now a
  typed `RawError::UnsupportedFeature` instead of pixels demosaiced against a
  fabricated pattern. Mutation-verified: forcing the Bayer branch back on makes
  `xtrans_known_channel_survives_top_level_dispatch` fail at interior pixel
  (2, 3) with "measured channel 1 clobbered … got 0.5125, sensor said 0.5" while
  the border pixels still pass — the interior/border split, reproduced.
- **`probe` was the one RAW entry point without panic isolation.**
  `decode::decode` and both `rawler_backend` entry points wrap their parse call
  in `std::panic::catch_unwind`; `decode::probe` called `rawloader::decode`
  directly, on the metadata-only path callers reach for precisely because it is
  meant to be the cheap, safe one. Now wrapped to match. **Corrected claim:**
  this is defense-in-depth, not a live host crash — rawloader 0.37.2 wraps its
  own `decode_unsafe` in `catch_unwind`, so a panicking decoder already
  surfaced as an `Err`; `Buffer::new` runs outside that guard and the behaviour
  is not part of rawloader's documented contract (the same reasoning
  `tests/rawler_panic.rs` already records for rawler). Verified live on the
  probe path by injecting a `panic!` inside the guard: with it, the new tests
  return `RawError::Malformed`; without it, they unwind through the caller
  while the `decode` case keeps passing.
- **The rawloader backend reported `SensorLayout::Bayer` for every
  single-channel sensor**, so an X-Trans file was described to callers as
  Bayer. Now derived from the CFA tile: 2×2 → `Bayer`, 6×6 → `XTrans`,
  anything else → `Unknown`. Mutation-verified against the new
  `sensor_layout_distinguishes_bayer_from_xtrans`.
- **New `tests/synthetic_dng.rs`** builds valid DNGs in-process (little-endian
  TIFF, one IFD, uncompressed 16-bit strip), so panic isolation, output-mode
  ranges, and sensor-layout reporting are all covered with no external corpus
  and no runtime file-existence skips.
- **Pushes to `main` now cancel their superseded CI runs.** `ci.yml` keyed its
  concurrency group on `${{ github.head_ref || github.run_id }}`.
  `github.head_ref` is populated only for `pull_request` events, so on a push it
  was empty and the group fell through to `github.run_id` — unique per run, so no
  two pushes ever shared a group and `cancel-in-progress` could never fire. Every
  push started a full matrix that ran to completion even when several commits
  landed seconds apart. Now keyed on `${{ github.ref }}`, which is set for both
  event types, so PR cancellation is unchanged and consecutive pushes supersede
  each other.
- **The `Fuzz regression` CI job could not fail.** It ran
  `cargo test --test fuzz_regression 2>/dev/null || echo "No regression test
  found…"` inside an `if [ -d fuzz/regression ]` guard, so a genuinely failing
  suite, a missing corpus, and a missing harness all reported green.
  `tests/fuzz_regression.rs` has existed the whole time, so the fallback was
  masking real failures rather than covering a missing target. The step is now
  a bare `cargo test --test fuzz_regression`, and the harness asserts at least
  `MIN_SEEDS` (1) replayable seeds are present — `zenutils_fuzz::RegressionSuite`
  treats a missing or empty seed dir as a clean no-op, so an emptied or renamed
  `fuzz/regression/` previously passed without replaying anything.
  Mutation-verified: removing the corpus and injecting a panic into the
  `classify` target each fail the test with exit code 101, the latter on the
  `x3f-oob-panic-issue12.bin` seed.

- **Probe/decode dimension parity on the rawler backend** (issue #5). `rawler_backend::probe`
  reported the camera `crop_area` / `active_area` size unconditionally, while the decode path's
  `apply_rawler_crop` fell back to the full sensor when that rectangle was empty or extended
  past the sensor — so a DNG with a malformed crop made `probe` and `decode(crop = true,
  orientation = false)` disagree exactly like the rawloader case fixed in 5eb1dbf. Both now
  share one `rawler_cropped_dims` helper (checked arithmetic; an absurd header offset can no
  longer overflow in `apply_rawler_crop`). Unit tests pin the helper and the rawloader
  `cropped_dims`/`apply_crop` pair (D70-shaped 3040→3008 crop, absent crop, three malformed
  shapes) — the first direct tests of the crop math, so the parity can't silently regress
  without the FiveK corpus present.
- `tests/probe_parity.rs` no longer reads a hard-coded `/mnt/v/input/fivek/dng`: the FiveK DNG
  tests are gated on `ZENRAW_FIVEK_DIR` (set by `just test-raw-parity`), the same caller-visible
  skip chain as `ZENRAW_RAW_SAMPLES_DIR`. Unset → skip with a message; set → a directory
  without a `.dng` is a hard failure, not a silent pass.
- **CI Clippy job green again under stable 1.98**: nine `chunks_exact`-with-constant-size
  sites in `tests/{formats,integration,regression}.rs` moved to `as_chunks::<N>()`, and
  `--all-features` (`_dev`) no longer errors with `private_interfaces` — the two
  `simd::*_fallible` kernels (only ever called from the rawler backend) are `pub(crate)`
  now, so the crate-local `AllocPref` stays private. Also cleared two `_dev`-only lints in
  `benches/kernel_tiers.rs`. No behaviour change.
- **rawler-only builds compile again** (`--no-default-features --features "std,rawler"`,
  with or without `ultrahdr`) — issue #10 item 1 (d2b1d473). The Bayer demosaic kernels
  were `#[cfg(feature = "rawloader")]` and the rawler backend called `rawloader::CFA::new`,
  so selecting `rawler` without `rawloader` failed with E0432/E0433. The kernels are now
  generic over a crate-internal CFA lookup (`BayerCfa`, a 2×2 tile) and the rawler backend
  builds that tile from rawler's own `color_at`. Output bytes are unchanged: a unit test
  asserts bit-exact parity between the `rawloader::CFA` and `BayerCfa` lookups for all four
  Bayer patterns and both demosaic methods. The public
  `demosaic::demosaic_to_rgb_f32(.., &rawloader::CFA, ..)` signature is untouched.
- **Latent panic in the rawler backend removed** (d2b1d473): `rawloader::CFA::new` panics
  on any pattern string whose length is not 0/4/16/36/144, and the rawler backend called it
  outside `catch_unwind` for every ≤4-character CFA. Sampling rawler's `color_at` instead
  cannot panic.
- CI now tests the rawler backend without rawloader (`rawler-only` job) and checks the two
  feature combinations from #10 in the `feature-perms` job; `just check` mirrors them
  (d2b1d473).

### Changed

- **`OutputMode::Linear` was documented "not clamped to `[0, 1]`" but is
  clamped.** `color::apply_color_pipeline` → `apply_color_matrix` clamps every
  component as it writes it, and it runs for `Develop` *and* `Linear`. **Chose
  to correct the documentation, not the behaviour**, because
  `apply_color_pipeline` is public API shared by both modes: making the clamp
  conditional needs either a signature change or a new public function, and
  removing it outright changes rendering for every `Linear` decode — both are
  public-API decisions that need the maintainer's approval, not a bug fix.
  Removing the clamp is also not obviously right on its own: with it deleted,
  the new `linear_output_is_clamped_to_unit_range` reports `Linear sample 0 =
  -2.6078527`, i.e. deeply negative out-of-gamut components, so an unclamped
  `Linear` needs a deliberate gamut policy rather than a deletion. Flagged for a
  maintainer decision.
- **`CameraRaw` is the mode that is genuinely unclamped above `1.0`** — and the
  docs said the opposite of that too. Found by measurement while pinning the
  above: sensor samples are normalised with `clamp(0.0, 1.0)`, but demosaicing
  runs next and the Malvar-He-Cutler Laplacian correction floors at zero with no
  ceiling, so high-contrast neighbourhoods overshoot; `Linear`/`Develop` lose the
  overshoot to the colour matrix, `CameraRaw` keeps it. The "not clamped" /
  "unclamped" / "highlights routinely exceed 1.0" claims were repeated in
  `README.md`, `README.crates.md`, `docs/architecture.md` and the `OutputMode`
  rustdoc; all four are corrected, and the ranges are pinned by
  `linear_output_is_clamped_to_unit_range`,
  `camera_raw_preserves_demosaic_overshoot_above_one` and
  `exposure_ev_multiplies_after_the_clamp`.

### Documentation

- **README docs audit, issue #7 (remaining items after cf551d2).** New "Output modes" table
  states the pixel format and bytes-per-channel per `OutputMode` (`Develop` = `RGB16_SRGB`
  u16 — there is no 8-bit mode; `Linear` / `CameraRaw` = f32) and how to narrow to u8; the
  "Reading the pixels out" section now says `PixelBuffer` is untyped and that the meaning of
  `into_vec()` / `as_contiguous_bytes()` bytes follows the mode. Quick start lists the helper
  crates the API hands you types from (`enough`, `zenpixels`, `bytemuck`). New "Resource
  limits & server error handling" section: `with_max_pixels` / `with_max_decode_bytes`
  defaults, the `RawLimitKind` each rejects with, and a `whereat::At<RawError>` match
  example (`.error()` / `.decompose()`). The `color::apply_srgb_gamma` /
  `color::f32_to_u8_srgb` footgun is documented in both the README and the doc comments:
  `f32_to_u8_srgb` is a pure quantiser that applies **no** transfer function, so the pair
  composes to exactly one sRGB encode (a unit test now pins that). `docs/architecture.md` no
  longer describes the removed `apply_gamma` / `auto_develop` fields or an RGB8 default;
  `OutputMode` doc comments no longer claim `Develop` can yield f32. `README.crates.md`
  regenerated (it had drifted: still named `RawError::Decode`).
- README backend table + note: iPhone ProRAW / 10-bit lossless-JPEG DNGs need the `rawler`
  backend — the default `rawloader` backend panics upstream on `sof.precision 10` (contained
  by zenraw as `RawError::Malformed`, but the file does not decode). Issue #10 item 2
  (d2b1d473).

### Changed

- deps: `zencodec = "0.1.26"` and `zencodec-testkit = "0.1.0"` now come from crates.io; the
  `[patch.crates-io]` git-rev pin (44ca7927) that carried the pre-release two-level
  `ErrorCategory` taxonomy, and the git-rev testkit dev-dep, are removed. No source change —
  `RawError`'s `CategorizedError` impl already matched the published 0.1.26 API.
- **`RawError` reshaped onto zencodec's two-level origin-first `ErrorCategory` taxonomy**
  (`Image`/`Request`/`Resource`/`Io`/`Internal`/`Stopped` — named `Lifecycle` in the PR #116
  pre-release, `Stopped` in the published 0.1.26). Every variant
  now maps to exactly one category via a new `#[cfg(feature = "zencodec")] impl
  CategorizedError for RawError` (8c069b6f):
  - `Decode(String)` split into `Malformed` (corrupt/invalid bitstream), `UnexpectedEof`
    (truncated input, including the pre-existing `data.len() < 64` guard), and `Io` (temp-file
    I/O, darktable-cli subprocess spawn/wait/timeout).
  - `InvalidInput(String)` split into `InvalidParameters` (caller-supplied path/config),
    `InvalidBuffer` (a caller pixel buffer with the wrong size — `dng_render::render*`), and
    `Malformed`/`UnexpectedEof` for PFM-content parse failures.
  - `Unsupported(String)` split three ways by origin: `UnsupportedFeature` (an image-bytes
    fault — an unrecognized CFA/sensor layout `rawler_backend.rs` can't demosaic), the new
    `#[cfg(feature = "zencodec")] UnsupportedOperation(zencodec::UnsupportedOperation)` (a
    caller-request fault — `streaming_decoder`/`animation_frame_decoder` in
    `zencodec_impl.rs`), and `Dependency` (an unclassified environment/external-tool fault —
    `darktable-cli` missing from `PATH`, or no decode backend feature compiled in).
  - `LimitExceeded(String)` split into `LimitExceeded(RawLimitKind, String)` (a configured or
    built-in ceiling — pixel count, decode working-set bytes, input size, PFM dimension/byte
    budgets) and a new `OutOfMemory(String)` (genuine `try_reserve` failure or an arithmetic
    size-computation overflow that could never be allocated). New crate-local
    `RawLimitKind` enum (`Width`/`Height`/`Pixels`/`Memory`/`InputSize`) carries which ceiling,
    without requiring the optional `zencodec` feature to be enabled.
  - `rawler::RawlerError` now routes through a new `From` impl distinguishing
    `Unsupported{..}` (→ `UnsupportedType`, a new image-bytes-unsupported-dialect variant) from
    `DecoderFailed` (→ `Malformed`), instead of collapsing both into one `Decode` string.
  - `Buffer(zenpixels::BufferError)` categorizes `AllocationFailed` as `Resource::OutOfMemory`
    and every other shape (`InvalidDimensions`, `StrideTooSmall`, ...) as `Internal::Bug`, since
    every call site computes both the byte buffer and its dimensions itself.
  - New `From<RawError> for At<zencodec::CodecError>` convenience bridge for a consumer that
    wants the shared envelope. zenraw's own zencodec trait impls (`RawDecoderConfig` /
    `RawDecodeJob` / `RawDecoder`) deliberately keep `type Error = At<RawError>` — not changed
    to `At<CodecError>` — so `RawError`'s category is recoverable by downcasting `At<RawError>`
    directly (or via the `From` bridge above), but does **not** survive erasure through a
    type-erased `Box<dyn Error>` boundary (e.g. `zencodec-testkit`'s
    `check_decode_error_envelope` / `check_decode_truncation_series`, both of which are
    designed to fail for exactly this "Pattern A" `type Error` shape — not wired up here for
    that reason, and because zenraw has no `EncoderConfig` for the testkit's other checks,
    which all require an encode/decode pair).
  - Bumped the optional `zencodec` dependency to `0.1.25` (pending release; `[patch.crates-io]`
    pins the unpublished two-level `ErrorCategory` reshape at a commit rev), and added the
    `zencodec-testkit` dev-dependency at the same rev (currently unused — see above).

### Added

- **`AllocPreference` honoured at untrusted decode allocations.** The decode
  pipeline's large, sensor-dimension-sized buffers (the normalized sensor
  buffer, the demosaic / non-Bayer `RGB f32` output, and the crop copy) now
  route through a per-call-site fallibility policy (`src/alloc_util.rs`): big
  untrusted buffers default to the fallible `try_reserve` path (graceful
  `RawError::LimitExceeded` instead of an abort), small bounded scratch stays
  infallible (`vec!`). The zencodec decode boundary sets it from
  `ResourceLimits::prefer_fallible_allocations`; the direct `decode()` API
  leaves it `CodecDefault` (each site keeps its default → behaviour unchanged).
  Explicit `Fallible` / `Infallible` override every site. No public API change.
- **`RawDecoderConfig::estimate_decode_resources`** (the `zencodec::decode::
  DecoderConfig` trait method): predicts peak memory (≈ 3× the `RGB f32`
  working set + fixed overhead, matching the measured 245.9 MiB / 6.12 MP
  develop anchor), serial threading, and wall-time, scaled to the
  `ComputeEnvironment` core count.
- Bumped the optional `zencodec` dependency to `0.1.24` (adds `AllocPreference`,
  `ResourceLimits::prefer_fallible_allocations`, and the
  `estimate_decode_resources` trait method).

### Changed

- **README overhaul + split crates.io README.** Full shields.io badge row
  (CI/crates.io/lib.rs/docs.rs/MSRV/license, `flat-square`), a `## Quick start`
  dependency block, a fallible-allocation note in the untrusted-input section,
  `with_target`/`with_exposure_ev`/`with_wb` and `estimate_decode_resources`
  documented, absolute license links, and the rendered crosslink footer. Added a
  generated `README.crates.md` (`readme = "README.crates.md"` in `Cargo.toml`)
  and `benchmarks/README.md` documenting the decode/heaptrack harness repro.

### Fixed

- **Untrusted-input hardening (no API change):**
  - `dng_render::eval_lut` no longer panics on an empty `ProfileToneCurve` LUT —
    `lut.len() - 1` underflowed/indexed out of bounds; now an empty curve is
    treated as identity and a single point as constant.
  - The default **rawler** backend (`rawler_backend::{decode,probe}`) now wraps
    `rawler::decode` in `catch_unwind`, matching the rawloader path, so an
    upstream panic on a malformed file is a typed `RawError`, not a process abort.
  - 32-bit (i686/wasm32): the attacker-controlled IFD/value offsets in
    `tiff_ifd::{read_entry_bytes,parse_ifd}` and `classify` now use `checked_add`
    so `offset + len` cannot overflow `usize` and wrap past the bounds check.

### Added

- `examples/heaptrack_decode.rs`: a reusable heaptrack/valgrind harness that
  decodes a camera RAW / DNG file from bytes via `zenraw::decode(.., Develop)` in a
  loop, for profiling heap-allocation behaviour. There is no committed RAW fixture
  (RAW files are large + licensing-encumbered), so it defaults to the block-storage
  `/mnt/v/input/raw-samples/nikon_d40.nef` (fetch via `just fetch-samples`) decoded
  8×; a path + iteration count can be passed. Driven by `just heaptrack-decode`.
  Profiled result is **healthy**: the develop pipeline is allocation-efficient —
  only ~65 allocations per *additional* decode (raw/RGB-f32/output buffers are
  reused; the decode loop barely allocates). The ~67k total allocations and ~6,751
  "leaked" / 19.7 MiB are a **one-time** cost: the `rawloader` backend deserializes
  its bundled `cameras.toml` camera-metadata database on first decode and retains it
  as a process-global cache (iteration-constant at 2/8/16 iterations — not a
  per-decode leak). Peak heap is 245.9 MiB for the 6.12 MP NEF (~3.3× the 73 MiB
  RGB-f32 intermediate; O(image) develop working set, no per-pixel/per-block churn).
- The `zencodec` decode adapter now honors `OrientationHint` (default `Preserve`): `with_orientation()` is overridden on `RawDecodeJob`, and `probe` / `output_info` / `decode` report dimensions and the EXIF Orientation tag consistently for the resolved hint. `Preserve` returns stored-orientation pixels + stored dims + the intrinsic tag; `Correct` / `CorrectAndTransform` / `ExactTransform` physically bake the resolved orientation into the decoded buffer and report display dims + `Orientation::Identity`. Adapter-only — the native `RawDecodeConfig` API and its `apply_orientation` default are unchanged. (`src/zencodec_impl.rs`)
- `orient::apply_orientation_bytes` (`pub(crate)`): a format-agnostic, whole-pixel byte-level orientation baker used by the adapter to bake arbitrary resolved orientations onto the decoded `RGB16`/`RGBF32` buffer. Verified bit-for-bit against the existing f32 baker for all 8 EXIF orientations. (`src/orient.rs`)
- `tests/orientation.rs`: end-to-end orientation tests over real RAW files (corpus-gated on `ZENRAW_RAW_SAMPLES_DIR` / FiveK DNG dir, caller-controlled skip), plus deterministic in-crate pixel-oracle and adapter-contract tests in `src/orient.rs` and `src/zencodec_impl.rs`.

### Changed

- Docs: clarified that DNG OpcodeList2 opcode-9 `GainMap` is a *lens shading table* (per-channel spatially-varying gain for vignette correction), distinct from the ISO 21496-1 / Apple HDR gain map (`apple::GainMapInfo`); disambiguating phrasing added across `docs/dng-format.md`, `docs/lens-corrections.md`, `docs/roadmap.md`, `docs/color-pipeline.md`, and `docs/reference-research.md` while retaining the spec name `GainMap`. (closes #2)
- Public-API snapshots migrated to the `zenutils-apidoc` 0.1.0 runner package
  at `apidoc/` (self-contained, CI-free): three snapshot files under
  `docs/public-api/`, regenerated via `just api-doc`. Replaces the in-crate
  `tests/public_api_doc.rs` copy, its `serde_json` dev-dep, and every
  `ZEN_API_DOC` / cargo-public-api trace in CI (ci.yml + fuzz.yml).
- Bumped `zencodec` 0.1.13 → 0.1.21 (required for `OrientationHint` + the `DecodeJob::with_orientation` trait method; the adapter compiled against the new version with no other API changes needed). (`Cargo.toml`)
- Removed `tests/` and `benches/` from the published package `include` list; downstream consumers no longer receive test code they cannot use.

### Removed

- The internal `pub(crate)` `IntoBufferError` trait (`src/error.rs`) and its two
  impls. Its rationale ("zenpixels 0.1.0 returns bare `BufferError`, local
  versions return `At<BufferError>`") was obsolete — `Cargo.toml` pins
  `zenpixels` 0.2.10, which always returns `At<BufferError>` — and the
  `At<BufferError>` impl flattened the trace via `.decompose().0`. Not a
  public-API change (the trait was crate-private). The two trait-only unit tests
  (`into_buffer_error_bare`, `into_buffer_error_at`) were removed with it (they
  tested the deleted trait); `from_buffer_error` (covering the retained bare
  `From<BufferError>`) stays.

### Fixed

- **Preserve the `BufferError` trace across the `PixelBuffer` boundary.** The 11
  decode sites that build a `PixelBuffer` (6 in `decode.rs`, 3 in
  `rawler_backend.rs`, 2 in `darktable.rs`) used
  `.map_err(|e| at!(RawError::Buffer(e.into_buffer_error())))`, where
  `e: At<BufferError>` was flattened by `into_buffer_error()` (`.decompose().0`
  dropped the frames) and then re-wrapped in a fresh single-frame `at!`. They now
  use `.map_err_at(RawError::Buffer)`, which applies the `RawError::Buffer` tuple
  constructor to the inner bare `BufferError` while keeping the original `At`
  trace frames (`RawError::Buffer` holds a bare `BufferError`). The callee's
  location frames now survive into `At<RawError>`.
- The rawler decode backend is now panic-isolated: `rawler_backend::decode`
  wraps the `rawler::decode` call in `std::panic::catch_unwind`, mirroring the
  rawloader backend, so a malformed/crafted RAW routed to rawler returns
  `RawError::Decode` instead of unwinding through and crashing the host process.
  Also hardened `dng_render::bradford_adapt`'s Bradford-matrix inversion
  `.unwrap()` into `.expect(...)`. Regression coverage in
  `tests/rawler_panic.rs` (rawler-feature-gated; defensive malformed-input cases
  always run, the good-decode path is corpus-gated on `ZENRAW_RAW_CORPUS`).
  Closes #12. (`src/rawler_backend.rs`, `src/dng_render.rs`)
- docs(readme): show how to read f32 pixels from the `PixelBuffer` (+ channel
  layout / value range), document `Stop` construction (`Unstoppable` no-op and a
  cancellable `almost_enough::Stopper`), and add an honest untrusted-input
  panic-safety note (rawloader catches backend panics via `catch_unwind`; rawler
  does not — isolate hostile decodes on a thread). The README previously named
  `output.pixels` as a `zenpixels::PixelBuffer` but documented no accessor to get
  the linear-f32 RGB slice out — found by an insulated external-developer test.
- `probe` now reports the same dimensions `decode` produces under default
  settings (crop applied). The rawloader backend's `probe` previously reported
  the full uncropped sensor size while `decode` applied the camera's crop, so
  `probe` and a default `decode` disagreed on the output width/height (e.g. a
  Nikon NEF probed as 3040×2014 but decoded to 3038×2014). `probe` and
  `apply_crop` now share a single `cropped_dims` helper so they can never
  diverge on the post-crop geometry; `sensor_width`/`sensor_height` still report
  the full uncropped sensor. The rawler backend already reported crop-applied
  dims and is unchanged. (`src/decode.rs`)
- `tests/probe_parity.rs` now compares `probe` against `decode(crop = true,
  orientation = false)` — the config whose geometry `probe` actually represents
  (crop-applied, stored sensor orientation) — instead of the uncropped
  `decode(crop = false, …)`, which encoded the wrong contract and asserted
  `probe == uncropped decode`. The two parity tests now pass on both the rawler
  and rawloader backends. (`tests/probe_parity.rs`)
- `tests/probe_parity.rs` RAW-corpus tests are now gated on the
  `ZENRAW_RAW_SAMPLES_DIR` env var instead of a hard-coded local-only
  `/mnt/v/input/raw-samples` path with a buried file-missing skip. CI
  leaves the var unset, so the corpus-dependent tests skip cleanly with a
  visible message rather than reaching `assert!(tested > 0)` with zero
  samples (which panicked in CI). When the var IS set, missing listed
  samples are hard failures. Added a `test-raw-parity` justfile target
  that sets the var to the canonical local corpus path, documenting the
  CI -> justfile -> test skip chain.

### Changed

- `tests/fuzz_regression.rs` now uses the shared `zen-fuzz-regress`
  test-helper crate (DEDUP-J2). Behaviour is unchanged — same
  `fuzz/regression/` seeds, same two targets (`classify`, `decode`),
  same panic-propagation failure semantics. The in-file `collect_seeds`
  scaffolding is now provided by `RegressionSuite`.

### Added

- `tests/fuzz_regression.rs` regression-harness template ported from
  zenwebp (DEDUP-J). Walks `fuzz/regression/` (incl. per-target subdirs)
  and runs every seed through `classify` and `decode` (with the same
  format-filtering the fuzz target uses) on the stable toolchain — no
  nightly required. Created `fuzz/regression/README.md` documenting how
  to add minimized crash seeds.

## [0.2.0] - 2026-04-17

### BREAKING CHANGES
- `apply_color_pipeline()` gained a 4th parameter

### Added
- Thread `OutputPrimaries` through color pipelines and `PixelDescriptor` so callers can request sRGB, Display P3, or BT.2020 output from both the rawloader-backed and rawler-backed decode paths (172c238)
- Probe-vs-decode parity tests verifying `classify`, `is_raw_file`, and `probe` agree with actual decode capability across DNG, NEF, CR2, ARW, ORF, RW2, and RAF (6ddd8d3)

### Changed
- Use `memchr::memmem` for the DNG header scan and XMP packet marker search; large multi-MB DNG XMP-miss scans drop from ~3 ms to ~65 us on a 5 MB FiveK DNG (c5035b5)
- Improve test coverage from 74% to 78% across `simd`, `error`, and `zencodec_impl`, and add a rawloader-only CI job so `decode.rs` is exercised without the rawler feature (47d50df)

### Fixed
- Detect Olympus ORF `IIRS` magic variant (`0x52 0x53`) alongside the existing `IIRO` so `classify()` and `is_raw_file()` correctly recognize cameras like the C5050Z (45eb9cd)
- Collapse nested `if` statements to satisfy `clippy::collapsible_if` on Rust 1.94 (c56778a)

## [0.1.2] - 2026-04-10

### Added
- cargo-fuzz infrastructure; harden decode against upstream panics from rawloader/rawler backends (89db3fa)
- Nightly fuzz workflow (60s on push, 5min nightly) (23f2995)

### Changed
- Collapse duplicate `normalize_uniform` into a `#[magetypes]` generic (21ad6ab)
- Bump zencodec to 0.1.13 (586405c)
- Gitignore tooling noise and exclude from published package (dd7d3c0)
- Commit `fuzz/Cargo.lock` for reproducible fuzz builds (6fcceb4)
- Gitignore `fuzz/corpus/` (a59c207)
- `cargo fmt` (b6491e8)

## [0.1.1] - 2026-04-01

### Changed
- Update dependencies: archmage 0.9.16, magetypes 0.9.16, linear-srgb 0.6.7, ultrahdr-core 0.3.4, zencodec 0.1.12, zenbench 0.1.3

## [0.1.0] - 2026-03-04

### Added
- Initial release
