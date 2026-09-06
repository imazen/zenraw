# Native ARM normalization audit, 2026-09-06

Apple M4 Pro, macOS 26.5.2, rustc 1.98.0, LLVM 22; no target-cpu=native.
Source baseline: 20220cf30a9b1ea097c0929869efc7b57acdf217. The benchmark
adds sizes and exact f32-bit equality checks; production code is unchanged.

`cargo bench --features _dev --bench kernel_tiers`, nice 19, four build/worker
threads. 200 interleaved rounds per case; 146.7 seconds total. Allocation is
included in both arms; token switching is outside timing.

| Samples | NEON | scalar tier | Paired scalar-over-NEON interval |
|---|---:|---:|---:|
| 17 | 41.2 ns | 43.7 ns | +4.2% to +7.4% |
| 64×64 | 1.1 us | 1.3 us | +10.3% to +13.5% |
| 256×256 | 16.0 us | 16.9 us | +2.5% to +6.5% |
| 1024×1024 | 144.1 us | 143.1 us | -2.4% to +2.2% |
| 4096×4096 | 2.7 ms | 2.8 ms | -1.3% to +2.9% |
| 24×2^20 | 4.1 ms | 4.1 ms | -1.9% to +2.2% |

The last three comparisons are ties. Some smaller cells have CV 21–25%; use
paired intervals, not rounded ratios. This measures normalization, not full RAW
processing or a camera-quality sweep.

Disassembly in zenraw-normalize.asm shows the NEON f32x8 loop as two q-register
loads, fsub/fmul/fmax/fmin, and two stores. The scalar tier auto-vectorizes too
(fmaxnm/fminnm), with a wider unrolled loop. No per-vector function call crosses
a target-feature boundary. Finite sensor inputs in all six cases match bit for
bit. Non-finite input semantics are not established by these comparisons.

Baseline default+_dev tests and strict library/bench clippy passed; feature-gated
backend tests are not covered by that baseline. Whole-decode profiling follows
separately; no end-to-end speedup is claimed.

## Whole decode, both backends

Fixture: `/Users/lilith/work/codec-artifacts/zenraw-arm-audit/nikon_d40.nef`,
5,844,273 bytes, SHA256
`44e88bc77b7a531b22647bcd07b9393c4568062e8f0906d3bbdecb42fbe29e03`.
Rawloader outputs 3038×2014; rawler outputs 3008×2000. The backend outputs
are not asserted equal to each other. Each backend's native/scalar pair has
identical dimensions and every output byte matches, in all three modes.

| Backend / mode | Native ms | Scalar ms | Paired scalar delta 95% CI |
|---|---:|---:|---|
| rawloader / Develop | 114.1 | 113.3 | −1.8% to +0.4% |
| rawloader / Linear | 61.4 | 61.3 | −1.7% to +1.4% |
| rawloader / CameraRaw | 58.2 | 58.4 | −1.2% to +1.7% |
| rawler / Develop | 242.7 | 243.4 | −0.3% to +0.9% |
| rawler / Linear | 67.7 | 67.7 | −1.0% to +1.1% |
| rawler / CameraRaw | 64.4 | 64.4 | −1.0% to +1.2% |

All six intervals cross zero. No whole-decode speedup is claimed; one NEF
fixture does not represent every camera, sensor layout or RAW format.
The configurations use different output crops and development paths, so the
table is not a backend quality/speed ranking.

Commands: `ZENRAW_BENCH_INPUT=<fixture> cargo bench --features _dev --bench
kernel_tiers -- --group=decode`, then the same command with
`--features rawler,_dev`. Both used the resource settings above. Harness source
is `benches/kernel_tiers.rs`; token toggles and correctness checks are outside
timing. Both arms include decoding and output allocation.

## Exact floating-point clamp correction

The `normalization_matches_scalar_bits_for_special_values` regression fails
on the previous vector body at length 8: -0.0 becomes +0.0. Scalar tails
use `f32::clamp`, whose comparison decisions preserve that sign. Ordered
comparisons and `f32x8::blend` now reproduce those decisions, including NaN
payloads. The test rotates 11 edge values through lengths 1–33 to cover
vector lanes and tails, and requires bitwise equality with scalar arithmetic.

87 native library tests pass; strict clippy passes. The focused regression
also passes x86_64 through Rosetta and WASM SIMD under Wasmtime. Native CI
now enables `_dev` for this regression. No public API changed. Earlier kernel
and whole-decode timing tables precede this arithmetic correction; post-fix
measurements are pending and must not be inferred from those tables.
