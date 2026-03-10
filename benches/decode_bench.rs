//! Benchmarks for zenraw decode pipeline.
//!
//! Requires raw sample files in `/mnt/v/input/raw-samples/`.
//! Run: `cargo bench --features rawler`

use std::fs;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use enough::Unstoppable;
use zenraw::{DemosaicMethod, RawDecodeConfig};

const SAMPLES_DIR: &str = "/mnt/v/input/raw-samples";

struct Sample {
    name: &'static str,
    file: &'static str,
}

const SAMPLES: &[Sample] = &[
    Sample {
        name: "DNG/iPhone12",
        file: "iphone12pro.dng",
    },
    Sample {
        name: "CR2/Canon350D",
        file: "canon_350d.cr2",
    },
    Sample {
        name: "NEF/NikonD40",
        file: "nikon_d40.nef",
    },
    Sample {
        name: "ARW/SonyNEX3",
        file: "sony_nex3.arw",
    },
    Sample {
        name: "RW2/PanasonicGF1",
        file: "panasonic_gf1.rw2",
    },
    Sample {
        name: "ORF/OlympusC5050",
        file: "olympus_c5050z.orf",
    },
];

fn load_sample(name: &str) -> Option<Vec<u8>> {
    let path = Path::new(SAMPLES_DIR).join(name);
    fs::read(&path).ok()
}

fn bench_full_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");
    group.sample_size(10);

    let config = RawDecodeConfig::default();

    for sample in SAMPLES {
        let Some(data) = load_sample(sample.file) else {
            eprintln!("skipping {}: file not found", sample.name);
            continue;
        };

        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("full", sample.name), &data, |b, data| {
            b.iter(|| zenraw::decode(data, &config, &Unstoppable).unwrap());
        });
    }

    group.finish();
}

fn bench_decode_gamma(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");
    group.sample_size(10);

    let config = RawDecodeConfig::default().with_gamma(true);

    // Just use the DNG — it's a good representative
    let Some(data) = load_sample("iphone12pro.dng") else {
        eprintln!("skipping gamma bench: iphone12pro.dng not found");
        return;
    };

    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("gamma", "DNG/iPhone12"),
        &data,
        |b, data| {
            b.iter(|| zenraw::decode(data, &config, &Unstoppable).unwrap());
        },
    );

    group.finish();
}

fn bench_demosaic_methods(c: &mut Criterion) {
    let mut group = c.benchmark_group("demosaic");
    group.sample_size(10);

    let Some(data) = load_sample("nikon_d40.nef") else {
        eprintln!("skipping demosaic bench: nikon_d40.nef not found");
        return;
    };

    group.throughput(Throughput::Bytes(data.len() as u64));

    for (name, method) in [
        ("bilinear", DemosaicMethod::Bilinear),
        ("malvar", DemosaicMethod::MalvarHeCutler),
    ] {
        let config = RawDecodeConfig::default().with_demosaic(method);
        group.bench_with_input(BenchmarkId::new(name, "NEF/NikonD40"), &data, |b, data| {
            b.iter(|| zenraw::decode(data, &config, &Unstoppable).unwrap());
        });
    }

    group.finish();
}

fn bench_probe(c: &mut Criterion) {
    let mut group = c.benchmark_group("probe");

    for sample in SAMPLES {
        let Some(data) = load_sample(sample.file) else {
            continue;
        };

        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("metadata", sample.name),
            &data,
            |b, data| {
                b.iter(|| zenraw::probe(data, &Unstoppable).unwrap());
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_full_decode,
    bench_decode_gamma,
    bench_demosaic_methods,
    bench_probe,
);
criterion_main!(benches);
