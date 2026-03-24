//! SIMD-accelerated inner loops for the decode pipeline.
//!
//! Uses archmage for safe AVX2+FMA dispatch on x86-64 and NEON on aarch64.
//! Falls back to scalar automatically when SIMD is unavailable.

use alloc::vec::Vec;

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use alloc::vec;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use archmage::prelude::*;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use magetypes::simd::f32x8;

// ── Normalize ────────────────────────────────────────────────────────────

/// Normalize f32 sensor data: `(sample - black) * inv_range`, clamped to [0, 1].
///
/// All pixels share the same black/inv_range. This is the fast path for
/// non-CFA data (cpp > 1) or when all CFA channels have identical levels.
#[allow(dead_code)]
pub fn normalize_uniform(data: &[f32], black: f32, inv_range: f32) -> Vec<f32> {
    #[cfg(target_arch = "x86_64")]
    if let Some(token) = X64V3Token::summon() {
        return normalize_uniform_avx2(token, data, black, inv_range);
    }
    #[cfg(target_arch = "aarch64")]
    if let Some(token) = NeonToken::summon() {
        return normalize_uniform_neon(token, data, black, inv_range);
    }
    data.iter()
        .map(|&s| ((s - black) * inv_range).clamp(0.0, 1.0))
        .collect()
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn normalize_uniform_avx2(token: X64V3Token, data: &[f32], black: f32, inv_range: f32) -> Vec<f32> {
    normalize_uniform_inner(token, data, black, inv_range)
}

#[cfg(target_arch = "x86_64")]
#[rite]
fn normalize_uniform_inner(
    token: X64V3Token,
    data: &[f32],
    black: f32,
    inv_range: f32,
) -> Vec<f32> {
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

#[cfg(target_arch = "aarch64")]
#[arcane]
fn normalize_uniform_neon(token: NeonToken, data: &[f32], black: f32, inv_range: f32) -> Vec<f32> {
    normalize_uniform_inner_neon(token, data, black, inv_range)
}

#[cfg(target_arch = "aarch64")]
#[rite]
fn normalize_uniform_inner_neon(
    token: NeonToken,
    data: &[f32],
    black: f32,
    inv_range: f32,
) -> Vec<f32> {
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
