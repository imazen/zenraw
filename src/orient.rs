//! EXIF orientation transforms for decoded images.
//!
//! Applies the rotation/flip specified by the EXIF orientation tag
//! so the output matches display orientation. After applying, the
//! orientation value should be recorded as 1 (Normal).

use alloc::vec;
use alloc::vec::Vec;

/// Apply EXIF orientation transform to interleaved RGB f32 pixel data.
///
/// Takes ownership to enable in-place transforms for flips/rotations
/// that don't change dimensions. Returns `(pixels, new_width, new_height)`.
///
/// EXIF orientation values:
/// - 1: Normal (identity)
/// - 2: Horizontal flip
/// - 3: Rotate 180°
/// - 4: Vertical flip
/// - 5: Transpose
/// - 6: Rotate 90° CW
/// - 7: Transverse
/// - 8: Rotate 270° CW
pub(crate) fn apply_orientation(
    rgb: Vec<f32>,
    width: usize,
    height: usize,
    orientation: u16,
) -> (Vec<f32>, usize, usize) {
    apply_orientation_stop(rgb, width, height, orientation, &how_far::NoPulse)
        .expect("NoPulse cannot cancel")
}

/// [`apply_orientation`] with cooperative cancellation — each O(pixels)
/// permutation checks `pulse` between row or pixel batches. Cancellation is
/// returned directly so a partially-permuted buffer cannot escape as success.
pub(crate) fn apply_orientation_stop(
    mut rgb: Vec<f32>,
    width: usize,
    height: usize,
    orientation: u16,
    pulse: &dyn how_far::Pulse,
) -> Result<(Vec<f32>, usize, usize), enough::StopReason> {
    crate::progress::total(
        pulse,
        match orientation {
            2 => height,
            3 => width * height / 2,
            4 => height / 2,
            5..=8 => width,
            _ => 0,
        },
    );
    // Entry check: bounds the gap attributed to the first in-helper check.
    pulse.check()?;
    let w = width;
    let h = height;
    Ok(match orientation {
        0 | 1 => (rgb, w, h),
        2 => {
            flip_horizontal(&mut rgb, w, h, pulse)?;
            (rgb, w, h)
        }
        3 => {
            rotate_180(&mut rgb, w, h, pulse)?;
            (rgb, w, h)
        }
        4 => {
            flip_vertical(&mut rgb, w, h, pulse)?;
            (rgb, w, h)
        }
        // Orientations 5-8 swap width and height.
        // Display image: new_width = h, new_height = w.
        5 => {
            // Transpose: display(dr,dc) ← src(dc, dr)
            let out = remap(&rgb, w, h, w, pulse, |dr, dc| (dc, dr))?;
            (out, h, w)
        }
        6 => {
            // Rotate 90° CW: display(dr,dc) ← src(h-1-dc, dr)
            let out = remap(&rgb, w, h, w, pulse, |dr, dc| (h - 1 - dc, dr))?;
            (out, h, w)
        }
        7 => {
            // Transverse: display(dr,dc) ← src(h-1-dc, w-1-dr)
            let out = remap(&rgb, w, h, w, pulse, |dr, dc| (h - 1 - dc, w - 1 - dr))?;
            (out, h, w)
        }
        8 => {
            // Rotate 270° CW: display(dr,dc) ← src(dc, w-1-dr)
            let out = remap(&rgb, w, h, w, pulse, |dr, dc| (dc, w - 1 - dr))?;
            (out, h, w)
        }
        _ => (rgb, w, h),
    })
}

