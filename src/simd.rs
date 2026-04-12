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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_uniform_basic() {
        let data = [100.0, 200.0, 300.0, 400.0, 500.0];
        let black = 100.0;
        let inv_range = 1.0 / 400.0; // white = 500
        let result = normalize_uniform(&data, black, inv_range);
        assert_eq!(result.len(), 5);
        assert!((result[0] - 0.0).abs() < 1e-6); // at black
        assert!((result[1] - 0.25).abs() < 1e-6);
        assert!((result[2] - 0.5).abs() < 1e-6);
        assert!((result[3] - 0.75).abs() < 1e-6);
        assert!((result[4] - 1.0).abs() < 1e-6); // at white
    }

    #[test]
    fn normalize_uniform_clamps() {
        let data = [-10.0, 0.0, 50.0, 100.0, 200.0];
        let black = 0.0;
        let inv_range = 1.0 / 100.0;
        let result = normalize_uniform(&data, black, inv_range);
        assert_eq!(result[0], 0.0); // below black → clamped to 0
        assert_eq!(result[4], 1.0); // above white → clamped to 1
    }

    #[test]
    fn normalize_uniform_many_elements() {
        // Enough elements to exercise SIMD (8-wide) + scalar tail
        let data: Vec<f32> = (0..35).map(|i| i as f32 * 10.0).collect();
        let black = 0.0;
        let inv_range = 1.0 / 340.0;
        let result = normalize_uniform(&data, black, inv_range);
        assert_eq!(result.len(), 35);
        for &v in &result {
            assert!((0.0..=1.0).contains(&v), "out of range: {v}");
        }
        assert!((result[0] - 0.0).abs() < 1e-6);
        assert!((result[34] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_uniform_empty() {
        let result = normalize_uniform(&[], 0.0, 1.0);
        assert!(result.is_empty());
    }

    #[test]
    fn extract_rgb_cpp3() {
        let data: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let result = extract_rgb_from_cpp(&data, 2, 3);
        assert_eq!(result, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn extract_rgb_cpp4_drops_alpha() {
        let data: Vec<f32> = vec![1.0, 2.0, 3.0, 99.0, 4.0, 5.0, 6.0, 88.0];
        let result = extract_rgb_from_cpp(&data, 2, 4);
        assert_eq!(result, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn extract_rgb_cpp6() {
        let data: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let result = extract_rgb_from_cpp(&data, 1, 6);
        assert_eq!(result, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn extract_rgb_short_data_pads_zeros() {
        let data: Vec<f32> = vec![1.0]; // only 1 element but expects cpp=3
        let result = extract_rgb_from_cpp(&data, 1, 3);
        assert_eq!(result, vec![1.0, 0.0, 0.0]);
    }
}
