//! Bayer pattern demosaicing algorithms.
//!
//! Converts single-channel Bayer CFA (color filter array) sensor data into
//! full RGB images. Implements bilinear interpolation as the baseline algorithm,
//! and Malvar-He-Cutler high-quality interpolation.

use alloc::vec;
use alloc::vec::Vec;

use archmage::prelude::*;

/// CFA color indices (matching rawloader convention).
const R: usize = 0;
const G: usize = 1;
const B: usize = 2;

/// Row/column → CFA colour-index lookup, abstracted over the backend's CFA
/// type so the Bayer kernels below compile without any decoder backend.
///
/// Implemented for [`BayerCfa`] (the rawler backend and the unit tests), for
/// [`CfaPattern`] (generic tiles), and — behind the `rawloader` feature — for
/// `rawloader::CFA` so the public [`demosaic_to_rgb_f32`] keeps its signature.
/// Colour indices follow the rawloader convention: 0=R, 1=G, 2=B, 3=E.
pub(crate) trait CfaColorAt {
    /// Colour index of the sensor site at (`row`, `col`).
    fn color_at(&self, row: usize, col: usize) -> usize;

    /// Size of the repeating CFA tile as `(width, height)`.
    ///
    /// `(2, 2)` for Bayer, `(6, 6)` for Fujifilm X-Trans, `(12, 12)` and
    /// `(2, 8)` for the handful of oddball sensors rawloader knows about.
    /// `(0, 0)` means the backend could not describe the pattern at all — the
    /// colour lookup is then meaningless and no demosaic kernel can be correct.
    ///
    /// The Bayer kernels below index a precomputed 2×2 tile with `row & 1` /
    /// `col & 1`, so they are only valid when this returns `(2, 2)`;
    /// [`demosaic_to_rgb_f32_fallible`] and friends dispatch on it.
    fn tile_dims(&self) -> (usize, usize);
}

#[cfg(feature = "rawloader")]
impl CfaColorAt for rawloader::CFA {
    #[inline]
    fn color_at(&self, row: usize, col: usize) -> usize {
        rawloader::CFA::color_at(self, row, col)
    }

    #[inline]
    fn tile_dims(&self) -> (usize, usize) {
        // `rawloader::CFA` exposes the repeating tile size directly; it is
        // `(0, 0)` for an empty/unparsed pattern (`CFA::is_valid()` is false).
        (self.width, self.height)
    }
}

/// A 2×2 Bayer CFA tile — the backend-independent CFA description the Bayer
/// demosaic kernels consume.
///
/// `color_at` is `tile[row & 1][col & 1]`, which is exactly what
/// `rawloader::CFA::color_at` (a 48×48 tiled table indexed with `% 48`)
/// returns for any 2×2 pattern, so output is bit-identical between the two
/// lookups for Bayer sensors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "rawler"), allow(dead_code))]
pub(crate) struct BayerCfa {
    tile: [[usize; 2]; 2],
}

#[cfg_attr(not(feature = "rawler"), allow(dead_code))]
impl BayerCfa {
    /// Build from a 2×2 tile of colour indices, `tile[row][col]`.
    pub(crate) const fn new(tile: [[usize; 2]; 2]) -> Self {
        Self { tile }
    }

    /// Parse a 4-character pattern string such as `"RGGB"` (row-major).
    /// Returns `None` for any other length or an unknown colour letter.
    #[cfg(test)]
    pub(crate) fn from_pattern_str(pattern: &str) -> Option<Self> {
        let bytes = pattern.as_bytes();
        if bytes.len() != 4 {
            return None;
        }
        let mut tile = [[0usize; 2]; 2];
        for (i, &b) in bytes.iter().enumerate() {
            tile[i / 2][i % 2] = match b {
                b'R' => R,
                b'G' => G,
                b'B' => B,
                b'E' => 3,
                _ => return None,
            };
        }
        Some(Self { tile })
    }
}

impl CfaColorAt for BayerCfa {
    #[inline]
    fn color_at(&self, row: usize, col: usize) -> usize {
        self.tile[row & 1][col & 1]
    }

    #[inline]
    fn tile_dims(&self) -> (usize, usize) {
        (2, 2)
    }
}

/// Demosaicing algorithm selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DemosaicMethod {
    /// Bilinear interpolation — fast, simple, slight color fringing.
    Bilinear,
    /// Malvar-He-Cutler (2004) gradient-corrected interpolation.
    /// Good balance of quality and speed.
    #[default]
    MalvarHeCutler,
}

/// Demosaic CFA data to RGB f32 pixels.
///
/// Input: single-channel f32 data (already normalized to 0.0..1.0),
/// width, height, and CFA pattern from rawloader.
///
/// Output: interleaved RGB f32 data with 3 components per pixel.
///
/// `method` selects between the two Bayer kernels and only applies to 2×2
/// patterns. A non-2×2 CFA (Fujifilm X-Trans is 6×6) is demosaiced with the
/// pattern-agnostic same-colour-neighbour kernel regardless of `method`,
/// because the Bayer kernels assume a 2×2 tile and would assign the wrong
/// colour to most sites.
///
/// Allocates the output buffer infallibly (`vec![]`). The
/// [`decode`](crate::decode) pipeline uses
/// [`demosaic_to_rgb_f32_fallible`] instead, which honours the caller's
/// [`AllocPref`](crate::alloc_util::AllocPref).
#[cfg(feature = "rawloader")]
pub fn demosaic_to_rgb_f32(
    data: &[f32],
    width: usize,
    height: usize,
    cfa: &rawloader::CFA,
    method: DemosaicMethod,
) -> Vec<f32> {
    demosaic_to_rgb_f32_infallible(data, width, height, cfa, method)
}

/// Backend-independent body of [`demosaic_to_rgb_f32`]: infallible `vec![]`
/// output allocation, any [`CfaColorAt`] lookup.
#[cfg_attr(not(any(feature = "rawloader", test)), allow(dead_code))]
fn demosaic_to_rgb_f32_infallible<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    cfa: &C,
    method: DemosaicMethod,
) -> Vec<f32> {
    let rgb = vec![0.0f32; width * height * 3];
    if cfa.tile_dims() == (2, 2) {
        match method {
            DemosaicMethod::Bilinear => demosaic_bilinear(data, width, height, cfa, rgb),
            DemosaicMethod::MalvarHeCutler => demosaic_malvar(data, width, height, cfa, rgb),
        }
    } else {
        // Non-Bayer tile (X-Trans 6×6, 12×12, 2×8, or an undescribed pattern):
        // the Bayer kernels index a precomputed 2×2 tile and would assign the
        // wrong colour to most sites. Fall back to the pattern-agnostic
        // same-colour-neighbour interpolation, which only ever asks the CFA.
        demosaic_xtrans_bilinear_into(data, width, height, cfa, rgb)
    }
}

