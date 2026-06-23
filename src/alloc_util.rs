//! Allocation helpers honoring the allocation-fallibility preference per call
//! site.
//!
//! A RAW/DNG decode mixes two allocation regimes:
//!
//! * **Big, untrusted-sized buffers** — the demosaic / color-pipeline output
//!   `RGB f32` buffer and the normalized sensor buffer, all sized from the
//!   sensor dimensions the file claims. A malicious header can demand gigabytes,
//!   so we want a graceful [`RawError::LimitExceeded`] rather than an abort.
//!   (The dimensions are *also* bounds-checked up front by
//!   [`enforce_decode_limits`](crate::decode::enforce_decode_limits), but the
//!   fallible path keeps the actual allocation graceful regardless.)
//! * **Small, bounded scratch** — per-tile / crop-row copies whose size is
//!   bounded by one row or one neighbourhood, not attacker-controlled in any
//!   unbounded way. These default to the *infallible* `vec!` path — a single
//!   `calloc` (for the zeroed case) is faster.
//!
//! [`AllocPref`] is a **3-mode, per-site override** of that default:
//! [`Fallible`](AllocPref::Fallible) / [`Infallible`](AllocPref::Infallible)
//! force one path everywhere; [`CodecDefault`](AllocPref::CodecDefault) keeps
//! each site's own default. The helper signatures therefore take the caller's
//! preference *and* the site default, and resolve them together.
//!
//! The carrier is a **crate-local** enum rather than `zencodec::AllocPreference`
//! so the alloc sites compile without the optional `zencodec` feature (the
//! direct [`decode`](crate::decode) API leaves it
//! [`CodecDefault`](AllocPref::CodecDefault), so behaviour is unchanged). At the
//! zencodec decode boundary, [`zencodec::AllocPreference`] is mapped onto this
//! type (see the `From` impl below, gated on the `zencodec` feature).
//!
//! The helpers are generic over the element type `T` because RAW decode buffers
//! are predominantly `Vec<f32>` (linear / scene-referred pixels) rather than the
//! `Vec<u8>` byte buffers a byte-oriented codec allocates.

use alloc::vec;
use alloc::vec::Vec;
use whereat::{At, at};

use crate::error::RawError;

/// Caller preference for allocation fallibility, resolved per call site.
///
/// Mirrors `zencodec::AllocPreference` but is defined crate-locally so the
/// untrusted-buffer alloc sites can honour it whether or not the optional
/// `zencodec` feature is enabled. The direct [`decode`](crate::decode) API
/// leaves it [`CodecDefault`](Self::CodecDefault) (each site keeps its own
/// default); the zencodec adapter sets it from
/// `ResourceLimits::prefer_fallible_allocations`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AllocPref {
    /// Each site keeps its own default (big untrusted buffers fallible, small
    /// bounded scratch infallible). Default — preserves existing behaviour.
    #[default]
    CodecDefault,
    /// Force the fallible (`try_reserve`) path everywhere — graceful OOM error.
    Fallible,
    /// Force the infallible (`vec!` / `with_capacity`) path everywhere — faster,
    /// aborts on OOM.
    Infallible,
}

#[cfg(feature = "zencodec")]
impl From<zencodec::AllocPreference> for AllocPref {
    fn from(p: zencodec::AllocPreference) -> Self {
        match p {
            zencodec::AllocPreference::Fallible => AllocPref::Fallible,
            zencodec::AllocPreference::Infallible => AllocPref::Infallible,
            // `CodecDefault` and any future `#[non_exhaustive]` variant keep the
            // per-site defaults.
            _ => AllocPref::CodecDefault,
        }
    }
}

/// Resolve the 3-mode [`AllocPref`] against THIS site's default fallibility.
///
/// * [`Fallible`](AllocPref::Fallible) → always `true`.
/// * [`Infallible`](AllocPref::Infallible) → always `false`.
/// * [`CodecDefault`](AllocPref::CodecDefault) → the site default, unchanged.
#[inline]
#[must_use]
pub(crate) fn resolve_fallible(pref: AllocPref, site_default_fallible: bool) -> bool {
    match pref {
        AllocPref::Fallible => true,
        AllocPref::Infallible => false,
        AllocPref::CodecDefault => site_default_fallible,
    }
}

/// Allocate `n` elements all equal to `fill`, honoring the per-site fallibility.
///
/// `pref` is the caller's [`AllocPref`]; `site_default_fallible` is this site's
/// default when `pref` is [`CodecDefault`](AllocPref::CodecDefault).
///
/// * fallible → `try_reserve_exact` then `resize`, returning
///   [`RawError::LimitExceeded`] on allocation failure.
/// * infallible → `vec![fill; n]` (single `calloc` when `fill` is the zero
///   bit-pattern; aborts on OOM).
pub(crate) fn alloc_filled<T: Clone>(
    pref: AllocPref,
    site_default_fallible: bool,
    fill: T,
    n: usize,
) -> Result<Vec<T>, At<RawError>> {
    if resolve_fallible(pref, site_default_fallible) {
        let mut v = Vec::new();
        v.try_reserve_exact(n).map_err(|_| {
            at!(RawError::LimitExceeded(alloc::format!(
                "out of memory allocating {n} elements"
            )))
        })?;
        v.resize(n, fill);
        Ok(v)
    } else {
        Ok(vec![fill; n])
    }
}

