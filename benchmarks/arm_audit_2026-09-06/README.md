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
