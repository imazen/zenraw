# Changelog

## [Unreleased]

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
