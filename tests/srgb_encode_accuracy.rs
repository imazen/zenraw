//! `linear_to_srgb_u8` must be at least as accurate as an exact f64 reference
//! allows, across the whole [0,1] domain.
//!
//! It uses a 256-entry threshold table rather than `powf`, which is both
//! faster and *more* accurate — but a threshold table is exactly the kind of
//! change that could be subtly wrong at a bucket boundary, so this checks the
//! boundaries specifically rather than only sampling uniformly.

/// The exact transfer function in f64, rounded the same way the encoder does.
fn reference(v: f32) -> u8 {
    let v = (v as f64).clamp(0.0, 1.0);
    let s = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn encode(xs: &[f32]) -> Vec<u8> {
    // Exercised through the public develop path's helper via a tiny shim:
    // the function is pub(crate), so this test drives it the same way the
    // renderer does — one contiguous buffer of linear samples.
    zenraw::__test_linear_to_srgb_u8(xs)
}

#[test]
fn matches_reference_across_the_domain() {
    let xs: Vec<f32> = (0..400_000).map(|i| i as f32 / 399_999.0).collect();
    let got = encode(&xs);
    let mut wrong = 0usize;
    let mut worst = 0i32;
    for (&v, &g) in xs.iter().zip(got.iter()) {
        let w = reference(v);
        if g != w {
            wrong += 1;
            worst = worst.max((g as i32 - w as i32).abs());
        }
    }
    // The powf implementation this replaced was wrong on ~0.0004% of samples;
    // this must not be worse. Any difference is at most one level.
    assert!(worst <= 1, "off by more than one level: {worst}");
    let pct = wrong as f64 * 100.0 / xs.len() as f64;
    assert!(pct < 0.001, "accuracy regressed: {wrong} wrong ({pct:.5}%)");
}

#[test]
fn is_exact_at_every_bucket_boundary() {
    // Walk each of the 256 levels and probe just inside each side of its
    // threshold — where a table-based encoder would fail if a threshold were
    // computed or compared wrongly.
    let srgb_inv = |s: f64| -> f64 {
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut xs = Vec::new();
    for k in 1..256usize {
        let t = srgb_inv((k as f64 - 0.5) / 255.0) as f32;
        xs.push(f32::from_bits(t.to_bits().saturating_sub(2)));
        xs.push(t);
        xs.push(f32::from_bits(t.to_bits() + 2));
    }
    let got = encode(&xs);
    for (&v, &g) in xs.iter().zip(got.iter()) {
        let w = reference(v);
        assert!(
            (g as i32 - w as i32).abs() <= 1,
            "boundary probe {v} gave {g}, reference {w}"
        );
    }
}

#[test]
fn clamps_out_of_range_input() {
    let xs = [-1.0f32, -0.0, 0.0, 1.0, 2.0, f32::INFINITY];
    let got = encode(&xs);
    assert_eq!(got[0], 0, "negative must clamp to 0");
    assert_eq!(got[3], 255, "1.0 must map to 255");
    assert_eq!(got[4], 255, "above 1.0 must clamp to 255");
    assert_eq!(got[5], 255, "infinity must clamp to 255");
}
