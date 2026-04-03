//! SIMD-accelerated inner loops for the decode pipeline.
//!
//! Uses archmage/magetypes for cross-platform SIMD with runtime dispatch.
//! AVX2+FMA on x86-64, NEON on aarch64, WASM SIMD128, scalar fallback.

use alloc::vec;
use alloc::vec::Vec;

use archmage::prelude::*;
use magetypes::simd::generic::f32x8 as GenericF32x8;

// ── Normalize ────────────────────────────────────────────────────────────

/// Normalize f32 sensor data: `(sample - black) * inv_range`, clamped to [0, 1].
///
/// All pixels share the same black/inv_range. This is the fast path for
/// non-CFA data (cpp > 1) or when all CFA channels have identical levels.
#[allow(dead_code)]
pub fn normalize_uniform(data: &[f32], black: f32, inv_range: f32) -> Vec<f32> {
    incant!(
        normalize_uniform_inner(data, black, inv_range),
        [v3, neon, wasm128, scalar]
    )
}

#[magetypes(v3, neon, wasm128, scalar)]
fn normalize_uniform_inner(token: Token, data: &[f32], black: f32, inv_range: f32) -> Vec<f32> {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;

    let mut out = vec![0.0f32; data.len()];
    let black_v = f32x8::splat(token, black);
    let inv_range_v = f32x8::splat(token, inv_range);
    let zero = f32x8::zero(token);
    let one = f32x8::splat(token, 1.0);

    let (src_chunks, src_tail) = f32x8::partition_slice(token, data);
    let (dst_chunks, dst_tail) = f32x8::partition_slice_mut(token, &mut out);

    for (src, dst) in src_chunks.iter().zip(dst_chunks.iter_mut()) {
        let v = f32x8::load(token, src);
        let normalized = (v - black_v) * inv_range_v;
        let clamped = normalized.max(zero).min(one);
        clamped.store(dst);
    }

    for (s, d) in src_tail.iter().zip(dst_tail.iter_mut()) {
        *d = ((*s - black) * inv_range).clamp(0.0, 1.0);
    }

    out
}

// ── Non-Bayer channel extraction ─────────────────────────────────────────

/// Extract RGB from cpp-interleaved data (cpp >= 3).
///
/// For cpp==3, this is a zero-copy clone. For cpp>3, drops extra channels.
#[allow(dead_code)]
pub fn extract_rgb_from_cpp(data: &[f32], pixel_count: usize, cpp: usize) -> Vec<f32> {
    if cpp == 3 {
        let len = pixel_count * 3;
        if data.len() >= len {
            return data[..len].to_vec();
        }
    }

    let mut rgb = Vec::with_capacity(pixel_count * 3);
    for i in 0..pixel_count {
        let base = i * cpp;
        rgb.push(if base < data.len() { data[base] } else { 0.0 });
        rgb.push(if base + 1 < data.len() {
            data[base + 1]
        } else {
            0.0
        });
        rgb.push(if base + 2 < data.len() {
            data[base + 2]
        } else {
            0.0
        });
    }
    rgb
}

// ── sRGB gamma ───────────────────────────────────────────────────────────

/// Linear to sRGB transfer function.
#[inline]
pub(crate) fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.0031308 {
        x * 12.92
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}
