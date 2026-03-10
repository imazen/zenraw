//! Profile a single DNG decode for callgrind/flamegraph.
//!
//! Usage:
//!   cargo build --release --features rawler --example profile_dng
//!   valgrind --tool=callgrind --callgrind-out-file=/tmp/zenraw-callgrind.out \
//!     target/release/examples/profile_dng /mnt/v/input/raw-samples/iphone12pro.dng

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/mnt/v/input/raw-samples/iphone12pro.dng".into());
    let data = std::fs::read(&path).expect("failed to read file");
    eprintln!("loaded {} bytes from {}", data.len(), path);

    let config = zenraw::RawDecodeConfig::default();
    let out = zenraw::decode(&data, &config, &enough::Unstoppable).expect("decode failed");
    eprintln!(
        "decoded {}x{} {} ({})",
        out.info.width, out.info.height, out.info.model, out.info.cfa_pattern
    );
}
