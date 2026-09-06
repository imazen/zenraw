//! SIMD-accelerated inner loops for the decode pipeline.
//!
//! Uses archmage/magetypes for cross-platform SIMD with runtime dispatch.
//! AVX2+FMA on x86-64, NEON on aarch64, WASM SIMD128, scalar fallback.

use alloc::vec;
use alloc::vec::Vec;

use archmage::prelude::*;
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "wasm32"
))]
use magetypes::simd::generic::f32x8 as GenericF32x8;

// ── Normalize ────────────────────────────────────────────────────────────

/// Normalize f32 sensor data: `(sample - black) * inv_range`, clamped to [0, 1].
///
/// All pixels share the same black/inv_range. This is the fast path for
/// non-CFA data (cpp > 1) or when all CFA channels have identical levels.
///
/// Allocates the output buffer infallibly (`vec![]`). The decode pipeline uses
/// [`normalize_uniform_fallible`], which honours the caller's
/// [`AllocPref`](crate::alloc_util::AllocPref).
#[allow(dead_code)]
pub fn normalize_uniform(data: &[f32], black: f32, inv_range: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; data.len()];
    incant!(
        normalize_uniform_into(data, black, inv_range, &mut out),
        [v3, neon, wasm128, scalar]
    );
    out
}

/// Like [`normalize_uniform`], but allocates the (untrusted-sized) output buffer
/// honoring the per-site [`AllocPref`](crate::alloc_util::AllocPref) — default
/// fallible. Output bytes are identical.
#[cfg(feature = "rawler")]
pub(crate) fn normalize_uniform_fallible(
    data: &[f32],
    black: f32,
    inv_range: f32,
    alloc_pref: crate::alloc_util::AllocPref,
) -> Result<Vec<f32>, whereat::At<crate::error::RawError>> {
    // Full normalized sensor buffer sized from the (untrusted) sensor dims →
    // default fallible.
    let mut out = crate::alloc_util::alloc_filled(alloc_pref, true, 0.0f32, data.len())?;
    incant!(
        normalize_uniform_into(data, black, inv_range, &mut out),
        [v3, neon, wasm128, scalar]
    );
    Ok(out)
}

/// Normalize `data` into the pre-allocated `out` (same length as `data`).
#[magetypes(v3, neon, wasm128, -scalar)]
fn normalize_uniform_into(token: Token, data: &[f32], black: f32, inv_range: f32, out: &mut [f32]) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;

    debug_assert_eq!(out.len(), data.len());
    let black_v = f32x8::splat(token, black);
    let inv_range_v = f32x8::splat(token, inv_range);
    let zero = f32x8::zero(token);
    let one = f32x8::splat(token, 1.0);

    let (src_chunks, src_tail) = f32x8::partition_slice(token, data);
    let (dst_chunks, dst_tail) = f32x8::partition_slice_mut(token, out);

    for (src, dst) in src_chunks.iter().zip(dst_chunks.iter_mut()) {
        let v = f32x8::load(token, src);
        let normalized = (v - black_v) * inv_range_v;
        // Ordered comparisons preserve signed zero and NaN payloads like
        // scalar f32::clamp; ISA min/max instructions have different rules.
        let lower = f32x8::blend(normalized.simd_lt(zero), zero, normalized);
        let clamped = f32x8::blend(lower.simd_gt(one), one, lower);
        clamped.store(dst);
    }

    for (s, d) in src_tail.iter().zip(dst_tail.iter_mut()) {
        *d = ((*s - black) * inv_range).clamp(0.0, 1.0);
    }
}

fn normalize_uniform_into_scalar(
    _token: ScalarToken,
    data: &[f32],
    black: f32,
    inv_range: f32,
    out: &mut [f32],
) {
    debug_assert_eq!(out.len(), data.len());
    for (src, dst) in data.iter().zip(out.iter_mut()) {
        *dst = ((*src - black) * inv_range).clamp(0.0, 1.0);
    }
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

/// Like [`extract_rgb_from_cpp`], but allocates the (untrusted-sized) output
/// `RGB f32` buffer honoring the per-site
/// [`AllocPref`](crate::alloc_util::AllocPref) — default fallible. Output bytes
/// are identical.
#[cfg(feature = "rawler")]
pub(crate) fn extract_rgb_from_cpp_fallible(
    data: &[f32],
    pixel_count: usize,
    cpp: usize,
    alloc_pref: crate::alloc_util::AllocPref,
) -> Result<Vec<f32>, whereat::At<crate::error::RawError>> {
    // Full-image RGB output sized from the (untrusted) sensor dims → default
    // fallible.
    let len = pixel_count.checked_mul(3).ok_or_else(|| {
        whereat::at!(crate::error::RawError::OutOfMemory(
            "RGB buffer size overflows usize".into()
        ))
    })?;
    let mut rgb = crate::alloc_util::vec_with_capacity(alloc_pref, true, len)?;
    if cpp == 3 && data.len() >= len {
        rgb.extend_from_slice(&data[..len]);
        return Ok(rgb);
    }
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
    Ok(rgb)
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

    #[cfg(all(
        feature = "_dev",
        any(
            target_arch = "aarch64",
            target_arch = "x86_64",
            target_arch = "wasm32"
        )
    ))]
    #[test]
    fn normalization_matches_scalar_bits_for_special_values() {
        #[cfg(not(target_arch = "wasm32"))]
        use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
        let values = [
            f32::NEG_INFINITY,
            -1.0,
            -0.0,
            0.0,
            0.5,
            1.0,
            f32::INFINITY,
            f32::NAN,
            f32::from_bits(0x7fc01234),
            f32::MIN_POSITIVE,
            f32::from_bits(1),
        ];
        let check = || {
            for len in 1..=33 {
                for offset in 0..values.len() {
                    let input: Vec<f32> = (0..len)
                        .map(|i| values[(i + offset) % values.len()])
                        .collect();
                    let expected: Vec<u32> = input
                        .iter()
                        .map(|&v| ((v - 0.0) * 1.0).clamp(0.0, 1.0).to_bits())
                        .collect();
                    let actual: Vec<u32> = normalize_uniform(&input, 0.0, 1.0)
                        .iter()
                        .map(|v| v.to_bits())
                        .collect();
                    assert_eq!(actual, expected, "len={len}, offset={offset}");
                }
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| check());
            assert!(report.warnings.is_empty(), "{:?}", report.warnings);
            assert!(report.permutations_run >= 2);
        }
        #[cfg(target_arch = "wasm32")]
        {
            assert!(Wasm128Token::summon().is_some());
            check();
        }
    }

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
