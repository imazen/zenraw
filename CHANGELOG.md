# Changelog

## [Unreleased]

### Added

- The `zencodec` decode adapter now honors `OrientationHint` (default `Preserve`): `with_orientation()` is overridden on `RawDecodeJob`, and `probe` / `output_info` / `decode` report dimensions and the EXIF Orientation tag consistently for the resolved hint. `Preserve` returns stored-orientation pixels + stored dims + the intrinsic tag; `Correct` / `CorrectAndTransform` / `ExactTransform` physically bake the resolved orientation into the decoded buffer and report display dims + `Orientation::Identity`. Adapter-only — the native `RawDecodeConfig` API and its `apply_orientation` default are unchanged. (`src/zencodec_impl.rs`)
- `orient::apply_orientation_bytes` (`pub(crate)`): a format-agnostic, whole-pixel byte-level orientation baker used by the adapter to bake arbitrary resolved orientations onto the decoded `RGB16`/`RGBF32` buffer. Verified bit-for-bit against the existing f32 baker for all 8 EXIF orientations. (`src/orient.rs`)
- `tests/orientation.rs`: end-to-end orientation tests over real RAW files (corpus-gated on `ZENRAW_RAW_SAMPLES_DIR` / FiveK DNG dir, caller-controlled skip), plus deterministic in-crate pixel-oracle and adapter-contract tests in `src/orient.rs` and `src/zencodec_impl.rs`.

### Changed

- Public-API snapshots migrated to the `zenutils-apidoc` 0.1.0 runner package
  at `apidoc/` (self-contained, CI-free): three snapshot files under
  `docs/public-api/`, regenerated via `just api-doc`. Replaces the in-crate
  `tests/public_api_doc.rs` copy, its `serde_json` dev-dep, and every
  `ZEN_API_DOC` / cargo-public-api trace in CI (ci.yml + fuzz.yml).
- Bumped `zencodec` 0.1.13 → 0.1.21 (required for `OrientationHint` + the `DecodeJob::with_orientation` trait method; the adapter compiled against the new version with no other API changes needed). (`Cargo.toml`)
- Removed `tests/` and `benches/` from the published package `include` list; downstream consumers no longer receive test code they cannot use.

### Fixed

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
