# `linear_to_srgb_u8`: a 14× SIMD path exists and was NOT taken — 2026-07-28

Platform: Apple Silicon (aarch64, NEON), darwin 25.5.0
Measured over 4.1M samples spanning [0,1], densified around the 0.0031308 knee where the
piecewise branches meet.

`linear_to_srgb_u8` (`src/dng_render.rs`) runs over every output sample of every RAW develop.
It evaluated `v.powf(1.0/2.4)` per element and pushed into a `Vec`.

## Options measured

| implementation | throughput | wrong vs exact f64 reference |
|---|---|---|
| original (`powf` + `Vec::push`) | 344 Melem/s | 17 / 4.1M — 0.0004% |
| `powf` + preallocated slice | 366 Melem/s (1.06×) | identical to original |
| `linear-srgb` rational polynomial SIMD | 4862 Melem/s (14.1×) | 80,926 / 4.1M — **1.97%** |
| **exact 256-threshold table (SHIPPED)** | **544 Melem/s (1.58×)** | **8 / 4.1M — 0.0002%** |

## What shipped: the threshold table — faster AND more accurate

An earlier revision of this note deferred the LUT idea as "a real kernel, not a swap, and was
not attempted here". That deferral was wrong; it took one measurement to settle.

`out == k` exactly when `srgb(v)*255 + 0.5` lands in `[k, k+1)`, so the linear-space threshold
for level `k` is `srgb_inv((k-0.5)/255)`, evaluated once in f64. Encoding is then a branchless
8-step binary search over 256 thresholds — no transcendental at all.

**1.58× faster than `powf` and wrong half as often** (8 vs 17 samples in 4.1M). It is strictly
better on both axes, so there is no tradeoff to hand to anyone. The residual 8 come from
storing thresholds as f32, so a value within an ULP of a boundary can land either side.

It is deliberately NOT vectorized: the search indexes the table by a per-lane value, which is
a gather, and AArch64 has no gather instruction — the same wall that stopped the GIF palette
expansion earlier in this sweep.

## What did not ship

The 14.2× SIMD path. `linear-srgb::default::linear_to_srgb_u8_slice` is
substantially faster but uses a rational polynomial approximation, and against an exact f64
reference it is wrong on 1.97% of samples where the current `powf` form is wrong on 0.0004%
— roughly **4600× more wrong pixels**. Every difference is ±1 u8, so it is imperceptible in
isolation, but this is a RAW developer whose entire purpose is output fidelity, and trading
4600× more quantization error for speed is a product decision, not a NEON-sweep decision.

`linear-srgb::precise` was checked as a possible both-ways answer: it is scalar `powf`, so it
offers accuracy without the speedup. There is no existing path that is both.

## If the 14× is ever wanted anyway

Exposing it as a choice would fit — this crate already has an `OutputMode`, so a
fidelity-vs-speed knob matches the existing shape. That is a public API addition and needs
sign-off. With the threshold table now at 1.58× for free and *better* accuracy, the remaining
gap is 9×, bought with 4600× more wrong pixels.
