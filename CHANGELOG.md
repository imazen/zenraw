# Changelog

## [Unreleased]

### QUEUED BREAKING CHANGES
<!-- Breaking changes that will ship together in the next major (or minor for 0.x) release.
     Add items here as you discover them. Do NOT ship these piecemeal — batch them. -->

### Added
- Wire `OutputPrimaries` through all color pipelines and descriptors (172c238)
- Probe-vs-decode parity tests (6ddd8d3)

### Changed
- Use `memchr` SIMD for DNG header scan and XMP marker search (c5035b5)
- Improve test coverage from 74% to 78% across all modules (47d50df)

### Fixed
- Detect Olympus ORF "IIRS" magic variant (0x52 0x53) (45eb9cd)
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
