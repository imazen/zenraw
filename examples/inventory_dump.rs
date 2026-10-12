//! Print the structural inventory of each RAW file named on the command line.
//!
//! `cargo run --features inventory --example inventory_dump -- a.nef b.dng`

use zencodec::decode::{DecodeJob, DecoderConfig};
use zenraw::RawDecoderConfig;

fn main() {
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).expect("read input");
        let job = RawDecoderConfig::new().job();
        match job.inventory(&data) {
            Ok(inv) => {
                println!("== {path}");
                print!("{inv}");
                if let Err(e) = inv.validate() {
                    println!("INVALID: {e}");
                }
                for (name, bytes) in inv.bytes_by_disposition() {
                    println!("  {name}: {bytes} bytes");
                }
            }
            Err(e) => println!("== {path}: error {e}"),
        }
    }
}
