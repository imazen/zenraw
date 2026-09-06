# zenraw development notes

## Known Bugs

### Fixed in the current ARM audit: normalization changed floating-point bits

`src/simd.rs::normalize_uniform_into` used vector min/max but scalar
`f32::clamp` for its tail. On M4 Pro, an eight-element block changed -0.0
into +0.0 while the scalar reference preserved its sign. The regression
`normalization_matches_scalar_bits_for_special_values` reproduces that
mismatch before the fix. Ordered comparisons and blends now use the same
clamping decisions in every lane, preserving NaNs and signed zero.

See `benchmarks/arm_audit_2026-09-06/README.md` for measurements and limits.
