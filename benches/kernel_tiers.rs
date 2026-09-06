//! Paired runtime-tier normalization, including allocation and exact output checks.
//! The scalar tier may auto-vectorize on AArch64.

use zenbench::prelude::*;

#[cfg(target_arch = "aarch64")]
type TierToken = archmage::NeonToken;
#[cfg(target_arch = "x86_64")]
type TierToken = archmage::X64V3Token;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
const TIER_NAME: &str = if cfg!(target_arch = "aarch64") {
    "neon"
} else {
    "v3(avx2)"
};

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn set_simd(on: bool) -> bool {
    TierToken::dangerously_disable_token_process_wide(!on).is_ok()
}
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn set_simd(_on: bool) -> bool {
    false
}

fn bench(suite: &mut Suite) {
    assert!(
        set_simd(true) && set_simd(false),
        "SIMD tier must be toggleable"
    );
    set_simd(true);

    for (label, n) in [
        ("17", 17usize),
        ("64x64", 64 * 64),
        ("256x256", 256 * 256),
        ("1024x1024", 1024 * 1024),
        ("4096x4096", 4096 * 4096),
        ("24MP", 24 << 20),
    ] {
        let data: &'static [f32] = Box::leak(
            (0..n)
                .map(|i| (i % 4096) as f32)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        assert!(set_simd(false));
        let scalar = zenraw::simd::normalize_uniform(data, 512.0, 1.0 / 3583.0);
        assert!(set_simd(true));
        let simd = zenraw::simd::normalize_uniform(data, 512.0, 1.0 / 3583.0);
        assert!(
            scalar
                .iter()
                .zip(&simd)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        drop((scalar, simd));
        suite.compare(format!("normalize_uniform/{label}"), move |g| {
            g.throughput(Throughput::Bytes((n * 4) as u64));
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                g.bench(arm, move |b| {
                    b.with_input(move || assert!(set_simd(simd)))
                        .run(move |_| zenraw::simd::normalize_uniform(data, 512.0, 1.0 / 3583.0))
                });
            }
        });
    }
    set_simd(true);
}

// Whole-decode cells are explicitly enabled by the caller with a fixture path.
// Backend selection remains Cargo feature controlled (rawloader by default;
// rawler when that feature is enabled).
fn bench_decode(suite: &mut Suite) {
    let Ok(path) = std::env::var("ZENRAW_BENCH_INPUT") else {
        return;
    };
    let data: &'static [u8] = Box::leak(
        std::fs::read(&path)
            .expect("RAW fixture")
            .into_boxed_slice(),
    );
    let backend = if cfg!(feature = "rawler") {
        "rawler"
    } else {
        "rawloader"
    };
    for (name, mode) in [
        ("develop", zenraw::OutputMode::Develop),
        ("linear", zenraw::OutputMode::Linear),
        ("camera_raw", zenraw::OutputMode::CameraRaw),
    ] {
        let config = zenraw::RawDecodeConfig::default().with_output(mode);
        assert!(set_simd(false));
        let scalar = zenraw::decode(data, &config, &enough::Unstoppable).unwrap();
        assert!(set_simd(true));
        let simd = zenraw::decode(data, &config, &enough::Unstoppable).unwrap();
        assert_eq!(scalar.pixels.width(), simd.pixels.width());
        assert_eq!(scalar.pixels.height(), simd.pixels.height());
        assert_eq!(
            scalar.pixels.as_contiguous_bytes().unwrap(),
            simd.pixels.as_contiguous_bytes().unwrap()
        );
        let pixels = u64::from(simd.pixels.width()) * u64::from(simd.pixels.height());
        eprintln!(
            "{backend}/{name}: {}x{} exact tier pixels, fixture={path}",
            simd.pixels.width(),
            simd.pixels.height()
        );
        drop((scalar, simd));
        suite.compare(format!("decode/{backend}/{name}"), move |g| {
            g.throughput(Throughput::Elements(pixels));
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                let config = config.clone();
                g.bench(arm, move |b| {
                    let config = config.clone();
                    b.with_input(move || assert!(set_simd(simd)))
                        .run(move |_| zenraw::decode(data, &config, &enough::Unstoppable).unwrap())
                });
            }
        });
    }
    assert!(set_simd(true));
}

zenbench::main!(bench, bench_decode);