/// Apply an EXIF orientation to a tightly-packed pixel buffer of arbitrary
/// bytes-per-pixel, moving whole `bpp`-sized pixels (never touching their
/// contents — so it is format-, channel-, and bit-depth-agnostic and
/// bit-exact for any layout).
///
/// `data` is `width * height * bpp` bytes, row-major, tight stride.
/// Returns `(out, new_width, new_height)`; width and height swap for the
/// four axis-swapping orientations (5–8), matching [`apply_orientation`].
///
/// This is the byte-level analogue of [`apply_orientation`] (which is f32-RGB
/// only). The adapter's strided equivalent is `apply_orientation_pixels`.
/// This tightly-packed helper also provides a reference for permutation tests.
///
/// Because orientation is a pure pixel permutation (no resampling, no
/// arithmetic), baking on the final integer/float buffer is identical to
/// baking on the pre-gamma f32 buffer — the pixels are preserved exactly.
pub(crate) fn apply_orientation_bytes(
    data: &[u8],
    width: usize,
    height: usize,
    bpp: usize,
    orientation: u16,
) -> (Vec<u8>, usize, usize) {
    debug_assert_eq!(data.len(), width * height * bpp);
    match orientation {
        // Identity / out-of-range: copy through unchanged.
        0 | 1 => (data.to_vec(), width, height),
        2 => (
            remap_bytes(data, width, height, bpp, width, height, |dr, dc| {
                (dr, width - 1 - dc)
            }),
            width,
            height,
        ),
        3 => (
            remap_bytes(data, width, height, bpp, width, height, |dr, dc| {
                (height - 1 - dr, width - 1 - dc)
            }),
            width,
            height,
        ),
        4 => (
            remap_bytes(data, width, height, bpp, width, height, |dr, dc| {
                (height - 1 - dr, dc)
            }),
            width,
            height,
        ),
        // Orientations 5–8 swap width and height: new dims = (h, w).
        5 => (
            remap_bytes(data, width, height, bpp, height, width, |dr, dc| (dc, dr)),
            height,
            width,
        ),
        6 => (
            remap_bytes(data, width, height, bpp, height, width, |dr, dc| {
                (height - 1 - dc, dr)
            }),
            height,
            width,
        ),
        7 => (
            remap_bytes(data, width, height, bpp, height, width, |dr, dc| {
                (height - 1 - dc, width - 1 - dr)
            }),
            height,
            width,
        ),
        8 => (
            remap_bytes(data, width, height, bpp, height, width, |dr, dc| {
                (dc, width - 1 - dr)
            }),
            height,
            width,
        ),
        _ => (data.to_vec(), width, height),
    }
}

/// Byte-level pixel remap: `map(dst_row, dst_col) -> (src_row, src_col)`.
///
/// Copies whole `bpp`-sized pixels from `src` (dims `src_w × _`, tight stride)
/// to a fresh `new_w × new_h` buffer. The source coordinate the closure returns
/// is always in-bounds for the orientations above.
fn remap_bytes(
    src: &[u8],
    src_w: usize,
    _src_h: usize,
    bpp: usize,
    new_w: usize,
    new_h: usize,
    map: impl Fn(usize, usize) -> (usize, usize),
) -> Vec<u8> {
    let mut out = vec![0u8; new_w * new_h * bpp];
    for dr in 0..new_h {
        for dc in 0..new_w {
            let (sr, sc) = map(dr, dc);
            let si = (sr * src_w + sc) * bpp;
            let di = (dr * new_w + dc) * bpp;
            out[di..di + bpp].copy_from_slice(&src[si..si + bpp]);
        }
    }
    out
}

/// Flip horizontally (mirror left↔right) in place.
fn flip_horizontal(
    rgb: &mut [f32],
    width: usize,
    height: usize,
    pulse: &dyn how_far::Pulse,
) -> Result<(), enough::StopReason> {
    let mut pace = pulse.paced(64);
    for r in 0..height {
        for c in 0..width / 2 {
            let l = (r * width + c) * 3;
            let ri = (r * width + (width - 1 - c)) * 3;
            for ch in 0..3 {
                rgb.swap(l + ch, ri + ch);
            }
        }
        pace.step(1)?;
    }
    pace.finish()?;
    Ok(())
}

/// Rotate 180° in place (reverse pixel order).
fn rotate_180(
    rgb: &mut [f32],
    width: usize,
    height: usize,
    pulse: &dyn how_far::Pulse,
) -> Result<(), enough::StopReason> {
    let n = width * height;
    for batch_start in (0..n / 2).step_by(1 << 18) {
        pulse.check()?;
        for i in batch_start..(n / 2).min(batch_start + (1 << 18)) {
            let j = n - 1 - i;
            let a = i * 3;
            let b = j * 3;
            for ch in 0..3 {
                rgb.swap(a + ch, b + ch);
            }
        }
        pulse.advance((n / 2 - batch_start).min(1 << 18) as u64);
    }
    Ok(())
}