/// Demosaic Bayer CFA data to RGB f32 pixels, honoring the per-site
/// [`AllocPref`](crate::alloc_util::AllocPref) for the (untrusted-sized) output
/// buffer.
///
/// This is the entry the [`decode`](crate::decode) pipeline uses: the output
/// `RGB f32` buffer is the largest intermediate and is sized from the sensor
/// dimensions the file claims, so it defaults to the fallible (`try_reserve`)
/// path. Behaviour and output bytes are identical to [`demosaic_to_rgb_f32`];
/// only the allocation strategy differs.
///
/// Generic over the CFA lookup so both decoder backends (and neither, for
/// unit tests) can drive it: the rawloader backend passes `&rawloader::CFA`,
/// the rawler backend a [`BayerCfa`].
#[cfg_attr(not(any(feature = "rawloader", feature = "rawler")), allow(dead_code))]
pub(crate) fn demosaic_to_rgb_f32_fallible<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    cfa: &C,
    method: DemosaicMethod,
    alloc_pref: crate::alloc_util::AllocPref,
) -> Result<Vec<f32>, whereat::At<crate::error::RawError>> {
    use whereat::at;
    let (tile_w, tile_h) = cfa.tile_dims();
    if tile_w == 0 || tile_h == 0 {
        // The backend could not describe the colour filter array at all, so
        // every `color_at` answer is a guess. Refusing beats emitting an image
        // whose colours are silently wrong.
        return Err(at!(crate::error::RawError::UnsupportedFeature(
            "RAW has no usable CFA pattern description — cannot demosaic".into()
        )));
    }
    let n = width
        .checked_mul(height)
        .and_then(|p| p.checked_mul(3))
        .ok_or_else(|| {
            at!(crate::error::RawError::OutOfMemory(
                "RGB f32 buffer size overflows usize".into()
            ))
        })?;
    // Full-image demosaic output sized from the (untrusted) sensor dims →
    // default fallible.
    let rgb = crate::alloc_util::alloc_filled(alloc_pref, true, 0.0f32, n)?;
    Ok(if (tile_w, tile_h) == (2, 2) {
        match method {
            DemosaicMethod::Bilinear => demosaic_bilinear(data, width, height, cfa, rgb),
            DemosaicMethod::MalvarHeCutler => demosaic_malvar(data, width, height, cfa, rgb),
        }
    } else {
        // Non-Bayer tile (X-Trans 6×6, 12×12, 2×8). The Bayer kernels index a
        // precomputed 2×2 tile with `row & 1` / `col & 1`, so on a 6×6 X-Trans
        // sensor they would read the wrong colour for most interior sites while
        // the clamped border path used the true `color_at` — the two halves of
        // the same image disagreeing. Route to the pattern-agnostic kernel,
        // which is what the rawler backend has always used for these sensors.
        demosaic_xtrans_bilinear_into(data, width, height, cfa, rgb)
    })
}

/// Bilinear interpolation demosaicing into a pre-allocated `rgb` buffer
/// (length `width * height * 3`).
///
/// Bayer-only: `rb_at_green_bilinear` assumes a green site's horizontal and
/// vertical neighbours are the two opposite colours, which holds for a 2×2 tile
/// and not for X-Trans. Callers dispatch on [`CfaColorAt::tile_dims`].
fn demosaic_bilinear<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    cfa: &C,
    mut rgb: Vec<f32>,
) -> Vec<f32> {
    debug_assert_eq!(rgb.len(), width * height * 3);
    debug_assert_eq!(cfa.tile_dims(), (2, 2), "Bayer kernel needs a 2×2 CFA tile");

    for row in 0..height {
        for col in 0..width {
            let color = cfa.color_at(row, col);
            let val = data[row * width + col];
            let out_idx = (row * width + col) * 3;

            match color {
                R => {
                    rgb[out_idx] = val;
                    rgb[out_idx + 1] = green_at_rb_bilinear(data, width, height, row, col);
                    rgb[out_idx + 2] = opposite_at_rb_bilinear(data, width, height, row, col, cfa);
                }
                G => {
                    let (r, b) = rb_at_green_bilinear(data, width, height, row, col, cfa);
                    rgb[out_idx] = r;
                    rgb[out_idx + 1] = val;
                    rgb[out_idx + 2] = b;
                }
                B => {
                    rgb[out_idx] = opposite_at_rb_bilinear(data, width, height, row, col, cfa);
                    rgb[out_idx + 1] = green_at_rb_bilinear(data, width, height, row, col);
                    rgb[out_idx + 2] = val;
                }
                _ => {
                    // E channel or unknown — treat as green
                    rgb[out_idx + 1] = val;
                }
            }
        }
    }

    rgb
}

/// Green at a red or blue site: average of 4 neighbors (cross pattern).
#[inline]
fn green_at_rb_bilinear(data: &[f32], width: usize, height: usize, row: usize, col: usize) -> f32 {
    let mut sum = 0.0f32;
    let mut count = 0u32;

    if row > 0 {
        sum += data[(row - 1) * width + col];
        count += 1;
    }
    if row + 1 < height {
        sum += data[(row + 1) * width + col];
        count += 1;
    }
    if col > 0 {
        sum += data[row * width + col - 1];
        count += 1;
    }
    if col + 1 < width {
        sum += data[row * width + col + 1];
        count += 1;
    }

    if count > 0 { sum / count as f32 } else { 0.0 }
}

/// R/B at a green site: average of 2 horizontal or 2 vertical neighbors.
#[inline]
fn rb_at_green_bilinear<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    row: usize,
    col: usize,
    cfa: &C,
) -> (f32, f32) {
    // Determine which neighbors are R and which are B.
    // At a green site, the horizontal neighbors are one color
    // and the vertical neighbors are the other.
    let h_color = if col > 0 {
        cfa.color_at(row, col - 1)
    } else {
        cfa.color_at(row, col + 1)
    };

    let h_avg = avg_horizontal(data, width, row, col);
    let v_avg = avg_vertical(data, height, width, row, col);

    if h_color == R {
        (h_avg, v_avg)
    } else {
        (v_avg, h_avg)
    }
}

