//! NEON-vs-forced-scalar for zenraw's one SIMD kernel.
//!
//! `normalize_uniform_into` is the crate's only dispatched kernel (both public
//! entry points funnel into it) and had no tier benchmark, so a path slower
//! than the scalar tier it dispatches away from was invisible. That failure
//! mode was real elsewhere in the 2026-07-29 aarch64 sweep — zenquant 0.58x,
//! linear-srgb 0.93x, zenresize 0.94x.
//!
//! NEON is BASELINE on aarch64, so the "scalar" arm is autovectorized too:
//! ~1.00x means LLVM already matched it, BELOW 1.00 is a bug.
//!
//! Run: `cargo bench --bench kernel_tiers`

use zenbench::prelude::*;

#[cfg(target_arch = "aarch64")]
type TierToken = archmage::NeonToken;
#[cfg(target_arch = "x86_64")]
type TierToken = archmage::X64V3Token;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
const TIER_NAME: &str = if cfg!(target_arch = "aarch64") {
    "neon"
} else {
    "v3(avx2)"
};

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn set_simd(on: bool) -> bool {
    use archmage::SimdToken;
    TierToken::dangerously_disable_token_process_wide(!on).is_ok()
}
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn set_simd(_on: bool) -> bool {
    false
}

fn bench(suite: &mut Suite) {
    if !set_simd(true) || !set_simd(false) {
        eprintln!("[kernel_tiers] SIMD tier not toggleable here. Skipping.");
        return;
    }
    set_simd(true);

    // A 24 MP sensor frame — the realistic size for this kernel.
    for (label, n) in [("1MP", 1usize << 20), ("24MP", 24 << 20)] {
        let data: &'static [f32] = Box::leak(
            (0..n)
                .map(|i| (i % 4096) as f32)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        suite.compare(&format!("normalize_uniform/{label}"), move |g| {
            g.throughput(Throughput::Bytes((n * 4) as u64));
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                g.bench(arm, move |b| {
                    b.with_input(move || set_simd(simd))
                        .run(move |_| zenraw::simd::normalize_uniform(data, 512.0, 1.0 / 3583.0))
                });
            }
        });
    }
    set_simd(true);
}

zenbench::main!(bench);