/// Flip vertically (mirror top↔bottom) in place.
fn flip_vertical(
    rgb: &mut [f32],
    width: usize,
    height: usize,
    pulse: &dyn how_far::Pulse,
) -> Result<(), enough::StopReason> {
    let mut pace = pulse.paced(64);
    let row_len = width * 3;
    for r in 0..height / 2 {
        let top = r * row_len;
        let bot = (height - 1 - r) * row_len;
        for i in 0..row_len {
            rgb.swap(top + i, bot + i);
        }
        pace.step(1)?;
    }
    pace.finish()?;
    Ok(())
}

/// Remap pixels from source to a new buffer with different dimensions.
///
/// `map(dst_row, dst_col) -> (src_row, src_col)`
///
/// Display dimensions: `new_w = old_height`, `new_h = old_width`.
fn remap(
    rgb: &[f32],
    src_w: usize,
    new_w: usize,
    new_h: usize,
    pulse: &dyn how_far::Pulse,
    map: impl Fn(usize, usize) -> (usize, usize),
) -> Result<Vec<f32>, enough::StopReason> {
    let mut pace = pulse.paced(64);
    let mut out = vec![0.0f32; new_w * new_h * 3];
    for dr in 0..new_h {
        for dc in 0..new_w {
            let (sr, sc) = map(dr, dc);
            let si = (sr * src_w + sc) * 3;
            let di = (dr * new_w + dc) * 3;
            out[di] = rgb[si];
            out[di + 1] = rgb[si + 1];
            out[di + 2] = rgb[si + 2];
        }
        pace.step(1)?;
    }
    pace.finish()?;
    Ok(out)
}

/// Apply an adapter orientation directly to rows, preserving padding in the
/// source and avoiding an intermediate contiguous full-image copy.
#[cfg(feature = "zencodec")]
pub(crate) fn apply_orientation_pixels(
    src: &zenpixels::PixelSlice<'_>,
    orientation: u16,
    alloc_pref: crate::alloc_util::AllocPref,
    pulse: &dyn how_far::Pulse,
) -> crate::Result<(Vec<u8>, usize, usize)> {
    let w = src.width() as usize;
    let h = src.rows() as usize;
    let dims = if (5..=8).contains(&orientation) {
        (h, w)
    } else {
        (w, h)
    };
    crate::progress::total(pulse, dims.1);
    match orientation {
        2 => remap_pixels(src, dims, alloc_pref, pulse, |r, c| (r, w - 1 - c)),
        3 => remap_pixels(src, dims, alloc_pref, pulse, |r, c| (h - 1 - r, w - 1 - c)),
        4 => remap_pixels(src, dims, alloc_pref, pulse, |r, c| (h - 1 - r, c)),
        5 => remap_pixels(src, dims, alloc_pref, pulse, |r, c| (c, r)),
        6 => remap_pixels(src, dims, alloc_pref, pulse, |r, c| (h - 1 - c, r)),
        7 => remap_pixels(src, dims, alloc_pref, pulse, |r, c| (h - 1 - c, w - 1 - r)),
        8 => remap_pixels(src, dims, alloc_pref, pulse, |r, c| (c, w - 1 - r)),
        _ => remap_pixels(src, dims, alloc_pref, pulse, |r, c| (r, c)),
    }
}