/// Opposite color at an R or B site (R at B, or B at R): average of 4 diagonal neighbors.
#[inline]
fn opposite_at_rb_bilinear<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    row: usize,
    col: usize,
    cfa: &C,
) -> f32 {
    let _ = cfa; // CFA not needed for diagonal average
    let mut sum = 0.0f32;
    let mut count = 0u32;

    if row > 0 && col > 0 {
        sum += data[(row - 1) * width + col - 1];
        count += 1;
    }
    if row > 0 && col + 1 < width {
        sum += data[(row - 1) * width + col + 1];
        count += 1;
    }
    if row + 1 < height && col > 0 {
        sum += data[(row + 1) * width + col - 1];
        count += 1;
    }
    if row + 1 < height && col + 1 < width {
        sum += data[(row + 1) * width + col + 1];
        count += 1;
    }

    if count > 0 { sum / count as f32 } else { 0.0 }
}

#[inline]
fn avg_horizontal(data: &[f32], width: usize, row: usize, col: usize) -> f32 {
    let left = if col > 0 {
        data[row * width + col - 1]
    } else {
        0.0
    };
    let right = if col + 1 < width {
        data[row * width + col + 1]
    } else {
        0.0
    };
    let count = (col > 0) as u32 + (col + 1 < width) as u32;
    if count > 0 {
        (left + right) / count as f32
    } else {
        0.0
    }
}

#[inline]
fn avg_vertical(data: &[f32], height: usize, width: usize, row: usize, col: usize) -> f32 {
    let top = if row > 0 {
        data[(row - 1) * width + col]
    } else {
        0.0
    };
    let bottom = if row + 1 < height {
        data[(row + 1) * width + col]
    } else {
        0.0
    };
    let count = (row > 0) as u32 + (row + 1 < height) as u32;
    if count > 0 {
        (top + bottom) / count as f32
    } else {
        0.0
    }
}

// ── Malvar-He-Cutler (2004) demosaicing ────────────────────────────────

/// Malvar-He-Cutler gradient-corrected interpolation.
///
/// Uses 5x5 kernels that combine same-color averages with Laplacian
/// correction terms from the known channel at each site. Produces
/// significantly fewer color artifacts than bilinear.
///
/// Reference: Malvar, He, Cutler. "High-Quality Linear Interpolation for
/// Demosaicing of Bayer-Patterned Color Images" (2004).
fn demosaic_malvar<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    cfa: &C,
    mut rgb: Vec<f32>,
) -> Vec<f32> {
    debug_assert_eq!(rgb.len(), width * height * 3);
    debug_assert_eq!(cfa.tile_dims(), (2, 2), "Bayer kernel needs a 2×2 CFA tile");

    // The 5×5 Malvar kernels need a 2-pixel border with clamped access.
    // For images too small to have an interior region, use the safe path.
    const BORDER: usize = 2;
    if width <= 2 * BORDER || height <= 2 * BORDER {
        for row in 0..height {
            for col in 0..width {
                malvar_pixel_clamped(data, &mut rgb, width, height, row, col, cfa);
            }
        }
        return rgb;
    }

    // Precompute CFA 2×2 tile colors (Bayer patterns repeat every 2 rows/cols)
    let cfa_tile = [
        [cfa.color_at(0, 0), cfa.color_at(0, 1)],
        [cfa.color_at(1, 0), cfa.color_at(1, 1)],
    ];

    // For green sites, precompute whether horizontal neighbors are R or B.
    // gh[rp][cp] is only meaningful when cfa_tile[rp][cp] == G.
    let mut gh = [[0usize; 2]; 2];
    for rp in 0..2 {
        for cp in 0..2 {
            if cfa_tile[rp][cp] == G {
                gh[rp][cp] = cfa_tile[rp][1 - cp];
            }
        }
    }

    // ── Border pixels: safe clamped access ──
    // Top 2 rows
    for row in 0..BORDER {
        for col in 0..width {
            malvar_pixel_clamped(data, &mut rgb, width, height, row, col, cfa);
        }
    }
    // Bottom 2 rows
    for row in (height - BORDER)..height {
        for col in 0..width {
            malvar_pixel_clamped(data, &mut rgb, width, height, row, col, cfa);
        }
    }
    // Left/right 2 columns of interior rows
    for row in BORDER..(height - BORDER) {
        for col in 0..BORDER {
            malvar_pixel_clamped(data, &mut rgb, width, height, row, col, cfa);
        }
        for col in (width - BORDER)..width {
            malvar_pixel_clamped(data, &mut rgb, width, height, row, col, cfa);
        }
    }

    // ── Interior pixels: direct indexing, no boundary checks ──
    malvar_interior(data, &mut rgb, width, height, cfa_tile, gh);

    rgb
}

/// Interior Malvar demosaic loop — autoversioned for AVX2/NEON dispatch.
///
/// Processes all pixels with row ∈ [2, height-2) and col ∈ [2, width-2)
/// using direct array indexing (no boundary clamping).
#[autoversion]
fn malvar_interior(
    data: &[f32],
    rgb: &mut [f32],
    width: usize,
    height: usize,
    cfa_tile: [[usize; 2]; 2],
    gh: [[usize; 2]; 2],
) {
    const BORDER: usize = 2;
    let w = width;
    for row in BORDER..(height - BORDER) {
        let rp = row & 1;
        for col in BORDER..(width - BORDER) {
            let cp = col & 1;
            let color = cfa_tile[rp][cp];
            let idx = row * w + col;
            let out = idx * 3;
            let val = data[idx];

            let n = data[idx - w];
            let s = data[idx + w];
            let e = data[idx + 1];
            let we = data[idx - 1];
            let n2 = data[idx - 2 * w];
            let s2 = data[idx + 2 * w];
            let e2 = data[idx + 2];
            let w2 = data[idx - 2];
            let ne = data[idx - w + 1];
            let nw = data[idx - w - 1];
            let se = data[idx + w + 1];
            let sw = data[idx + w - 1];

            match color {
                R => {
                    rgb[out] = val;
                    rgb[out + 1] =
                        ((4.0 * val + 2.0 * (n + s + e + we) - (n2 + s2 + e2 + w2)) / 8.0).max(0.0);
                    rgb[out + 2] =
                        ((6.0 * val + 2.0 * (ne + nw + se + sw) - 1.5 * (n2 + s2 + e2 + w2)) / 8.0)
                            .max(0.0);
                }
                G => {
                    let h = (5.0 * val + 4.0 * (e + we) - (ne + nw + se + sw) - (e2 + w2)
                        + 0.5 * (n2 + s2))
                        / 8.0;
                    let v = (5.0 * val + 4.0 * (n + s) - (ne + nw + se + sw) - (n2 + s2)
                        + 0.5 * (e2 + w2))
                        / 8.0;
                    rgb[out + 1] = val;
                    if gh[rp][cp] == R {
                        rgb[out] = h.max(0.0);
                        rgb[out + 2] = v.max(0.0);
                    } else {
                        rgb[out] = v.max(0.0);
                        rgb[out + 2] = h.max(0.0);
                    }
                }
                B => {
                    rgb[out] =
                        ((6.0 * val + 2.0 * (ne + nw + se + sw) - 1.5 * (n2 + s2 + e2 + w2)) / 8.0)
                            .max(0.0);
                    rgb[out + 1] =
                        ((4.0 * val + 2.0 * (n + s + e + we) - (n2 + s2 + e2 + w2)) / 8.0).max(0.0);
                    rgb[out + 2] = val;
                }
                _ => {
                    rgb[out + 1] = val;
                }
            }
        }
    }
}

