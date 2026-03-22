/// darktable-compatible sigmoid tone mapper.
///
/// Implements the generalized log-logistic sigmoid from darktable's sigmoid module.
/// The film/paper model: `magnitude * pow(pow(film_fog + x, film_power) / (paper_exp + pow(film_fog + x, film_power)), paper_power)`
///
/// This operates per-channel in linear RGB space (not Oklab), matching darktable's
/// scene-referred default pipeline.
///
/// Parameters match darktable's sigmoid module:
/// - `contrast`: middle-grey contrast (1.5 default, range 0.1-10)
/// - `skew`: contrast skewness (-1 to 1, 0 = symmetric)
/// - `hue_preservation`: 0.0-1.0 (1.0 default, maps from dt's 0-100)
///
/// Data from darktable (GPL-2.0+), reimplemented from mathematical description.
const MIDDLE_GREY: f32 = 0.1845;

/// Internal parameters computed from user-facing contrast/skew.
#[derive(Clone, Debug)]
pub(crate) struct DtSigmoidParams {
    pub(crate) white_target: f32,
    pub(crate) black_target: f32,
    pub(crate) paper_exp: f32,
    pub(crate) film_fog: f32,
    pub(crate) film_power: f32,
    pub(crate) paper_power: f32,
    pub(crate) hue_preservation: f32,
}

/// The core generalized log-logistic sigmoid function.
fn loglogistic_sigmoid(
    value: f32,
    magnitude: f32,
    paper_exp: f32,
    film_fog: f32,
    film_power: f32,
    paper_power: f32,
) -> f32 {
    let clamped = value.max(0.0);
    let film_response = (film_fog + clamped).powf(film_power);
    let paper_response =
        magnitude * (film_response / (paper_exp + film_response)).powf(paper_power);
    if paper_response.is_nan() {
        magnitude
    } else {
        paper_response
    }
}

/// Compute internal sigmoid parameters from user-facing contrast and skew.
///
/// Follows darktable's commit_params() logic.
pub(crate) fn compute_params(
    contrast: f32,
    skew: f32,
    display_white: f32,
    display_black: f32,
    hue_preservation: f32,
) -> DtSigmoidParams {
    let white_target = 0.01 * display_white;
    let black_target = 0.01 * display_black;

    // Step 1: Reference slope at middle grey with no skew
    let ref_film_power = contrast;
    let ref_paper_power = 1.0f32;
    let ref_magnitude = 1.0f32;
    let ref_film_fog = 0.0f32;
    let ref_paper_exp = MIDDLE_GREY.powf(ref_film_power) * ((ref_magnitude / MIDDLE_GREY) - 1.0);

    let delta = 1e-6f32;
    let ref_plus = loglogistic_sigmoid(
        MIDDLE_GREY + delta,
        ref_magnitude,
        ref_paper_exp,
        ref_film_fog,
        ref_film_power,
        ref_paper_power,
    );
    let ref_minus = loglogistic_sigmoid(
        MIDDLE_GREY - delta,
        ref_magnitude,
        ref_paper_exp,
        ref_film_fog,
        ref_film_power,
        ref_paper_power,
    );
    let ref_slope = (ref_plus - ref_minus) / (2.0 * delta);

    // Step 2: Apply skew
    let paper_power = 5.0f32.powf(-skew);

    // Step 3: Temporary slope at film_power=1 with target white
    let temp_film_power = 1.0f32;
    let temp_white_grey_relation = (white_target / MIDDLE_GREY).powf(1.0 / paper_power) - 1.0;
    let temp_paper_exp = MIDDLE_GREY.powf(temp_film_power) * temp_white_grey_relation;

    let temp_plus = loglogistic_sigmoid(
        MIDDLE_GREY + delta,
        white_target,
        temp_paper_exp,
        0.0,
        temp_film_power,
        paper_power,
    );
    let temp_minus = loglogistic_sigmoid(
        MIDDLE_GREY - delta,
        white_target,
        temp_paper_exp,
        0.0,
        temp_film_power,
        paper_power,
    );
    let temp_slope = (temp_plus - temp_minus) / (2.0 * delta);

    // Step 4: Film power scales reference slope to target
    let film_power = if temp_slope.abs() > 1e-10 {
        ref_slope / temp_slope
    } else {
        contrast
    };

    // Step 5: Final parameter computation
    let white_grey_relation = (white_target / MIDDLE_GREY).powf(1.0 / paper_power) - 1.0;
    let white_black_relation = (black_target / white_target).powf(-1.0 / paper_power) - 1.0;

    let wgr_root = white_grey_relation.powf(1.0 / film_power);
    let wbr_root = white_black_relation.powf(1.0 / film_power);
    let film_fog = if (wbr_root - wgr_root).abs() > 1e-10 {
        MIDDLE_GREY * wgr_root / (wbr_root - wgr_root)
    } else {
        0.0
    };

    let paper_exp = (film_fog + MIDDLE_GREY).powf(film_power) * white_grey_relation;

    DtSigmoidParams {
        white_target,
        black_target,
        paper_exp,
        film_fog,
        film_power,
        paper_power,
        hue_preservation,
    }
}