#[cfg(feature = "zencodec")]
fn remap_pixels(
    src: &zenpixels::PixelSlice<'_>,
    (width, height): (usize, usize),
    alloc_pref: crate::alloc_util::AllocPref,
    pulse: &dyn how_far::Pulse,
    map: impl Fn(usize, usize) -> (usize, usize),
) -> crate::Result<(Vec<u8>, usize, usize)> {
    let bpp = src.descriptor().bytes_per_pixel();
    let mut out = crate::alloc_util::alloc_filled(alloc_pref, true, 0u8, width * height * bpp)?;
    let mut pace = pulse.paced(64);
    for r in 0..height {
        for c in 0..width {
            let (sr, sc) = map(r, c);
            let row = src.row(sr as u32);
            let dst = (r * width + c) * bpp;
            out[dst..dst + bpp].copy_from_slice(&row[sc * bpp..(sc + 1) * bpp]);
        }
        pace.step(1)
            .map_err(|e| whereat::at!(crate::RawError::from(e)))?;
    }
    pace.finish()
        .map_err(|e| whereat::at!(crate::RawError::from(e)))?;
    Ok((out, width, height))
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "zencodec")]
    #[test]
    fn observed_orientation_matches_byte_permutations_for_strided_rows() {
        use how_far::{Complete, Outcome, Total};
        use how_far_along::{Phase, PulseTree};
        for descriptor in [
            zenpixels::PixelDescriptor::RGB16_SRGB,
            zenpixels::PixelDescriptor::RGBF32_LINEAR,
        ] {
            let (w, h) = (5usize, 3usize);
            let bpp = descriptor.bytes_per_pixel();
            let stride = 8 * bpp;
            let mut storage = vec![0xa5a5_a5a5u32; stride * h / 4];
            let bytes = bytemuck::cast_slice_mut::<u32, u8>(&mut storage);
            let mut tight = Vec::new();
            for r in 0..h {
                for c in 0..w * bpp {
                    let v = (r * w * bpp + c) as u8;
                    bytes[r * stride + c] = v;
                    tight.push(v);
                }
            }
            let src =
                zenpixels::PixelSlice::new(bytes, w as u32, h as u32, stride, descriptor).unwrap();
            for orientation in 1..=8 {
                let pulse =
                    PulseTree::new(Phase::new("orient", Total::Unknown), how_far::Unstoppable);
                let expected = super::apply_orientation_bytes(&tight, w, h, bpp, orientation);
                let actual = super::apply_orientation_pixels(
                    &src,
                    orientation,
                    crate::alloc_util::AllocPref::CodecDefault,
                    &pulse,
                )
                .unwrap();
                assert_eq!(actual, expected);
                let snap = pulse.observer().snapshot();
                assert_eq!(snap.total, Total::Exact(actual.2 as u64));
                assert_eq!(snap.completed, actual.2 as u64);
                pulse.complete_as(Outcome::Succeeded);
            }
        }
    }

    extern crate std;

    use super::*;

    /// Make a 3x2 test image with identifiable pixels.
    /// Pixel at (row, col) has value (row*10 + col) in all channels.
    fn make_test_image() -> (Vec<f32>, usize, usize) {
        let w = 3;
        let h = 2;
        // Row 0: [0,0,0], [1,1,1], [2,2,2]
        // Row 1: [10,10,10], [11,11,11], [12,12,12]
        let mut rgb = Vec::with_capacity(w * h * 3);
        for r in 0..h {
            for c in 0..w {
                let v = (r * 10 + c) as f32;
                rgb.extend_from_slice(&[v, v, v]);
            }
        }
        (rgb, w, h)
    }

    fn pixel_at(rgb: &[f32], width: usize, row: usize, col: usize) -> f32 {
        rgb[(row * width + col) * 3]
    }

    #[test]
    fn orient_1_identity() {
        let (rgb, w, h) = make_test_image();
        let (out, nw, nh) = apply_orientation(rgb, w, h, 1);
        assert_eq!((nw, nh), (3, 2));
        assert_eq!(pixel_at(&out, nw, 0, 0), 0.0);
        assert_eq!(pixel_at(&out, nw, 0, 2), 2.0);
        assert_eq!(pixel_at(&out, nw, 1, 0), 10.0);
    }

    #[test]
    fn orient_2_flip_horizontal() {
        let (rgb, w, h) = make_test_image();
        let (out, nw, nh) = apply_orientation(rgb, w, h, 2);
        assert_eq!((nw, nh), (3, 2));
        // Row 0 reversed: [2,1,0]
        assert_eq!(pixel_at(&out, nw, 0, 0), 2.0);
        assert_eq!(pixel_at(&out, nw, 0, 2), 0.0);
        // Row 1 reversed: [12,11,10]
        assert_eq!(pixel_at(&out, nw, 1, 0), 12.0);
        assert_eq!(pixel_at(&out, nw, 1, 2), 10.0);
    }

    #[test]
    fn orient_3_rotate_180() {
        let (rgb, w, h) = make_test_image();
        let (out, nw, nh) = apply_orientation(rgb, w, h, 3);
        assert_eq!((nw, nh), (3, 2));
        // Bottom-right becomes top-left
        assert_eq!(pixel_at(&out, nw, 0, 0), 12.0);
        assert_eq!(pixel_at(&out, nw, 1, 2), 0.0);
    }

    #[test]
    fn orient_4_flip_vertical() {
        let (rgb, w, h) = make_test_image();
        let (out, nw, nh) = apply_orientation(rgb, w, h, 4);
        assert_eq!((nw, nh), (3, 2));
        // Rows swapped
        assert_eq!(pixel_at(&out, nw, 0, 0), 10.0);
        assert_eq!(pixel_at(&out, nw, 1, 0), 0.0);
    }

    #[test]
    fn orient_5_transpose() {
        let (rgb, w, h) = make_test_image();
        let (out, nw, nh) = apply_orientation(rgb, w, h, 5);
        // 3x2 → 2x3 (new_width = old_height = 2, new_height = old_width = 3)
        assert_eq!((nw, nh), (2, 3));
        // (0,0) ← src(0,0) = 0
        assert_eq!(pixel_at(&out, nw, 0, 0), 0.0);
        // (0,1) ← src(1,0) = 10
        assert_eq!(pixel_at(&out, nw, 0, 1), 10.0);
        // (1,0) ← src(0,1) = 1
        assert_eq!(pixel_at(&out, nw, 1, 0), 1.0);
        // (2,1) ← src(1,2) = 12
        assert_eq!(pixel_at(&out, nw, 2, 1), 12.0);
    }

    #[test]
    fn orient_6_rotate_90_cw() {
        let (rgb, w, h) = make_test_image();
        let (out, nw, nh) = apply_orientation(rgb, w, h, 6);
        assert_eq!((nw, nh), (2, 3));
        // (0,0) ← src(h-1-0, 0) = src(1,0) = 10
        assert_eq!(pixel_at(&out, nw, 0, 0), 10.0);
        // (0,1) ← src(h-1-1, 0) = src(0,0) = 0
        assert_eq!(pixel_at(&out, nw, 0, 1), 0.0);
        // (2,0) ← src(h-1-0, 2) = src(1,2) = 12
        assert_eq!(pixel_at(&out, nw, 2, 0), 12.0);
        // (2,1) ← src(h-1-1, 2) = src(0,2) = 2
        assert_eq!(pixel_at(&out, nw, 2, 1), 2.0);
    }

    #[test]
    fn orient_7_transverse() {
        let (rgb, w, h) = make_test_image();
        let (out, nw, nh) = apply_orientation(rgb, w, h, 7);
        assert_eq!((nw, nh), (2, 3));
        // (0,0) ← src(h-1-0, w-1-0) = src(1,2) = 12
        assert_eq!(pixel_at(&out, nw, 0, 0), 12.0);
        // (2,1) ← src(h-1-1, w-1-2) = src(0,0) = 0
        assert_eq!(pixel_at(&out, nw, 2, 1), 0.0);
    }

    #[test]
    fn orient_8_rotate_270_cw() {
        let (rgb, w, h) = make_test_image();
        let (out, nw, nh) = apply_orientation(rgb, w, h, 8);
        assert_eq!((nw, nh), (2, 3));
        // (0,0) ← src(0, w-1-0) = src(0,2) = 2
        assert_eq!(pixel_at(&out, nw, 0, 0), 2.0);
        // (2,1) ← src(2, w-1-1) = src(2,1) = ... wait, src is 3x2 so row 2 doesn't exist
        // Let me recalculate. src is w=3, h=2.
        // orient 8: display(dr,dc) ← src(dc, w-1-dr)
        // (0,0) ← src(0, 2) = 2
        assert_eq!(pixel_at(&out, nw, 0, 0), 2.0);
        // (0,1) ← src(1, 2) = 12
        assert_eq!(pixel_at(&out, nw, 0, 1), 12.0);
        // (2,0) ← src(0, 0) = 0
        assert_eq!(pixel_at(&out, nw, 2, 0), 0.0);
        // (2,1) ← src(1, 0) = 10
        assert_eq!(pixel_at(&out, nw, 2, 1), 10.0);
    }

    #[test]
    fn orient_roundtrips() {
        // Applying orientation then its inverse should get back to original.
        // 6 (90° CW) then 8 (270° CW) = identity
        let (rgb, w, h) = make_test_image();
        let original = rgb.clone();
        let (rotated, nw, nh) = apply_orientation(rgb, w, h, 6);
        let (back, fw, fh) = apply_orientation(rotated, nw, nh, 8);
        assert_eq!((fw, fh), (w, h));
        assert_eq!(original, back);
    }

    // ── Byte-level baker (apply_orientation_bytes) ──────────────────────
    //
    // These pin the SACRED-pixel behavior of the multi-byte baker the zencodec
    // adapter uses for arbitrary orientations on the decoded RGB16/RGBF32
    // buffer. Each pixel is a distinct multi-byte tag so transposes and flips
    // are verified bit-for-bit (not just by a single channel).

    /// Make a 3×2 image with `bpp`-byte pixels. Pixel (r,c) is `bpp` bytes all
    /// equal to a unique id `(r*16 + c) + 1` (never 0, so a stray zero is
    /// visible). Returns `(bytes, w, h, bpp)`.
    fn make_byte_image(bpp: usize) -> (Vec<u8>, usize, usize, usize) {
        let (w, h) = (3usize, 2usize);
        let mut data = Vec::with_capacity(w * h * bpp);
        for r in 0..h {
            for c in 0..w {
                let id = (r * 16 + c) as u8 + 1;
                for _ in 0..bpp {
                    data.push(id);
                }
            }
        }
        (data, w, h, bpp)
    }

    /// The `bpp`-byte pixel id at (row, col) in a tightly-packed buffer.
    fn byte_pixel_id(data: &[u8], width: usize, bpp: usize, row: usize, col: usize) -> u8 {
        let i = (row * width + col) * bpp;
        let id = data[i];
        // All bytes of a pixel must be identical — proves whole-pixel moves.
        for k in 0..bpp {
            assert_eq!(data[i + k], id, "pixel ({row},{col}) byte {k} mismatch");
        }
        id
    }

    #[test]
    fn bytes_identity_copies_through() {
        for bpp in [6usize, 12] {
            let (data, w, h, _) = make_byte_image(bpp);
            let (out, nw, nh) = apply_orientation_bytes(&data, w, h, bpp, 1);
            assert_eq!((nw, nh), (w, h));
            assert_eq!(out, data, "bpp={bpp}: identity must be a verbatim copy");
        }
    }

    #[test]
    fn bytes_flip_horizontal_oracle() {
        // RGB16-shaped (6 bytes/pixel).
        let (data, w, h, bpp) = make_byte_image(6);
        let (out, nw, nh) = apply_orientation_bytes(&data, w, h, bpp, 2);
        assert_eq!((nw, nh), (3, 2));
        // ids: row0 = 1,2,3 ; row1 = 17,18,19. After H-flip each row reverses.
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 0), 3);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 2), 1);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 1, 0), 19);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 1, 2), 17);
    }

    #[test]
    fn bytes_rotate180_oracle() {
        let (data, w, h, bpp) = make_byte_image(6);
        let (out, nw, nh) = apply_orientation_bytes(&data, w, h, bpp, 3);
        assert_eq!((nw, nh), (3, 2));
        // bottom-right (19) → top-left; top-left (1) → bottom-right.
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 0), 19);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 1, 2), 1);
    }

    #[test]
    fn bytes_flip_vertical_oracle() {
        let (data, w, h, bpp) = make_byte_image(6);
        let (out, nw, nh) = apply_orientation_bytes(&data, w, h, bpp, 4);
        assert_eq!((nw, nh), (3, 2));
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 0), 17); // row1 ↑
        assert_eq!(byte_pixel_id(&out, nw, bpp, 1, 0), 1); // row0 ↓
    }

    #[test]
    fn bytes_transpose_oracle_rgb16() {
        // EXIF 5 (Transpose). 3×2 → 2×3. 6 bytes/pixel (RGB16).
        let (data, w, h, bpp) = make_byte_image(6);
        let (out, nw, nh) = apply_orientation_bytes(&data, w, h, bpp, 5);
        assert_eq!((nw, nh), (2, 3));
        // display(dr,dc) ← src(dc, dr): (0,0)←(0,0)=1, (0,1)←(1,0)=17,
        // (1,0)←(0,1)=2, (2,1)←(1,2)=19.
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 0), 1);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 1), 17);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 1, 0), 2);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 2, 1), 19);
    }

    #[test]
    fn bytes_rotate90_oracle_rgbf32() {
        // EXIF 6 (Rotate 90° CW). 3×2 → 2×3. 12 bytes/pixel (RGBF32 width).
        let (data, w, h, bpp) = make_byte_image(12);
        let (out, nw, nh) = apply_orientation_bytes(&data, w, h, bpp, 6);
        assert_eq!((nw, nh), (2, 3));
        // display(dr,dc) ← src(h-1-dc, dr):
        // (0,0)←(1,0)=17, (0,1)←(0,0)=1, (2,0)←(1,2)=19, (2,1)←(0,2)=3.
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 0), 17);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 1), 1);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 2, 0), 19);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 2, 1), 3);
    }

    #[test]
    fn bytes_transverse_oracle() {
        // EXIF 7. 3×2 → 2×3.
        let (data, w, h, bpp) = make_byte_image(6);
        let (out, nw, nh) = apply_orientation_bytes(&data, w, h, bpp, 7);
        assert_eq!((nw, nh), (2, 3));
        // display(dr,dc) ← src(h-1-dc, w-1-dr):
        // (0,0)←(1,2)=19 ; (2,1)←(0,0)=1.
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 0), 19);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 2, 1), 1);
    }

    #[test]
    fn bytes_rotate270_oracle() {
        // EXIF 8. 3×2 → 2×3.
        let (data, w, h, bpp) = make_byte_image(6);
        let (out, nw, nh) = apply_orientation_bytes(&data, w, h, bpp, 8);
        assert_eq!((nw, nh), (2, 3));
        // display(dr,dc) ← src(dc, w-1-dr):
        // (0,0)←(0,2)=3 ; (0,1)←(1,2)=19 ; (2,0)←(0,0)=1 ; (2,1)←(1,0)=17.
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 0), 3);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 0, 1), 19);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 2, 0), 1);
        assert_eq!(byte_pixel_id(&out, nw, bpp, 2, 1), 17);
    }

    #[test]
    fn bytes_matches_f32_baker_for_all_orientations() {
        // The byte baker must agree element-for-element with the f32 RGB baker
        // (the native decode path) for every EXIF orientation — proving the
        // adapter's bake on the final buffer is identical to the native bake.
        // Encode each f32 channel value as a 4-byte little-endian group so the
        // 12-byte "pixel" carries the full RGB triple.
        for orient in 1..=8u16 {
            let (rgb, w, h) = make_test_image(); // 3×2 f32 RGB
            let bytes: Vec<u8> = rgb.iter().flat_map(|v| v.to_le_bytes()).collect();
            let bpp = 12; // 3 channels × 4 bytes

            let (f_out, fw, fh) = apply_orientation(rgb, w, h, orient);
            let (b_out, bw, bh) = apply_orientation_bytes(&bytes, w, h, bpp, orient);

            assert_eq!((bw, bh), (fw, fh), "orient {orient}: dims differ");
            let f_bytes: Vec<u8> = f_out.iter().flat_map(|v| v.to_le_bytes()).collect();
            assert_eq!(
                b_out, f_bytes,
                "orient {orient}: byte baker diverges from f32 baker"
            );
        }
    }
}
