//! Time the seekbar waveform of a file and dump its bytes (peaks, then RMS) for a quick look.
//! cargo run --release -p poximp-quality --example waveform -- <file> [out.bin]

use std::time::Instant;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(file) = args.next() else {
        eprintln!("usage: waveform <file> [out.bin]");
        std::process::exit(2);
    };
    let _ = std::fs::read(&file); // warm the OS file cache
    let t = Instant::now();
    let w = poximp_quality::waveform::waveform(file.as_ref()).expect("decode");
    println!("{} bytes in {:.0} ms", w.len(), t.elapsed().as_secs_f64() * 1000.0);
    if let Some(out) = args.next() {
        std::fs::write(out, &w).expect("write");
    }
}
