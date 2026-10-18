//! Time one structural inventory of a crafted TIFF, for the walker's
//! resource bounds. Run under `/usr/bin/time -v` for peak memory.
//!
//! `cargo run --release --features inventory --example inventory_stress -- <mode> <n>`
//!
//! - `distinct <n>`: IFD0 with `n` inline SHORT entries of distinct tags.
//! - `bigdistinct <n>`: the same as BigTIFF (tags repeat every 65,536).
//! - `subifds <k>`: IFD0 with `k` SubIFD pointers to overlapping IFDs, each
//!   declaring 65,535 entries in a 0xFF-filled region.
//! - `subptrs <n>`: IFD0 with `n` SubIFDs entries of 1,024 pointers each, all
//!   to one empty IFD.
//! - `blowup <len>`: a `len`-byte file, 4 SubIFDs entries x 1,024 pointers to
//!   IFDs two bytes apart in a 0xFF-filled region.
//! - `overlap <n>`: `n` one-byte strips and 450 values covering all of them.
//! - `wideptrs <n> [c]`: IFD0 with `n` SubIFDs entries, each LONG[`c`]
//!   (default 131,072), all naming one shared array of `c` pointers.

use std::time::Instant;

use zencodec::decode::{DecodeJob, DecoderConfig};
use zenraw::RawDecoderConfig;

type Config = RawDecoderConfig;

fn header() -> Vec<u8> {
    b"II\x2a\x00\x08\x00\x00\x00".to_vec()
}

fn entry(b: &mut Vec<u8>, tag: u16, typ: u16, count: u32, field: u32) {
    b.extend_from_slice(&tag.to_le_bytes());
    b.extend_from_slice(&typ.to_le_bytes());
    b.extend_from_slice(&count.to_le_bytes());
    b.extend_from_slice(&field.to_le_bytes());
}

fn distinct(n: u32) -> Vec<u8> {
    let mut b = header();
    b.extend_from_slice(&(n as u16).to_le_bytes());
    for i in 0..n {
        entry(&mut b, i as u16, 3, 1, 1);
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    b
}

fn big_distinct(n: u64) -> Vec<u8> {
    let mut b = b"II\x2b\x00\x08\x00\x00\x00".to_vec();
    b.extend_from_slice(&16u64.to_le_bytes());
    b.extend_from_slice(&n.to_le_bytes());
    for i in 0..n {
        b.extend_from_slice(&((i % 65536) as u16).to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
    }
    b.extend_from_slice(&0u64.to_le_bytes());
    b
}

fn subifds(k: u32) -> Vec<u8> {
    let groups = k.div_ceil(1024);
    let arrays_at = 8 + 2 + 12 * groups + 4;
    let region = arrays_at + 4 * k;
    let region = region + (region & 1);
    let mut b = header();
    b.extend_from_slice(&(groups as u16).to_le_bytes());
    for g in 0..groups {
        entry(
            &mut b,
            330,
            4,
            (k - g * 1024).min(1024),
            arrays_at + 4 * 1024 * g,
        );
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    for i in 0..k {
        b.extend_from_slice(&(region + 2 * i).to_le_bytes());
    }
    b.resize(region as usize, 0);
    b.resize(region as usize + 2 + 65535 * 12 + 4 + 2 * k as usize, 0xFF);
    b
}

fn subptrs(n: u32) -> Vec<u8> {
    let arr = 8 + 2 + 12 * n + 4;
    let target = arr + 4096;
    let mut b = header();
    b.extend_from_slice(&(n as u16).to_le_bytes());
    for _ in 0..n {
        entry(&mut b, 330, 4, 1024, arr);
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..1024 {
        b.extend_from_slice(&target.to_le_bytes());
    }
    b.extend_from_slice(&[0; 6]);
    b
}

fn blowup(len: usize) -> Vec<u8> {
    let arrays_at = 8 + 2 + 4 * 12 + 4;
    let region = arrays_at + 4 * 4096;
    let mut b = header();
    b.extend_from_slice(&4u16.to_le_bytes());
    for k in 0..4u32 {
        entry(&mut b, 330, 4, 1024, arrays_at + k * 4096);
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    for k in 0..4096u32 {
        b.extend_from_slice(&(region + 2 * k).to_le_bytes());
    }
    b.resize(len.max(b.len()), 0xFF);
    b
}

fn overlap(n: u32) -> Vec<u8> {
    let mut b = header();
    let region = b.len() as u32;
    b.resize(b.len() + 2 * n as usize, 0);
    let offs_at = b.len() as u32;
    for k in 0..n {
        b.extend_from_slice(&(region + 2 * k).to_le_bytes());
    }
    let cnts_at = b.len() as u32;
    for _ in 0..n {
        b.extend_from_slice(&1u16.to_le_bytes());
    }
    let ifd1 = b.len() as u32;
    b.extend_from_slice(&450u16.to_le_bytes());
    for i in 0..450u16 {
        entry(&mut b, 60000 + i, 7, 2 * n, region);
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    let ifd0 = b.len() as u32;
    b.extend_from_slice(&4u16.to_le_bytes());
    entry(&mut b, 256, 3, 1, 4);
    entry(&mut b, 257, 3, 1, 4);
    entry(&mut b, 273, 4, n, offs_at);
    entry(&mut b, 279, 3, n, cnts_at);
    b.extend_from_slice(&ifd1.to_le_bytes());
    b[4..8].copy_from_slice(&ifd0.to_le_bytes());
    b
}

fn wideptrs(n: u32, c: u32) -> Vec<u8> {
    let arr = 8 + 2 + 12 * n + 4;
    let target = arr + 4 * c;
    let mut b = header();
    b.extend_from_slice(&(n as u16).to_le_bytes());
    for _ in 0..n {
        entry(&mut b, 330, 4, c, arr);
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..c {
        b.extend_from_slice(&target.to_le_bytes());
    }
    b.extend_from_slice(&[0; 6]);
    b
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(n)) = (args.get(1), args.get(2).and_then(|s| s.parse::<u64>().ok()))
    else {
        eprintln!(
            "usage: inventory_stress <distinct|bigdistinct|subifds|subptrs|blowup|overlap|wideptrs> <n> [c]"
        );
        std::process::exit(2);
    };
    let data = match mode.as_str() {
        "distinct" => distinct(n as u32),
        "bigdistinct" => big_distinct(n),
        "subifds" => subifds(n as u32),
        "subptrs" => subptrs(n as u32),
        "blowup" => blowup(n as usize),
        "overlap" => overlap(n as u32),
        "wideptrs" => wideptrs(
            n as u32,
            args.get(3).and_then(|s| s.parse().ok()).unwrap_or(131_072),
        ),
        _ => {
            eprintln!("unknown mode {mode}");
            std::process::exit(2);
        }
    };
    let t = Instant::now();
    let inv = Config::new().job().inventory(&data);
    let el = t.elapsed().as_secs_f64();
    match inv {
        Ok(inv) => println!(
            "mode={mode} n={n} input={} bytes parts={} valid={} inventory_time={el:.3}s",
            data.len(),
            inv.parts().len(),
            inv.validate().is_ok(),
        ),
        Err(e) => println!(
            "mode={mode} n={n} input={} bytes: error {e} ({el:.3}s)",
            data.len()
        ),
    }
}
