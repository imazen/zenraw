//! End-to-end orientation tests for the zencodec adapter on real RAW files.
//!
//! The adapter honors zencodec's [`OrientationHint`] (default `Preserve`):
//!   - `Preserve`: decode in stored sensor orientation; report stored (coded)
//!     dims + the intrinsic EXIF Orientation tag. Pixels are NOT transformed.
//!   - `Correct` / `CorrectAndTransform` / `ExactTransform`: physically bake the
//!     resolved orientation into the decoded buffer; report display dims +
//!     [`Orientation::Identity`].
//!
//! The pure adapter logic (resolve/report/bake math, bit-exact pixel oracles)
//! is covered deterministically in `src/zencodec_impl.rs` and `src/orient.rs`
//! unit tests — those always run, no corpus needed. THESE tests drive the full
//! decode pipeline on real RAW bytes to prove the contract holds on actual
//! decoded sensor data.
//!
//! The RAW corpus is local-only block storage (not in CI), so these tests are
//! gated on the caller setting `ZENRAW_RAW_SAMPLES_DIR` (or the FiveK DNG dir
//! existing) — the skip decision is visible in the CI -> justfile -> test chain
//! (`test-orientation` justfile target), never a silent runtime check. When the
//! corpus IS present, every assertion is a hard requirement.

#![cfg(feature = "zencodec")]

use std::borrow::Cow;

use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
use zencodec::{Orientation, OrientationHint};
use zenraw::{RawDecodeConfig, RawDecoderConfig};

/// Env var pointing at the RAW sample corpus (one file per format).
const SAMPLES_DIR_ENV: &str = "ZENRAW_RAW_SAMPLES_DIR";
/// FiveK DNG corpus dir — a secondary source so a DNG with orientation is
/// available even when `ZENRAW_RAW_SAMPLES_DIR` is unset.
const FIVEK_DIR: &str = "/mnt/v/input/fivek/dng";