/// Compute default parameters matching darktable 5.5 defaults.
pub(crate) fn default_params() -> DtSigmoidParams {
    compute_params(1.5, 0.0, 100.0, 0.0152, 1.0)
}

/// Apply darktable sigmoid per-channel to linear RGB f32 data in-place.
///
/// This applies the log-logistic sigmoid to each RGB channel independently,
/// with optional hue preservation (interpolating the middle channel).
pub(crate) fn apply_dt_sigmoid(data: &mut [f32], params: &DtSigmoidParams) {
    let n = data.len() / 3;
    for i in 0..n {
        let base = i * 3;
        let r = data[base];
        let g = data[base + 1];
        let b = data[base + 2];

        // Desaturate negative values
        let avg = ((r + g + b) / 3.0).max(0.0);
        let min_val = r.min(g).min(b);
        let sat_factor = if min_val < 0.0 {
            -avg / (min_val - avg)
        } else {
            1.0
        };
        let r = avg + sat_factor * (r - avg);
        let g = avg + sat_factor * (g - avg);
        let b = avg + sat_factor * (b - avg);

        // Per-channel sigmoid
        let sr = loglogistic_sigmoid(
            r,
            params.white_target,
            params.paper_exp,
            params.film_fog,
            params.film_power,
            params.paper_power,
        );
        let sg = loglogistic_sigmoid(
            g,
            params.white_target,
            params.paper_exp,
            params.film_fog,
            params.film_power,
            params.paper_power,
        );
        let sb = loglogistic_sigmoid(
            b,
            params.white_target,
            params.paper_exp,
            params.film_fog,
            params.film_power,
            params.paper_power,
        );

        if params.hue_preservation > 1e-6 {
            // Hue preservation: find channel order and interpolate middle channel
            let pix = [r, g, b];
            let per_ch = [sr, sg, sb];
            let (max_i, mid_i, min_i) = channel_order(&pix);

            let chroma = pix[max_i] - pix[min_i];
            let midscale = if chroma.abs() > 1e-10 {
                (pix[mid_i] - pix[min_i]) / chroma
            } else {
                0.0
            };

            // Full hue correction for middle channel
            let full_hue_mid = per_ch[min_i] + (per_ch[max_i] - per_ch[min_i]) * midscale;
            let naive_hue_mid = (1.0 - params.hue_preservation) * per_ch[mid_i]
                + params.hue_preservation * full_hue_mid;

            // Energy preservation
            let per_ch_energy = per_ch[0] + per_ch[1] + per_ch[2];
            let naive_hue_energy = per_ch[min_i] + naive_hue_mid + per_ch[max_i];
            let pix_min_plus_mid = pix[min_i] + pix[mid_i];
            let blend = if pix_min_plus_mid.abs() > 1e-10 {
                2.0 * pix[min_i] / pix_min_plus_mid
            } else {
                0.0
            };
            let energy_target = blend * per_ch_energy + (1.0 - blend) * naive_hue_energy;

            let mut out = [0.0f32; 3];
            if naive_hue_mid <= per_ch[mid_i] {
                let hp = params.hue_preservation;
                let corrected_mid = ((1.0 - hp) * per_ch[mid_i]
                    + hp * (midscale * per_ch[max_i]
                        + (1.0 - midscale) * (energy_target - per_ch[max_i])))
                    / (1.0 + hp * (1.0 - midscale));
                out[min_i] = energy_target - per_ch[max_i] - corrected_mid;
                out[mid_i] = corrected_mid;
                out[max_i] = per_ch[max_i];
            } else {
                let hp = params.hue_preservation;
                let corrected_mid = ((1.0 - hp) * per_ch[mid_i]
                    + hp * (per_ch[min_i] * (1.0 - midscale)
                        + midscale * (energy_target - per_ch[min_i])))
                    / (1.0 + hp * midscale);
                out[min_i] = per_ch[min_i];
                out[mid_i] = corrected_mid;
                out[max_i] = energy_target - per_ch[min_i] - corrected_mid;
            }

            data[base] = out[0];
            data[base + 1] = out[1];
            data[base + 2] = out[2];
        } else {
            data[base] = sr;
            data[base + 1] = sg;
            data[base + 2] = sb;
        }
    }
}

/// Determine max/mid/min channel indices.
fn channel_order(pix: &[f32; 3]) -> (usize, usize, usize) {
    if pix[0] >= pix[1] {
        if pix[1] > pix[2] {
            (0, 1, 2)
        }
        // r >= g > b
        else if pix[2] > pix[0] {
            (2, 0, 1)
        }
        // b > r >= g
        else if pix[2] > pix[1] {
            (0, 2, 1)
        }
        // r >= b > g
        else {
            (0, 1, 2)
        } // r >= g >= b (handles r==g==b)
    } else if pix[0] >= pix[2] {
        (1, 0, 2)
    }
    // g > r >= b
    else if pix[2] > pix[1] {
        (2, 1, 0)
    }
    // b > g > r
    else {
        (1, 2, 0)
    } // g >= b > r
}