/// Process a single pixel using the safe clamped `px()` access pattern.
/// Used for border pixels where 5×5 kernel neighbors may be out of bounds.
fn malvar_pixel_clamped<C: CfaColorAt + ?Sized>(
    data: &[f32],
    rgb: &mut [f32],
    width: usize,
    height: usize,
    row: usize,
    col: usize,
    cfa: &C,
) {
    let color = cfa.color_at(row, col);
    let val = data[row * width + col];
    let out_idx = (row * width + col) * 3;

    match color {
        R => {
            rgb[out_idx] = val;
            rgb[out_idx + 1] = malvar_green_at_rb(data, width, height, row, col);
            rgb[out_idx + 2] = malvar_opposite_at_rb(data, width, height, row, col, cfa);
        }
        G => {
            let (r, b) = malvar_rb_at_green(data, width, height, row, col, cfa);
            rgb[out_idx] = r;
            rgb[out_idx + 1] = val;
            rgb[out_idx + 2] = b;
        }
        B => {
            rgb[out_idx] = malvar_opposite_at_rb(data, width, height, row, col, cfa);
            rgb[out_idx + 1] = malvar_green_at_rb(data, width, height, row, col);
            rgb[out_idx + 2] = val;
        }
        _ => {
            rgb[out_idx + 1] = val;
        }
    }
}

/// Safe pixel fetch with border clamping.
#[inline]
fn px(data: &[f32], width: usize, height: usize, row: isize, col: isize) -> f32 {
    let r = row.clamp(0, height as isize - 1) as usize;
    let c = col.clamp(0, width as isize - 1) as usize;
    data[r * width + c]
}

/// Green at R or B site using Malvar-He-Cutler kernel:
/// G = (4*Gc + 2*Laplacian_cross) / 8
/// where Gc = sum of 4 green cross neighbors
/// and Laplacian_cross = 4*center - top2 - bottom2 - left2 - right2
#[inline]
fn malvar_green_at_rb(data: &[f32], width: usize, height: usize, row: usize, col: usize) -> f32 {
    let r = row as isize;
    let c = col as isize;

    // Kernel coefficients from Malvar et al. (Table 1, Equation 1):
    //        0  0 -1  0  0
    //        0  0  2  0  0
    //       -1  2  4  2 -1
    //        0  0  2  0  0
    //        0  0 -1  0  0
    // Divide by 8
    let center = px(data, width, height, r, c);
    let n = px(data, width, height, r - 1, c);
    let s = px(data, width, height, r + 1, c);
    let e = px(data, width, height, r, c + 1);
    let w = px(data, width, height, r, c - 1);
    let n2 = px(data, width, height, r - 2, c);
    let s2 = px(data, width, height, r + 2, c);
    let e2 = px(data, width, height, r, c + 2);
    let w2 = px(data, width, height, r, c - 2);

    let val = (4.0 * center + 2.0 * (n + s + e + w) - (n2 + s2 + e2 + w2)) / 8.0;
    val.max(0.0)
}

/// R or B at opposite-color (B or R) site using Malvar-He-Cutler diagonal kernel:
///        0  0 -3/2  0  0
///        0  2   0   2  0
///      -3/2 0   6   0 -3/2
///        0  2   0   2  0
///        0  0 -3/2  0  0
/// Divide by 8
#[inline]
fn malvar_opposite_at_rb<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    row: usize,
    col: usize,
    cfa: &C,
) -> f32 {
    let _ = cfa;
    let r = row as isize;
    let c = col as isize;

    let center = px(data, width, height, r, c);
    let ne = px(data, width, height, r - 1, c + 1);
    let nw = px(data, width, height, r - 1, c - 1);
    let se = px(data, width, height, r + 1, c + 1);
    let sw = px(data, width, height, r + 1, c - 1);
    let n2 = px(data, width, height, r - 2, c);
    let s2 = px(data, width, height, r + 2, c);
    let e2 = px(data, width, height, r, c + 2);
    let w2 = px(data, width, height, r, c - 2);

    let val = (6.0 * center + 2.0 * (ne + nw + se + sw) - 1.5 * (n2 + s2 + e2 + w2)) / 8.0;
    val.max(0.0)
}

/// R and B at a green site. Uses directional kernels depending on
/// whether the row has R or B horizontal neighbors.
fn malvar_rb_at_green<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    row: usize,
    col: usize,
    cfa: &C,
) -> (f32, f32) {
    let r = row as isize;
    let c = col as isize;

    let center = px(data, width, height, r, c);

    // Horizontal kernel (for the color that's in horizontal neighbors):
    //    0   0  1/2  0   0
    //    0  -1   0  -1   0
    //   -1   4   5   4  -1
    //    0  -1   0  -1   0
    //    0   0  1/2  0   0
    // Divide by 8
    let n = px(data, width, height, r - 1, c);
    let s = px(data, width, height, r + 1, c);
    let e = px(data, width, height, r, c + 1);
    let w = px(data, width, height, r, c - 1);
    let ne = px(data, width, height, r - 1, c + 1);
    let nw = px(data, width, height, r - 1, c - 1);
    let se = px(data, width, height, r + 1, c + 1);
    let sw = px(data, width, height, r + 1, c - 1);
    let n2 = px(data, width, height, r - 2, c);
    let s2 = px(data, width, height, r + 2, c);
    let e2 = px(data, width, height, r, c + 2);
    let w2 = px(data, width, height, r, c - 2);

    let h_val =
        (5.0 * center + 4.0 * (e + w) - (ne + nw + se + sw) - (e2 + w2) + 0.5 * (n2 + s2)) / 8.0;
    let v_val =
        (5.0 * center + 4.0 * (n + s) - (ne + nw + se + sw) - (n2 + s2) + 0.5 * (e2 + w2)) / 8.0;

    let h_color = if col > 0 {
        cfa.color_at(row, col - 1)
    } else {
        cfa.color_at(row, col + 1)
    };

    if h_color == R {
        (h_val.max(0.0), v_val.max(0.0))
    } else {
        (v_val.max(0.0), h_val.max(0.0))
    }
}