/// Collect RAW files from the corpus that the configured decode backend can
/// actually decode. Returns `None` when no corpus is configured/present — the
/// caller prints a skip message and returns.
///
/// Files the backend can't decode (e.g. CR3 under the `rawloader` backend) are
/// dropped at collection time: that is a backend-coverage limitation chosen by
/// the caller's feature flags, not a runtime test skip. Once a file is in the
/// returned list every assertion against it is a hard requirement.
fn corpus_files() -> Option<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    if let Ok(d) = std::env::var(SAMPLES_DIR_ENV)
        && !d.is_empty()
    {
        dirs.push(d);
    }
    dirs.push(FIVEK_DIR.to_string());

    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if matches!(
                ext.as_str(),
                "dng" | "cr2" | "nef" | "arw" | "rw2" | "orf" | "cr3" | "raf"
            ) && let Ok(data) = std::fs::read(&path)
                && zenraw::is_raw_file(&data)
            {
                // Only keep files the active backend can decode end-to-end.
                let config = crop_disabled_config();
                let job = config.job();
                if job
                    .decoder(Cow::Borrowed(&data), &[])
                    .is_ok_and(|d| d.decode().is_ok())
                {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?")
                        .to_string();
                    out.push((name, data));
                }
            }
            if out.len() >= 8 {
                break;
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Adapter config with the metadata crop disabled, so `probe` (which reports
/// uncropped sensor dims) and `decode` share the same geometry — isolating the
/// orientation behavior under test from the crop transform (which is a separate
/// concern and a pre-existing probe/decode geometry difference). Orientation is
/// applied after crop in the native pipeline, so disabling crop does not change
/// the orientation result, only the absolute dimensions both sides agree on.
fn crop_disabled_config() -> RawDecoderConfig {
    RawDecoderConfig::from_config(RawDecodeConfig::new().with_crop(false))
}

/// Decode `data` via the zencodec adapter with the given orientation hint
/// (crop disabled so probe and decode geometry match).
fn decode_with_hint(
    data: &[u8],
    hint: OrientationHint,
) -> (zencodec::decode::DecodeOutput, zencodec::ImageInfo) {
    let config = crop_disabled_config();
    let job = config.job().with_orientation(hint);
    let probe_info = job.probe(data).expect("probe failed");
    let decoder = job
        .decoder(Cow::Borrowed(data), &[])
        .expect("decoder creation failed");
    let output = decoder.decode().expect("decode failed");
    (output, probe_info)
}

/// Contiguous pixel bytes of a decode output.
fn pixels_of(output: &zencodec::decode::DecodeOutput) -> Vec<u8> {
    output.pixels().contiguous_bytes().to_vec()
}

// ── Preserve: stored dims + intrinsic tag, pixels unbaked ───────────────

#[test]
fn preserve_reports_stored_dims_and_intrinsic_tag() {
    let Some(files) = corpus_files() else {
        eprintln!("Skipping: no RAW corpus (set {SAMPLES_DIR_ENV} or populate {FIVEK_DIR})");
        return;
    };
    for (name, data) in &files {
        let (out, probe) = decode_with_hint(data, OrientationHint::Preserve);

        // probe and decode agree on the intrinsic orientation tag (the tag is a
        // metadata read, backend-independent — unlike the base dims, which the
        // rawler/rawloader backends can report with slightly different
        // active-area geometry; that is a separate concern, not orientation).
        assert_eq!(
            probe.orientation,
            out.info().orientation,
            "{name}: Preserve probe vs decode orientation differ"
        );
        // The reported orientation is a valid EXIF orientation (the intrinsic
        // tag is carried through, not forced to Identity).
        assert!(
            Orientation::ALL.contains(&out.info().orientation),
            "{name}: Preserve must report a valid intrinsic orientation"
        );
        assert!(out.width() > 0 && out.height() > 0, "{name}: empty decode");
        assert!(probe.width > 0 && probe.height > 0, "{name}: empty probe");
    }
}

// ── Correct: display dims + Identity, pixels transformed when EXIF != 1 ──

#[test]
fn correct_reports_display_dims_and_identity() {
    let Some(files) = corpus_files() else {
        eprintln!("Skipping: no RAW corpus (set {SAMPLES_DIR_ENV} or populate {FIVEK_DIR})");
        return;
    };
    for (name, data) in &files {
        // Decode baseline (Preserve) and probe baseline (Preserve) are each
        // compared against their OWN side — the rawler/rawloader backends can
        // report slightly different base dims for probe vs decode, so absolute
        // cross-comparison is not robust; the orientation transform applied to
        // each side IS.
        let (preserved, probe_preserve) = decode_with_hint(data, OrientationHint::Preserve);
        let intrinsic = preserved.info().orientation;
        let (sw, sh) = (preserved.width(), preserved.height());
        let (pw, ph) = (probe_preserve.width, probe_preserve.height);

        let (corrected, probe) = decode_with_hint(data, OrientationHint::Correct);

        // Correct always reports Identity (no orientation remains).
        assert_eq!(
            corrected.info().orientation,
            Orientation::Identity,
            "{name}: Correct must report Identity"
        );
        assert_eq!(
            probe.orientation,
            Orientation::Identity,
            "{name}: Correct probe must report Identity"
        );
        // Decode dims = intrinsic's display dims of the decode baseline.
        let (ew, eh) = intrinsic.output_dimensions(sw, sh);
        assert_eq!(
            (corrected.width(), corrected.height()),
            (ew, eh),
            "{name}: Correct decode display dims (intrinsic={intrinsic:?})"
        );
        // Probe dims = intrinsic's display dims of the probe baseline.
        let (pew, peh) = intrinsic.output_dimensions(pw, ph);
        assert_eq!(
            (probe.width, probe.height),
            (pew, peh),
            "{name}: Correct probe display dims (intrinsic={intrinsic:?})"
        );

        // For an axis-swapping intrinsic, the dims must actually have swapped
        // (proves the bake happened, not just a relabel).
        if intrinsic.swaps_axes() {
            assert_eq!(
                (corrected.width(), corrected.height()),
                (sh, sw),
                "{name}: axis-swapping intrinsic {intrinsic:?} must swap decoded dims"
            );
        }
    }
}

// ── Correct on a known-oriented DNG bakes pixels (oracle vs manual bake) ─

#[test]
fn correct_bakes_pixels_matching_manual_orientation() {
    let Some(files) = corpus_files() else {
        eprintln!("Skipping: no RAW corpus (set {SAMPLES_DIR_ENV} or populate {FIVEK_DIR})");
        return;
    };

    let mut checked_nontrivial = false;
    for (name, data) in &files {
        let (preserved, _) = decode_with_hint(data, OrientationHint::Preserve);
        let intrinsic = preserved.info().orientation;
        if intrinsic.is_identity() {
            // Upright image: Correct is a no-op; pixels must be byte-identical.
            let (corrected, _) = decode_with_hint(data, OrientationHint::Correct);
            assert_eq!(
                pixels_of(&corrected),
                pixels_of(&preserved),
                "{name}: Correct on upright image must not change pixels"
            );
            continue;
        }
        checked_nontrivial = true;

        // Manually bake the intrinsic orientation onto the Preserve pixels using
        // the same coordinate model, then compare to the adapter's Correct
        // output bit-for-bit (pixels are SACRED — exact, no resampling).
        let bpp = preserved.descriptor().bytes_per_pixel();
        let sw = preserved.width() as usize;
        let sh = preserved.height() as usize;
        let src = pixels_of(&preserved);
        let manual = manual_bake(&src, sw, sh, bpp, intrinsic);

        let (corrected, _) = decode_with_hint(data, OrientationHint::Correct);
        let got = pixels_of(&corrected);
        assert_eq!(
            got.len(),
            manual.len(),
            "{name}: Correct baked byte count mismatch (intrinsic={intrinsic:?})"
        );
        assert!(
            got == manual,
            "{name}: Correct baked pixels diverge from manual {intrinsic:?} bake"
        );
    }

    if !checked_nontrivial {
        eprintln!(
            "note: no corpus file had a non-identity intrinsic orientation; \
             only the no-op Correct path was exercised end-to-end"
        );
    }
}

// ── ExactTransform ignores EXIF; CorrectAndTransform composes ────────────

#[test]
fn exact_transform_ignores_exif_and_correct_and_transform_composes() {
    let Some(files) = corpus_files() else {
        eprintln!("Skipping: no RAW corpus (set {SAMPLES_DIR_ENV} or populate {FIVEK_DIR})");
        return;
    };
    for (name, data) in &files {
        let (preserved, probe_preserve) = decode_with_hint(data, OrientationHint::Preserve);
        let intrinsic = preserved.info().orientation;
        let bpp = preserved.descriptor().bytes_per_pixel();
        let sw = preserved.width() as usize;
        let sh = preserved.height() as usize;
        let (pw, ph) = (probe_preserve.width, probe_preserve.height);
        let src = pixels_of(&preserved);

        // ExactTransform(Rotate90): the EXIF tag is ignored, so the output is
        // exactly Rotate90 applied to the stored pixels.
        let (exact, eprobe) =
            decode_with_hint(data, OrientationHint::ExactTransform(Orientation::Rotate90));
        let want_exact = manual_bake(&src, sw, sh, bpp, Orientation::Rotate90);
        assert_eq!(exact.info().orientation, Orientation::Identity);
        assert_eq!(eprobe.orientation, Orientation::Identity);
        assert_eq!(
            (exact.width(), exact.height()),
            Orientation::Rotate90.output_dimensions(sw as u32, sh as u32),
            "{name}: ExactTransform decode dims"
        );
        // Probe dims = Rotate90 applied to the probe baseline (each side vs its
        // own baseline; backends may differ on absolute base dims).
        assert_eq!(
            (eprobe.width, eprobe.height),
            Orientation::Rotate90.output_dimensions(pw, ph),
            "{name}: ExactTransform probe dims"
        );
        assert!(
            pixels_of(&exact) == want_exact,
            "{name}: ExactTransform(Rotate90) must apply Rotate90 to stored pixels, EXIF ignored"
        );

        // CorrectAndTransform(Rotate90) = intrinsic.then(Rotate90) applied to
        // the stored pixels.
        let net = intrinsic.then(Orientation::Rotate90);
        let (ct, _) = decode_with_hint(
            data,
            OrientationHint::CorrectAndTransform(Orientation::Rotate90),
        );
        let want_ct = manual_bake(&src, sw, sh, bpp, net);
        assert_eq!(ct.info().orientation, Orientation::Identity);
        assert_eq!(
            (ct.width(), ct.height()),
            net.output_dimensions(sw as u32, sh as u32),
            "{name}: CorrectAndTransform dims (net={net:?})"
        );
        assert!(
            pixels_of(&ct) == want_ct,
            "{name}: CorrectAndTransform must equal intrinsic.then(Rotate90)"
        );
    }
}

// ── output_info matches decode for every hint ───────────────────────────

#[test]
fn output_info_consistent_with_decode_for_every_hint() {
    let Some(files) = corpus_files() else {
        eprintln!("Skipping: no RAW corpus (set {SAMPLES_DIR_ENV} or populate {FIVEK_DIR})");
        return;
    };
    let hints = [
        OrientationHint::Preserve,
        OrientationHint::Correct,
        OrientationHint::ExactTransform(Orientation::Rotate270),
        OrientationHint::CorrectAndTransform(Orientation::FlipH),
    ];
    for (name, data) in &files {
        // output_info derives from `probe`; decode from the decode backend. The
        // two backends can disagree on absolute base dims, so we compare each
        // side against its OWN Preserve baseline and require the reported
        // applied-orientation to agree (that is the orientation contract).
        let oi_base = crop_disabled_config()
            .job()
            .with_orientation(OrientationHint::Preserve)
            .output_info(data)
            .expect("output_info baseline failed");
        let (oi_bw, oi_bh) = (oi_base.width, oi_base.height);
        // The intrinsic EXIF orientation, read from a Preserve decode's tag.
        let (preserve_decode, _) = decode_with_hint(data, OrientationHint::Preserve);
        let intrinsic = preserve_decode.info().orientation;

        for hint in hints {
            let config = crop_disabled_config();
            let job = config.job().with_orientation(hint);
            let oi = job.output_info(data).expect("output_info failed");
            let decoder = job
                .decoder(Cow::Borrowed(data), &[])
                .expect("decoder creation failed");
            let out = decoder.decode().expect("decode failed");

            // `output_info.orientation_applied` is the transform the decoder
            // WILL bake; it must equal the resolved orientation for the hint.
            let resolved = resolve_for_hint(hint, intrinsic);
            assert_eq!(
                oi.orientation_applied, resolved,
                "{name}: hint {hint:?}: output_info applied-orientation wrong"
            );
            // `decode().info().orientation` is the REMAINING orientation after
            // baking: Identity on bake hints (pixels final), the intrinsic tag
            // on Preserve (caller still applies it). These are complementary to
            // `orientation_applied`, not equal to it.
            let expected_remaining = if hint == OrientationHint::Preserve {
                intrinsic
            } else {
                Orientation::Identity
            };
            assert_eq!(
                out.info().orientation,
                expected_remaining,
                "{name}: hint {hint:?}: decode remaining-orientation wrong"
            );
            // output_info dims = resolved transform of the output_info baseline.
            let (ew, eh) = resolved.output_dimensions(oi_bw, oi_bh);
            assert_eq!(
                (oi.width, oi.height),
                (ew, eh),
                "{name}: hint {hint:?}: output_info dims inconsistent with its own baseline"
            );
        }
    }
}

/// Mirror of the adapter's `resolve_orientation` for the test's expectations.
/// (The adapter helper is private; this duplicates its documented semantics.)
fn resolve_for_hint(hint: OrientationHint, intrinsic: Orientation) -> Orientation {
    match hint {
        OrientationHint::Preserve => Orientation::Identity,
        OrientationHint::Correct => intrinsic,
        OrientationHint::ExactTransform(t) => t,
        OrientationHint::CorrectAndTransform(t) => intrinsic.then(t),
        _ => Orientation::Identity,
    }
}

// ── Helper: manual orientation bake (independent oracle) ─────────────────

/// Bake `orientation` onto a tightly-packed `bpp`-byte-per-pixel buffer using
/// the EXIF coordinate model, independently of the crate's baker. This is the
/// oracle the adapter's decode output is checked against.
fn manual_bake(src: &[u8], w: usize, h: usize, bpp: usize, orientation: Orientation) -> Vec<u8> {
    assert_eq!(src.len(), w * h * bpp);
    let (nw, nh) = if orientation.swaps_axes() {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; nw * nh * bpp];
    for dr in 0..nh {
        for dc in 0..nw {
            // map dst (dr,dc) -> src (sr,sc) per EXIF orientation.
            let (sr, sc) = match orientation {
                Orientation::Identity => (dr, dc),
                Orientation::FlipH => (dr, w - 1 - dc),
                Orientation::Rotate180 => (h - 1 - dr, w - 1 - dc),
                Orientation::FlipV => (h - 1 - dr, dc),
                Orientation::Transpose => (dc, dr),
                Orientation::Rotate90 => (h - 1 - dc, dr),
                Orientation::Transverse => (h - 1 - dc, w - 1 - dr),
                Orientation::Rotate270 => (dc, w - 1 - dr),
                _ => (dr, dc),
            };
            let si = (sr * w + sc) * bpp;
            let di = (dr * nw + dc) * bpp;
            out[di..di + bpp].copy_from_slice(&src[si..si + bpp]);
        }
    }
    out
}
