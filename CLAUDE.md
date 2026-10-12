# zenraw development notes

## Known Bugs

### Fixed: inventory lost transient timeout reasons

Inventory retained only a cancellation boolean and re-polled the token while
constructing the error. A token reporting a timeout once became generic
cancellation. Traversal now captures the first reason across all nested RAW
containers, and the adapter returns that reason directly.

### Fixed: allocation-error fixture could succeed on i686

The fallible-allocation tests requested `usize::MAX / 8` f32 elements, which
fits within the 32-bit address space and can succeed. Both fixtures now request
`usize::MAX` elements, keeping the same required `RawError::OutOfMemory` result
while guaranteeing capacity overflow on either pointer width. No production
allocation behavior changed.

### Fixed in the current ARM audit: normalization changed floating-point bits

`src/simd.rs::normalize_uniform_into` used vector min/max but scalar
`f32::clamp` for its tail. On M4 Pro, an eight-element block changed -0.0
into +0.0 while the scalar reference preserved its sign. The regression
`normalization_matches_scalar_bits_for_special_values` reproduces that
mismatch before the fix. Ordered comparisons and blends now use the same
clamping decisions in every lane, preserving NaNs and signed zero.

See `benchmarks/arm_audit_2026-09-06/README.md` for measurements and limits.
