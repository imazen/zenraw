//! Micro-benchmark for `is_dng_data` and XMP extraction paths.
//!
//! Run: `cargo run --release --example bench_byte_scan -- <path-to-dng>`
//!
//! Measures the SIMD-accelerated memchr scans against an inlined scalar
//! reference implementation (copied from the pre-memchr versions).

use std::env;
use std::fs;
use std::time::Instant;

fn is_dng_scalar(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }
    let is_tiff = (data[0] == b'I' && data[1] == b'I' && data[2] == 42 && data[3] == 0)
        || (data[0] == b'M' && data[1] == b'M' && data[2] == 0 && data[3] == 42);
    if !is_tiff {
        return false;
    }
    let search_len = data.len().min(4096);
    let le = data[0] == b'I';
    for i in 0..search_len.saturating_sub(1) {
        if le {
            if data[i] == 0x12 && data[i + 1] == 0xC6 {
                return true;
            }
        } else if data[i] == 0xC6 && data[i + 1] == 0x12 {
            return true;
        }
    }
    false
}

fn is_dng_memchr(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }
    let is_tiff = (data[0] == b'I' && data[1] == b'I' && data[2] == 42 && data[3] == 0)
        || (data[0] == b'M' && data[1] == b'M' && data[2] == 0 && data[3] == 42);
    if !is_tiff {
        return false;
    }
    let search_len = data.len().min(4096);
    let haystack = &data[..search_len];
    let needle: &[u8] = if data[0] == b'I' {
        &[0x12, 0xC6]
    } else {
        &[0xC6, 0x12]
    };
    memchr::memmem::find(haystack, needle).is_some()
}

fn find_scalar(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn find_memchr(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    memchr::memmem::find(haystack, needle)
}

fn bench<F: FnMut() -> R, R>(label: &str, iters: u32, mut f: F) -> u128 {
    // warmup
    for _ in 0..iters / 10 {
        std::hint::black_box(f());
    }
    let t = Instant::now();
    for _ in 0..iters {
        std::hint::black_box(f());
    }
    let ns = t.elapsed().as_nanos();
    let per = ns / iters as u128;
    println!("  {label:<24} {per:>8} ns/call ({iters} iters)");
    per
}

fn main() {
    let path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/mnt/v/input/fivek/dng/a0001-jmac_DSC1459.dng".to_string());

    let data = fs::read(&path).expect("read file");
    println!("File: {path}");
    println!("Size: {:.2} MB", data.len() as f64 / 1_048_576.0);
    println!();

    println!("is_dng_data (4KB header scan):");
    let s1 = bench("scalar (byte loop)", 100_000, || is_dng_scalar(&data));
    let s2 = bench("memchr::memmem", 100_000, || is_dng_memchr(&data));
    println!("  => speedup: {:.2}x", s1 as f64 / s2 as f64);
    println!();

    println!("find_bytes (full-file XMP marker search):");
    // Typical XMP markers
    let begin = b"<?xpacket begin";
    let end = b"</x:xmpmeta>";

    let s3 = bench("scalar windows (begin)", 1_000, || {
        find_scalar(&data, begin)
    });
    let s4 = bench("memchr::memmem (begin)", 1_000, || {
        find_memchr(&data, begin)
    });
    println!("  => speedup: {:.2}x", s3 as f64 / s4 as f64);

    let s5 = bench("scalar windows (end)", 1_000, || find_scalar(&data, end));
    let s6 = bench("memchr::memmem (end)", 1_000, || find_memchr(&data, end));
    println!("  => speedup: {:.2}x", s5 as f64 / s6 as f64);

    // Worst-case: marker near the end of the buffer (force full scan)
    println!();
    println!("find_bytes (no match, must scan entire file):");
    let nomatch = b"ZZ_NEVER_PRESENT_MARKER_ZZ";
    let s7 = bench("scalar windows (nomatch)", 100, || {
        find_scalar(&data, nomatch)
    });
    let s8 = bench("memchr::memmem (nomatch)", 1_000, || {
        find_memchr(&data, nomatch)
    });
    println!("  => speedup: {:.2}x", s7 as f64 / s8 as f64);
}