// ── Generic CFA pattern + X-Trans demosaicing ─────────────────────────

/// Generic CFA pattern for arbitrary array sizes (X-Trans 6x6, etc.).
///
/// Stores the repeating color pattern and provides `color_at(row, col)`.
#[cfg_attr(not(feature = "rawler"), allow(dead_code))]
pub(crate) struct CfaPattern {
    colors: Vec<u8>,
    width: usize,
    height: usize,
}

#[cfg_attr(not(feature = "rawler"), allow(dead_code))]
impl CfaPattern {
    /// Create from a flat color array with the given tile dimensions.
    ///
    /// Colors: 0=R, 1=G, 2=B (matching rawloader convention).
    pub fn new(colors: Vec<u8>, width: usize, height: usize) -> Self {
        debug_assert_eq!(colors.len(), width * height);
        Self {
            colors,
            width,
            height,
        }
    }

    /// Color index at (row, col), wrapping by the tile dimensions.
    #[inline]
    pub fn color_at(&self, row: usize, col: usize) -> usize {
        let r = row % self.height;
        let c = col % self.width;
        self.colors[r * self.width + c] as usize
    }
}

impl CfaColorAt for CfaPattern {
    #[inline]
    fn color_at(&self, row: usize, col: usize) -> usize {
        CfaPattern::color_at(self, row, col)
    }

    #[inline]
    fn tile_dims(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

/// Bilinear demosaic for X-Trans and other non-Bayer CFA patterns.
///
/// For each pixel, the known channel is preserved and missing channels
/// are interpolated by averaging same-color neighbors in a 5×5 window.
/// This is a baseline quality algorithm — sufficient for previews and
/// testing but not as sharp as frequency-domain X-Trans algorithms.
#[allow(dead_code)] // test-only since the rawler path uses the _fallible variant
pub(crate) fn demosaic_xtrans_bilinear(
    data: &[f32],
    width: usize,
    height: usize,
    cfa: &CfaPattern,
) -> Vec<f32> {
    let rgb = vec![0.0f32; width * height * 3];
    demosaic_xtrans_bilinear_into(data, width, height, cfa, rgb)
}

/// X-Trans bilinear demosaic honoring the per-site
/// [`AllocPref`](crate::alloc_util::AllocPref) for the (untrusted-sized) output
/// buffer. Used by the rawler backend's decode pipeline; output bytes are
/// identical to [`demosaic_xtrans_bilinear`].
#[cfg(feature = "rawler")]
pub(crate) fn demosaic_xtrans_bilinear_fallible<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    cfa: &C,
    alloc_pref: crate::alloc_util::AllocPref,
) -> Result<Vec<f32>, whereat::At<crate::error::RawError>> {
    use whereat::at;
    let n = width
        .checked_mul(height)
        .and_then(|p| p.checked_mul(3))
        .ok_or_else(|| {
            at!(crate::error::RawError::OutOfMemory(
                "RGB f32 buffer size overflows usize".into()
            ))
        })?;
    // Full-image demosaic output sized from the (untrusted) sensor dims →
    // default fallible.
    let rgb = crate::alloc_util::alloc_filled(alloc_pref, true, 0.0f32, n)?;
    Ok(demosaic_xtrans_bilinear_into(data, width, height, cfa, rgb))
}

/// X-Trans bilinear demosaic into a pre-allocated `rgb` buffer (length
/// `width * height * 3`).
fn demosaic_xtrans_bilinear_into<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    cfa: &C,
    mut rgb: Vec<f32>,
) -> Vec<f32> {
    debug_assert_eq!(rgb.len(), width * height * 3);

    for row in 0..height {
        for col in 0..width {
            let known = cfa.color_at(row, col);
            let out_idx = (row * width + col) * 3;

            // Preserve known channel
            rgb[out_idx + known] = data[row * width + col];

            // Interpolate missing channels
            for ch in 0..3 {
                if ch == known {
                    continue;
                }
                rgb[out_idx + ch] =
                    interpolate_channel_xtrans(data, width, height, row, col, ch, cfa);
            }
        }
    }

    rgb
}

/// Average same-color neighbors within a 5×5 window.
fn interpolate_channel_xtrans<C: CfaColorAt + ?Sized>(
    data: &[f32],
    width: usize,
    height: usize,
    row: usize,
    col: usize,
    target_ch: usize,
    cfa: &C,
) -> f32 {
    let mut sum = 0.0f32;
    let mut count = 0u32;

    let r_start = row.saturating_sub(2);
    let r_end = (row + 3).min(height);
    let c_start = col.saturating_sub(2);
    let c_end = (col + 3).min(width);

    for nr in r_start..r_end {
        for nc in c_start..c_end {
            if nr == row && nc == col {
                continue;
            }
            if cfa.color_at(nr, nc) == target_ch {
                sum += data[nr * width + nc];
                count += 1;
            }
        }
    }

    if count > 0 { sum / count as f32 } else { 0.0 }
}

// Runs under EVERY feature set (including `--no-default-features --features
// std,rawler` and no backend at all): the Bayer kernels are backend-independent
// since imazen/zenraw#10 and these tests are what keeps them that way.
#[cfg(test)]
mod tests {
    use super::*;

    const BAYER_PATTERNS: [&str; 4] = ["RGGB", "BGGR", "GRBG", "GBRG"];

    /// Create a simple 4x4 RGGB Bayer pattern with known values.
    fn make_test_bayer() -> (Vec<f32>, usize, usize, BayerCfa) {
        // RGGB pattern:
        // R G R G
        // G B G B
        // R G R G
        // G B G B
        let cfa = BayerCfa::from_pattern_str("RGGB").unwrap();
        let width = 4;
        let height = 4;

        // Fill with checkerboard values:
        // R sites = 0.8, G sites = 0.5, B sites = 0.3
        let mut data = vec![0.0f32; width * height];
        for row in 0..height {
            for col in 0..width {
                let color = cfa.color_at(row, col);
                data[row * width + col] = match color {
                    R => 0.8,
                    G => 0.5,
                    B => 0.3,
                    _ => 0.0,
                };
            }
        }

        (data, width, height, cfa)
    }

    #[test]
    fn bilinear_produces_correct_dimensions() {
        let (data, width, height, cfa) = make_test_bayer();
        let rgb = demosaic_bilinear(&data, width, height, &cfa, vec![0.0; width * height * 3]);
        assert_eq!(rgb.len(), width * height * 3);
    }

    #[test]
    fn malvar_produces_correct_dimensions() {
        let (data, width, height, cfa) = make_test_bayer();
        let rgb = demosaic_malvar(&data, width, height, &cfa, vec![0.0; width * height * 3]);
        assert_eq!(rgb.len(), width * height * 3);
    }

