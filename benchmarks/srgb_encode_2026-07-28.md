# `linear_to_srgb_u8`: a 14× SIMD path exists and was NOT taken — 2026-07-28

Platform: Apple Silicon (aarch64, NEON), darwin 25.5.0
Measured over 4.1M samples spanning [0,1], densified around the 0.0031308 knee where the
piecewise branches meet.

`linear_to_srgb_u8` (`src/dng_render.rs`) runs over every output sample of every RAW develop.
It evaluated `v.powf(1.0/2.4)` per element and pushed into a `Vec`.

## Options measured

| implementation | throughput | wrong vs exact f64 reference |
|---|---|---|
| current (`powf` + `Vec::push`) | 349 Melem/s | 17 / 4.1M — **0.0004%** |
| `powf` + preallocated slice | 371 Melem/s (1.06×) | identical to current |
| `linear-srgb` rational polynomial SIMD | **4963 Melem/s (14.2×)** | 80,926 / 4.1M — **1.97%** |

## What shipped, and what did not

**Shipped:** the preallocated-slice form. Bit-identical (same `powf`, same rounding), worth
only 1.06% because `powf` dominates the loop — unlike the push-bound loops elsewhere in this
workspace where removing `push` was worth 3–40×. Free, so taken.

**Not shipped:** the 14.2× SIMD path. `linear-srgb::default::linear_to_srgb_u8_slice` is
substantially faster but uses a rational polynomial approximation, and against an exact f64
reference it is wrong on 1.97% of samples where the current `powf` form is wrong on 0.0004%
— roughly **4600× more wrong pixels**. Every difference is ±1 u8, so it is imperceptible in
isolation, but this is a RAW developer whose entire purpose is output fidelity, and trading
4600× more quantization error for speed is a product decision, not a NEON-sweep decision.

`linear-srgb::precise` was checked as a possible both-ways answer: it is scalar `powf`, so it
offers accuracy without the speedup. There is no existing path that is both.

## If this is revisited

The clean resolution is to expose it as a choice rather than pick one — this crate already has
an `OutputMode`, so a fidelity-vs-speed knob would fit the existing shape. That is a public
API addition, so it needs sign-off.

A LUT is also viable and would be exact: the output is u8, so there are only 256 buckets, and
the linear-space thresholds between them could be precomputed once. That is a real kernel, not
a swap, and was not attempted here.
