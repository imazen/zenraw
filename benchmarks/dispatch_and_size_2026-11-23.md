# zenraw Dispatch & Binary Size Measurements (2026-11-23)

## Summary of Leaf Changes

- `src/simd.rs`: extracted the scalar remainder loop in `normalize_uniform_into` into `#[cold] #[inline(never)] fn normalize_uniform_tail_scalar`, preventing LLVM from auto-vectorizing cold tail code inside AVX2 (`v3`) instantiations (eliminates `vmaskmov` tail instructions and keeps the AVX2 hot loop compact).
- `src/color.rs:51,267` (`apply_color_matrix_stop`, `f32_to_u8_inner`) and `src/demosaic.rs:525` (`malvar_interior_stop`): switched `#[autoversion]` to `#[autoversion(v3, neon, wasm128, scalar)]`, dropping unused AVX-512 (`v4`) tier specializations (`zenraw::.*_v4`: `3 -> 0`). All `15` remaining `v4` symbols in linked binaries belong to `rawler::decoders::rw2::v4decompressor`.

## Verification Gates (`i265`, `rustc 1.99.1`)

- `cargo fmt --check`
- `cargo check --target wasm32-unknown-unknown --no-default-features`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --release`
- `cargo test --release --features rawler,zencodec,inventory`
- `cargo test --doc`
- Bundled parity check (`r5600g`, `rustc 1.98.0`): `BUNDLE PARITY OK (tail_17, odd_200084, 517x387 color/u8/malvar)` (0 diffs vs unedited `dc501952`).

## Native ELF & Wasm Size (`i265`, `rustc 1.99.1`, `-C strip=none`, no `-C target-cpu=native`)

| Target | Metric | BEFORE (`dc501952`) | AFTER | Delta |
|---|---|---:|---:|---:|
| `decode_bench` (`--features rawler`) | `.text` | 8,846,693 B | 8,840,533 B | -6,160 B |
| `kernel_tiers` (`--features _dev,rawler`) | `.text` | 8,821,601 B | 8,815,509 B | -6,092 B |
| Both ELFs | `vmaskmov` insns | 2 | 0 | -2 |
| Both ELFs | `simd_inner` symbols | 0 | 0 | 0 |
| Both ELFs | `zenraw::.*_v4` symbols | 3 | 0 | -3 |
| Wasm cdylib probe (`wasm32-unknown-unknown`, `+simd128`) | raw / stripped | 25,902 B | 25,902 B | 0 B |
| Wasm cdylib probe (`wasm32-unknown-unknown`, `+simd128`) | `gzip -n -9` | 11,073 B | 11,073 B | 0 B |

Baseline clean rebuild on `r5600g` (`rustc 1.98.0`, unedited `dc501952`, `-C strip=none`): `decode_bench` `.text = 8,840,249 B` (`vmaskmov: 2`), `kernel_tiers` `.text = 8,812,613 B` (`vmaskmov: 2`).

## Interleaved Before/After `zenbench` Timings (`r5600g`, Ryzen 5 5600G, pinned `taskset -c 4`, `rustc 1.98.0`, no `-C target-cpu=native`)

Run ID: `18ea97ba61e9faa6-180f30-a15828387e163d62` (`25` rounds per group, bundled `zenraw_before` vs `zenraw`).

| Benchmark | BEFORE (mean ± MAD) | AFTER (mean ± MAD) | 95% CI vs BEFORE | Assessment |
|---|---:|---:|---|---|
| `normalize_uniform_tail_17` (32K calls/round) | 22.7 ± 1.0 ns | 18.8 ± 0.7 ns | `[-19.7%, -15.8%]` | Faster (excludes zero) |
| `normalize_uniform_odd_200084` (28 calls/round) | 30.6 ± 1.0 µs | 28.3 ± 1.1 µs | `[-11.2%, -3.7%]` | Faster (excludes zero) |
| `apply_color_pipeline_517x387` (16 calls/round) | 234.9 ± 11.0 µs | 225.3 ± 3.3 µs | `[-1.1%, +1.5%]` | Neutral (CI crosses zero) |
| `f32_to_u8_srgb_517x387` (10 calls/round) | 473.0 ± 3.6 µs | 493.8 ± 6.1 µs | `[-0.6%, +1.4%]` | Neutral (CI crosses zero) |
| `malvar_demosaic_517x387` (4 calls/round) | 1.2 ± 0.0 ms | 1.2 ± 0.1 ms | `[-1.6%, +4.7%]` | Neutral (CI crosses zero) |

### Reference `kernel_tiers` (`dc501952` on `r5600g`, `v3(avx2)` vs `scalar`, 25 rounds)

- `normalize_uniform/17`: `v3(avx2)` `46.6 ± 2.4 ns` vs `scalar` `28.0 ± 0.8 ns` (`-37.4% to -34.7%`)
- `normalize_uniform/64x64`: `v3(avx2)` `484 ± 10 ns` vs `scalar` `569 ± 24 ns` (`+14.1% to +17.7%`)
- `normalize_uniform/256x256`: `v3(avx2)` `8.9 ± 0.2 µs` vs `scalar` `9.4 ± 0.1 µs` (`+7.0% to +7.9%`)
- `normalize_uniform/1024x1024`: `v3(avx2)` `203.9 ± 15.6 µs` vs `scalar` `218.6 ± 19.4 µs` (`+7.1% to +17.2%`)
- `normalize_uniform/4096x4096`: `v3(avx2)` `37.5 ± 1.4 ms` vs `scalar` `36.9 ± 1.3 ms` (`+0.1% to +5.4%`, `[1] CI crosses zero`)
- `normalize_uniform/24MP`: `v3(avx2)` `54.0 ± 2.4 ms` vs `scalar` `57.1 ± 2.6 ms` (`-2.5% to +14.4%`, CI crosses zero)