    #[test]
    fn bilinear_known_channel_preserved() {
        let (data, width, height, cfa) = make_test_bayer();
        let rgb = demosaic_bilinear(&data, width, height, &cfa, vec![0.0; width * height * 3]);

        // At R sites, the red channel should be the original value
        for row in 0..height {
            for col in 0..width {
                let color = cfa.color_at(row, col);
                let idx = (row * width + col) * 3;
                match color {
                    R => assert!((rgb[idx] - 0.8).abs() < 1e-6, "R at ({row},{col})"),
                    G => assert!((rgb[idx + 1] - 0.5).abs() < 1e-6, "G at ({row},{col})"),
                    B => assert!((rgb[idx + 2] - 0.3).abs() < 1e-6, "B at ({row},{col})"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn malvar_known_channel_preserved() {
        let (data, width, height, cfa) = make_test_bayer();
        let rgb = demosaic_malvar(&data, width, height, &cfa, vec![0.0; width * height * 3]);

        for row in 0..height {
            for col in 0..width {
                let color = cfa.color_at(row, col);
                let idx = (row * width + col) * 3;
                match color {
                    R => assert!((rgb[idx] - 0.8).abs() < 1e-6, "R at ({row},{col})"),
                    G => assert!((rgb[idx + 1] - 0.5).abs() < 1e-6, "G at ({row},{col})"),
                    B => assert!((rgb[idx + 2] - 0.3).abs() < 1e-6, "B at ({row},{col})"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn bilinear_output_non_negative() {
        let (data, width, height, cfa) = make_test_bayer();
        let rgb = demosaic_bilinear(&data, width, height, &cfa, vec![0.0; width * height * 3]);
        for val in &rgb {
            assert!(*val >= 0.0, "Bilinear produced negative value: {val}");
        }
    }

    #[test]
    fn malvar_output_non_negative() {
        let (data, width, height, cfa) = make_test_bayer();
        let rgb = demosaic_malvar(&data, width, height, &cfa, vec![0.0; width * height * 3]);
        for val in &rgb {
            assert!(*val >= 0.0, "Malvar produced negative value: {val}");
        }
    }

    #[test]
    fn uniform_input_produces_uniform_output() {
        // If all Bayer values are the same, all RGB outputs should be the same
        let cfa = BayerCfa::from_pattern_str("RGGB").unwrap();
        let width = 8;
        let height = 8;
        let data = vec![0.5f32; width * height];

        let rgb_bilinear =
            demosaic_bilinear(&data, width, height, &cfa, vec![0.0; width * height * 3]);
        let rgb_malvar = demosaic_malvar(&data, width, height, &cfa, vec![0.0; width * height * 3]);

        // Interior pixels should be very close to 0.5 for all channels
        for row in 2..height - 2 {
            for col in 2..width - 2 {
                let idx = (row * width + col) * 3;
                for ch in 0..3 {
                    assert!(
                        (rgb_bilinear[idx + ch] - 0.5).abs() < 1e-4,
                        "Bilinear interior not uniform at ({row},{col}) ch={ch}: {}",
                        rgb_bilinear[idx + ch]
                    );
                    assert!(
                        (rgb_malvar[idx + ch] - 0.5).abs() < 1e-4,
                        "Malvar interior not uniform at ({row},{col}) ch={ch}: {}",
                        rgb_malvar[idx + ch]
                    );
                }
            }
        }
    }

    #[test]
    fn different_cfa_patterns() {
        let width = 8;
        let height = 8;
        let data = vec![0.5f32; width * height];

        for pattern in &BAYER_PATTERNS {
            let cfa = BayerCfa::from_pattern_str(pattern).unwrap();
            let rgb = demosaic_to_rgb_f32_infallible(
                &data,
                width,
                height,
                &cfa,
                DemosaicMethod::Bilinear,
            );
            assert_eq!(rgb.len(), width * height * 3, "Pattern {pattern}");

            let rgb = demosaic_to_rgb_f32_infallible(
                &data,
                width,
                height,
                &cfa,
                DemosaicMethod::MalvarHeCutler,
            );
            assert_eq!(rgb.len(), width * height * 3, "Pattern {pattern}");
        }
    }

    /// Deterministic, non-uniform sensor image so every kernel term matters.
    fn make_gradient_noise(width: usize, height: usize) -> Vec<f32> {
        (0..width * height)
            .map(|i| ((i * 37 + 13) % 101) as f32 / 100.0)
            .collect()
    }

    #[test]
    fn bayer_cfa_from_pattern_str() {
        let cfa = BayerCfa::from_pattern_str("GRBG").unwrap();
        assert_eq!(cfa.color_at(0, 0), G);
        assert_eq!(cfa.color_at(0, 1), R);
        assert_eq!(cfa.color_at(1, 0), B);
        assert_eq!(cfa.color_at(1, 1), G);
        // Wraps every 2 rows/cols.
        assert_eq!(cfa.color_at(6, 9), R);
        assert_eq!(cfa.color_at(7, 8), B);

        assert!(BayerCfa::from_pattern_str("RGG").is_none());
        assert!(BayerCfa::from_pattern_str("RGGBX").is_none());
        assert!(BayerCfa::from_pattern_str("RGGX").is_none());
    }

    #[test]
    fn bayer_cfa_matches_generic_cfa_pattern() {
        for pattern in &BAYER_PATTERNS {
            let bayer = BayerCfa::from_pattern_str(pattern).unwrap();
            let colors: Vec<u8> = pattern
                .bytes()
                .map(|b| match b {
                    b'R' => R as u8,
                    b'G' => G as u8,
                    b'B' => B as u8,
                    _ => unreachable!(),
                })
                .collect();
            let generic = CfaPattern::new(colors, 2, 2);
            for row in 0..7 {
                for col in 0..9 {
                    assert_eq!(
                        CfaColorAt::color_at(&bayer, row, col),
                        CfaColorAt::color_at(&generic, row, col),
                        "{pattern} at ({row},{col})"
                    );
                }
            }
        }
    }

    /// `demosaic_to_rgb_f32_fallible` (the decode-pipeline entry) produces the
    /// same bytes as the infallible entry for the same CFA — only the
    /// allocation strategy differs.
    #[test]
    fn fallible_entry_matches_infallible_bytes() {
        let (width, height) = (16, 12);
        let data = make_gradient_noise(width, height);
        for pattern in &BAYER_PATTERNS {
            let cfa = BayerCfa::from_pattern_str(pattern).unwrap();
            for method in [DemosaicMethod::Bilinear, DemosaicMethod::MalvarHeCutler] {
                let a = demosaic_to_rgb_f32_infallible(&data, width, height, &cfa, method);
                let b = demosaic_to_rgb_f32_fallible(
                    &data,
                    width,
                    height,
                    &cfa,
                    method,
                    crate::alloc_util::AllocPref::default(),
                )
                .unwrap();
                assert_eq!(a.len(), b.len());
                assert!(
                    a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
                    "{pattern} {method:?}: fallible/infallible outputs differ"
                );
            }
        }
    }

    /// The rawler backend now feeds a [`BayerCfa`] into the same kernels the
    /// rawloader backend drives with `rawloader::CFA`. Both lookups must
    /// yield bit-identical pixels for every 2×2 pattern and both methods —
    /// this is the guard that the #10 fix changed no output bytes.
    #[cfg(feature = "rawloader")]
    #[test]
    fn bayer_cfa_is_bit_identical_to_rawloader_cfa() {
        let (width, height) = (16, 12);
        let data = make_gradient_noise(width, height);
        for pattern in &BAYER_PATTERNS {
            let rl = rawloader::CFA::new(pattern);
            let bayer = BayerCfa::new([
                [rl.color_at(0, 0), rl.color_at(0, 1)],
                [rl.color_at(1, 0), rl.color_at(1, 1)],
            ]);
            assert_eq!(bayer, BayerCfa::from_pattern_str(pattern).unwrap());
            for method in [DemosaicMethod::Bilinear, DemosaicMethod::MalvarHeCutler] {
                let via_rawloader = demosaic_to_rgb_f32(&data, width, height, &rl, method);
                let via_bayer =
                    demosaic_to_rgb_f32_infallible(&data, width, height, &bayer, method);
                assert_eq!(via_rawloader.len(), via_bayer.len());
                for (i, (a, b)) in via_rawloader.iter().zip(&via_bayer).enumerate() {
                    assert_eq!(
                        a.to_bits(),
                        b.to_bits(),
                        "{pattern} {method:?}: byte mismatch at f32 index {i}: {a} vs {b}"
                    );
                }
            }
        }
    }

    /// Standard Fuji X-Trans 6x6 CFA pattern.
    fn make_xtrans_cfa() -> CfaPattern {
        // Typical X-Trans II/III pattern
        #[rustfmt::skip]
        let colors: Vec<u8> = vec![
            G, B, G, G, R, G,
            R, G, R, B, G, B,
            G, B, G, G, R, G,
            G, R, G, G, B, G,
            B, G, B, R, G, R,
            G, R, G, G, B, G,
        ].into_iter().map(|c| c as u8).collect();
        CfaPattern::new(colors, 6, 6)
    }

    #[test]
    fn xtrans_bilinear_dimensions() {
        let cfa = make_xtrans_cfa();
        let width = 24;
        let height = 24;
        let data = vec![0.5f32; width * height];
        let rgb = demosaic_xtrans_bilinear(&data, width, height, &cfa);
        assert_eq!(rgb.len(), width * height * 3);
    }

    #[test]
    fn xtrans_bilinear_known_channel_preserved() {
        let cfa = make_xtrans_cfa();
        let width = 12;
        let height = 12;
        // Fill each sensor site with a known value based on its color
        let mut data = vec![0.0f32; width * height];
        for row in 0..height {
            for col in 0..width {
                data[row * width + col] = match cfa.color_at(row, col) {
                    R => 0.8,
                    G => 0.5,
                    B => 0.3,
                    _ => 0.0,
                };
            }
        }

        let rgb = demosaic_xtrans_bilinear(&data, width, height, &cfa);
        for row in 0..height {
            for col in 0..width {
                let known = cfa.color_at(row, col);
                let idx = (row * width + col) * 3;
                let expected = match known {
                    R => 0.8,
                    G => 0.5,
                    B => 0.3,
                    _ => 0.0,
                };
                assert!(
                    (rgb[idx + known] - expected).abs() < 1e-6,
                    "Known channel not preserved at ({row},{col})"
                );
            }
        }
    }

    #[test]
    fn xtrans_bilinear_uniform_input() {
        let cfa = make_xtrans_cfa();
        let width = 24;
        let height = 24;
        let data = vec![0.5f32; width * height];
        let rgb = demosaic_xtrans_bilinear(&data, width, height, &cfa);

        // Interior pixels should be close to 0.5 for all channels
        for row in 3..height - 3 {
            for col in 3..width - 3 {
                let idx = (row * width + col) * 3;
                for ch in 0..3 {
                    assert!(
                        (rgb[idx + ch] - 0.5).abs() < 1e-4,
                        "X-Trans interior not uniform at ({row},{col}) ch={ch}: {}",
                        rgb[idx + ch]
                    );
                }
            }
        }
    }

    #[test]
    fn xtrans_bilinear_non_negative() {
        let cfa = make_xtrans_cfa();
        let width = 12;
        let height = 12;
        let mut data = vec![0.0f32; width * height];
        // Random-ish pattern
        for (i, v) in data.iter_mut().enumerate() {
            *v = ((i * 37 + 13) % 100) as f32 / 100.0;
        }
        let rgb = demosaic_xtrans_bilinear(&data, width, height, &cfa);
        for val in &rgb {
            assert!(*val >= 0.0, "X-Trans produced negative value: {val}");
        }
    }

    // ── CFA-tile dispatch (imazen/zenraw#… — X-Trans on the Bayer kernel) ──

    /// A CFA whose pattern the backend could not describe at all.
    struct UndescribedCfa;
    impl CfaColorAt for UndescribedCfa {
        fn color_at(&self, _row: usize, _col: usize) -> usize {
            0
        }
        fn tile_dims(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    /// Fill every sensor site with a value that identifies its CFA colour, so
    /// "the known channel survived demosaicing" is checkable per pixel.
    fn fill_by_color<C: CfaColorAt + ?Sized>(cfa: &C, width: usize, height: usize) -> Vec<f32> {
        let mut data = vec![0.0f32; width * height];
        for row in 0..height {
            for col in 0..width {
                data[row * width + col] = match cfa.color_at(row, col) {
                    R => 0.8,
                    G => 0.5,
                    B => 0.3,
                    _ => 0.0,
                };
            }
        }
        data
    }

    /// **The X-Trans regression.** Every demosaic kernel must leave the channel
    /// the sensor actually measured untouched at that site. The 2×2 Bayer
    /// kernels cannot: they read a precomputed `cfa_tile[row & 1][col & 1]`, so
    /// on a 6×6 X-Trans CFA they write `val` into the wrong channel for most
    /// interior sites, while the clamped border path (which consults the real
    /// `color_at`) writes it into the right one — the interior and the border of
    /// one image disagreeing. Dispatching on `tile_dims()` is what fixes it.
    ///
    /// Both `DemosaicMethod`s are checked because both used to reach a
    /// 2×2-only kernel.
    #[test]
    fn xtrans_known_channel_survives_top_level_dispatch() {
        let cfa = make_xtrans_cfa();
        let (width, height) = (24, 18);
        let data = fill_by_color(&cfa, width, height);

        for method in [DemosaicMethod::Bilinear, DemosaicMethod::MalvarHeCutler] {
            let rgb = demosaic_to_rgb_f32_infallible(&data, width, height, &cfa, method);
            for row in 0..height {
                for col in 0..width {
                    let known = cfa.color_at(row, col);
                    let idx = (row * width + col) * 3;
                    let expected = data[row * width + col];
                    assert!(
                        (rgb[idx + known] - expected).abs() < 1e-6,
                        "{method:?}: measured channel {known} clobbered at ({row},{col}): \
                         got {}, sensor said {expected}",
                        rgb[idx + known]
                    );
                }
            }
        }
    }

    /// A 6×6 CFA must produce the same pixels through the top-level dispatch as
    /// through the pattern-agnostic kernel called directly — i.e. the dispatch
    /// really did route away from the Bayer kernels, and routed to *this* one.
    #[test]
    fn xtrans_dispatch_matches_pattern_agnostic_kernel() {
        let cfa = make_xtrans_cfa();
        let (width, height) = (24, 18);
        let data = make_gradient_noise(width, height);
        let direct = demosaic_xtrans_bilinear(&data, width, height, &cfa);
        for method in [DemosaicMethod::Bilinear, DemosaicMethod::MalvarHeCutler] {
            let routed = demosaic_to_rgb_f32_infallible(&data, width, height, &cfa, method);
            assert_eq!(routed.len(), direct.len());
            for (i, (a, b)) in routed.iter().zip(&direct).enumerate() {
                assert_eq!(
                    a.to_bits(),
                    b.to_bits(),
                    "{method:?}: dispatch diverged from the X-Trans kernel at f32 index {i}"
                );
            }
        }
    }

    /// The dispatch must not disturb Bayer: a 2×2 CFA still selects the Bayer
    /// kernel the caller asked for, bit for bit. This is what keeps every
    /// existing Bayer corpus byte-identical across the routing change.
    #[test]
    fn bayer_dispatch_still_selects_the_requested_kernel() {
        let (width, height) = (16, 12);
        let data = make_gradient_noise(width, height);
        for pattern in &BAYER_PATTERNS {
            let cfa = BayerCfa::from_pattern_str(pattern).unwrap();
            for method in [DemosaicMethod::Bilinear, DemosaicMethod::MalvarHeCutler] {
                let routed = demosaic_to_rgb_f32_infallible(&data, width, height, &cfa, method);
                let direct = match method {
                    DemosaicMethod::Bilinear => demosaic_bilinear(
                        &data,
                        width,
                        height,
                        &cfa,
                        vec![0.0f32; width * height * 3],
                    ),
                    DemosaicMethod::MalvarHeCutler => demosaic_malvar(
                        &data,
                        width,
                        height,
                        &cfa,
                        vec![0.0f32; width * height * 3],
                    ),
                };
                for (i, (a, b)) in routed.iter().zip(&direct).enumerate() {
                    assert_eq!(
                        a.to_bits(),
                        b.to_bits(),
                        "{pattern} {method:?}: dispatch changed the Bayer result at index {i}"
                    );
                }
            }
        }
    }

    /// A CFA the backend could not describe is refused with a typed error
    /// rather than demosaiced against a fabricated pattern.
    #[test]
    fn undescribed_cfa_is_a_typed_error() {
        let (width, height) = (8, 8);
        let data = vec![0.5f32; width * height];
        let err = demosaic_to_rgb_f32_fallible(
            &data,
            width,
            height,
            &UndescribedCfa,
            DemosaicMethod::MalvarHeCutler,
            crate::alloc_util::AllocPref::default(),
        )
        .expect_err("an undescribed CFA must not silently demosaic");
        assert!(
            matches!(err.error(), crate::error::RawError::UnsupportedFeature(_)),
            "expected UnsupportedFeature, got {err:?}"
        );
    }

    /// `tile_dims` must report what the pattern actually is — the dispatch is
    /// only as good as this answer.
    #[test]
    fn tile_dims_reports_the_real_pattern_size() {
        assert_eq!(
            BayerCfa::from_pattern_str("RGGB").unwrap().tile_dims(),
            (2, 2)
        );
        assert_eq!(make_xtrans_cfa().tile_dims(), (6, 6));
        assert_eq!(UndescribedCfa.tile_dims(), (0, 0));
    }

    #[cfg(feature = "rawloader")]
    #[test]
    fn rawloader_cfa_tile_dims_match_upstream() {
        // rawloader::CFA::new maps pattern length → tile size; a 36-char
        // pattern is X-Trans's 6×6, and an empty pattern is undescribed.
        assert_eq!(rawloader::CFA::new("RGGB").tile_dims(), (2, 2));
        let xtrans = "GBGGRGRGRBGBGBGGRGGRGGBGBGBRGRGRGGBG";
        assert_eq!(xtrans.len(), 36);
        assert_eq!(rawloader::CFA::new(xtrans).tile_dims(), (6, 6));
        assert_eq!(rawloader::CFA::new("").tile_dims(), (0, 0));
    }

    /// End-to-end through the public entry point with a real `rawloader::CFA`:
    /// the X-Trans pattern that used to hit the Bayer kernel must now preserve
    /// the measured channel everywhere.
    #[cfg(feature = "rawloader")]
    #[test]
    fn public_entry_routes_rawloader_xtrans_correctly() {
        let cfa = rawloader::CFA::new("GBGGRGRGRBGBGBGGRGGRGGBGBGBRGRGRGGBG");
        let (width, height) = (24, 18);
        let data = fill_by_color(&cfa, width, height);
        for method in [DemosaicMethod::Bilinear, DemosaicMethod::MalvarHeCutler] {
            let rgb = demosaic_to_rgb_f32(&data, width, height, &cfa, method);
            for row in 0..height {
                for col in 0..width {
                    let known = CfaColorAt::color_at(&cfa, row, col);
                    let idx = (row * width + col) * 3;
                    let expected = data[row * width + col];
                    assert!(
                        (rgb[idx + known] - expected).abs() < 1e-6,
                        "{method:?}: measured channel clobbered at ({row},{col})"
                    );
                }
            }
        }
    }
}