/// Allocate an empty `Vec<T>` with reserved capacity for `cap` elements,
/// honoring the per-site fallibility (for the `Vec::with_capacity` + push/extend
/// sites).
///
/// `pref` is the caller's [`AllocPref`]; `site_default_fallible` is this site's
/// default when `pref` is [`CodecDefault`](AllocPref::CodecDefault).
///
/// * fallible → `try_reserve_exact`, returning [`RawError::LimitExceeded`] on
///   allocation failure.
/// * infallible → `Vec::with_capacity(cap)` (aborts on OOM).
///
/// The returned `Vec` is empty (length 0); the caller fills it.
pub(crate) fn vec_with_capacity<T>(
    pref: AllocPref,
    site_default_fallible: bool,
    cap: usize,
) -> Result<Vec<T>, At<RawError>> {
    if resolve_fallible(pref, site_default_fallible) {
        let mut v = Vec::new();
        v.try_reserve_exact(cap).map_err(|_| {
            at!(RawError::LimitExceeded(alloc::format!(
                "out of memory allocating {cap} elements"
            )))
        })?;
        Ok(v)
    } else {
        Ok(Vec::with_capacity(cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `CodecDefault` keeps each site's own default fallibility.

    #[test]
    fn codec_default_keeps_site_default_true() {
        // Big-buffer site (default fallible): CodecDefault stays fallible.
        assert!(resolve_fallible(AllocPref::CodecDefault, true));
    }

    #[test]
    fn codec_default_keeps_site_default_false() {
        // Small-scratch site (default infallible): CodecDefault stays infallible.
        assert!(!resolve_fallible(AllocPref::CodecDefault, false));
    }

    #[test]
    fn explicit_fallible_overrides_any_site_default() {
        assert!(resolve_fallible(AllocPref::Fallible, false));
        assert!(resolve_fallible(AllocPref::Fallible, true));
    }

    #[test]
    fn explicit_infallible_overrides_any_site_default() {
        assert!(!resolve_fallible(AllocPref::Infallible, true));
        assert!(!resolve_fallible(AllocPref::Infallible, false));
    }

    #[test]
    fn alloc_filled_all_modes_equal_bytes() {
        // f32 zero-fill (the RAW decode's dominant buffer element).
        let a = alloc_filled(AllocPref::CodecDefault, true, 0.0f32, 4096).unwrap();
        let b = alloc_filled(AllocPref::Infallible, true, 0.0f32, 4096).unwrap();
        let c = alloc_filled(AllocPref::Fallible, false, 0.0f32, 4096).unwrap();
        assert_eq!(a.len(), 4096);
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert!(a.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn alloc_filled_nonzero_fill() {
        let v = alloc_filled(AllocPref::Fallible, true, 0.5f32, 8).unwrap();
        assert_eq!(v, alloc::vec![0.5f32; 8]);
    }

    #[test]
    fn vec_with_capacity_reserves_and_is_empty() {
        let a: Vec<f32> = vec_with_capacity(AllocPref::Infallible, false, 1024).unwrap();
        let b: Vec<f32> = vec_with_capacity(AllocPref::Fallible, false, 1024).unwrap();
        assert_eq!(a.len(), 0);
        assert_eq!(b.len(), 0);
        assert!(a.capacity() >= 1024);
        assert!(b.capacity() >= 1024);
    }

    #[test]
    fn alloc_filled_fallible_oom_returns_err() {
        // Request an impossibly large allocation; the fallible path must
        // return Err (mapped to LimitExceeded) rather than abort.
        let r = alloc_filled(AllocPref::Fallible, true, 0.0f32, usize::MAX / 8);
        assert!(r.is_err());
        assert!(matches!(
            r.unwrap_err().decompose().0,
            RawError::LimitExceeded(_)
        ));
    }

    #[test]
    fn vec_with_capacity_fallible_oom_returns_err() {
        let r: Result<Vec<f32>, _> = vec_with_capacity(AllocPref::Fallible, true, usize::MAX / 8);
        assert!(r.is_err());
        assert!(matches!(
            r.unwrap_err().decompose().0,
            RawError::LimitExceeded(_)
        ));
    }

    #[cfg(feature = "zencodec")]
    #[test]
    fn from_zencodec_alloc_preference_maps_all_modes() {
        assert_eq!(
            AllocPref::from(zencodec::AllocPreference::CodecDefault),
            AllocPref::CodecDefault
        );
        assert_eq!(
            AllocPref::from(zencodec::AllocPreference::Fallible),
            AllocPref::Fallible
        );
        assert_eq!(
            AllocPref::from(zencodec::AllocPreference::Infallible),
            AllocPref::Infallible
        );
    }
}
