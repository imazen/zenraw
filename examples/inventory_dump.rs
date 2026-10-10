//! Print the structural inventory of each RAW file named on the command line.
//!
//! `cargo run --features zencodec --example inventory_dump -- a.nef b.dng`

use zencodec::decode::{DecodeJob, DecoderConfig};
use zenraw::RawDecoderConfig;

fn main() {
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).expect("read input");
        let job = RawDecoderConfig::new().job();
        match job.inventory(&data) {
            Ok(Some(inv)) => {
                println!("== {path}");
                print!("{inv}");
                if let Err(e) = inv.validate() {
                    println!("INVALID: {e}");
                }
                for (name, bytes) in inv.bytes_by_disposition() {
                    println!("  {name}: {bytes} bytes");
                }
            }
            Ok(None) => println!("== {path}: no inventory"),
            Err(e) => println!("== {path}: error {e}"),
        }
    }
}
