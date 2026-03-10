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
    mut rgb: Vec<f32>,
    width: usize,
    height: usize,
    orientation: u16,
) -> (Vec<f32>, usize, usize) {
    let w = width;
    let h = height;
    match orientation {
        0 | 1 => (rgb, w, h),
        2 => {
            flip_horizontal(&mut rgb, w, h);
            (rgb, w, h)
        }
        3 => {
            rotate_180(&mut rgb, w, h);
            (rgb, w, h)
        }
        4 => {
            flip_vertical(&mut rgb, w, h);
            (rgb, w, h)
        }
        // Orientations 5-8 swap width and height.
        // Display image: new_width = h, new_height = w.
        5 => {
            // Transpose: display(dr,dc) ← src(dc, dr)
            let out = remap(&rgb, w, h, w, |dr, dc| (dc, dr));
            (out, h, w)
        }
        6 => {
            // Rotate 90° CW: display(dr,dc) ← src(h-1-dc, dr)
            let out = remap(&rgb, w, h, w, |dr, dc| (h - 1 - dc, dr));
            (out, h, w)
        }
        7 => {
            // Transverse: display(dr,dc) ← src(h-1-dc, w-1-dr)
            let out = remap(&rgb, w, h, w, |dr, dc| (h - 1 - dc, w - 1 - dr));
            (out, h, w)
        }
        8 => {
            // Rotate 270° CW: display(dr,dc) ← src(dc, w-1-dr)
            let out = remap(&rgb, w, h, w, |dr, dc| (dc, w - 1 - dr));
            (out, h, w)
        }
        _ => (rgb, w, h),
    }
}

/// Flip horizontally (mirror left↔right) in place.
fn flip_horizontal(rgb: &mut [f32], width: usize, height: usize) {
    for r in 0..height {
        for c in 0..width / 2 {
            let l = (r * width + c) * 3;
            let ri = (r * width + (width - 1 - c)) * 3;
            for ch in 0..3 {
                rgb.swap(l + ch, ri + ch);
            }
        }
    }
}

/// Rotate 180° in place (reverse pixel order).
fn rotate_180(rgb: &mut [f32], width: usize, height: usize) {
    let n = width * height;
    for i in 0..n / 2 {
        let j = n - 1 - i;
        let a = i * 3;
        let b = j * 3;
        for ch in 0..3 {
            rgb.swap(a + ch, b + ch);
        }
    }
}

/// Flip vertically (mirror top↔bottom) in place.
fn flip_vertical(rgb: &mut [f32], width: usize, height: usize) {
    let row_len = width * 3;
    for r in 0..height / 2 {
        let top = r * row_len;
        let bot = (height - 1 - r) * row_len;
        for i in 0..row_len {
            rgb.swap(top + i, bot + i);
        }
    }
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
    map: impl Fn(usize, usize) -> (usize, usize),
) -> Vec<f32> {
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
    }
    out
}

#[cfg(test)]
mod tests {
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
}
