# zenraw development notes

## Known Bugs

### Incremental input admission

PR25 asked the shared whole-file helper for unbounded prefix growth and checked
the compressed input cap only after the full source was buffered. A private
guard now admits lengths before requests, retains resource/lifecycle failures,
and estimates compressed input plus decoded output before reporting a header.
Unknown input permits one EOF lookahead byte only within the memory budget.
`just incremental-admission-check` exercises zero/exact caps, offsets, overflow,
terminal errors, early headers and byte-exact output into strided sink rows.
The exact pinned graph passes this focused suite with both rawloader and rawler.
The sink uses nonzero whole-pixel padding required by zenpixels0.2.16; exact
pixel equality and untouched padding are asserted. A transient job-stop witness
also checks the cached preflight handoff. Negative controls reproduce both
missing admission and swallowed cancellation when the guard/cache is removed.
These are focused local gates, not a
complete release or platform validation claim.

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

### Cancellation propagation (PR #18 review)

Private demosaic, orientation, normalization, and sigmoid kernels return their
cancellation error directly. Re-polling a caller-supplied `enough::Stop` cannot
recover a discarded error: the trait does not require errors to remain sticky.
`single_poll_cancellation_is_never_lost` exercises each observed decode poll
with a token that fails exactly once, on both rawloader and rawler builds.
Flat sample passes poll between batches, outside their pixel loops.

### Progress integration (PR #18 revision)

The native decode/probe hook is `Option<&dyn how_far::Pulse>`. `None` disables
observation; existing stop tokens work through `StopOnly::borrowed`. Decode
owns and completes only child phases. The external RAW parser calls have no
interior progress/cancellation hook. `tests/pulse_progress.rs` exercises both
backends, exact completed counts (including tails), pixel identity, cancellation
outcomes, final-report cancellation, and non-sticky stop errors. The rawler
non-Bayer extraction now also checks and reports between batches.
