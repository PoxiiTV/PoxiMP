//! Print path -> verdict for every audio file in the given folders/files (calibration tool).
//! cargo run --release -p poximp-quality --example quality_scan -- <folder|file>...

use poximp_quality::{aggregate_album, analyze, Options};
use std::path::PathBuf;
use std::time::Instant;

const EXTS: [&str; 11] = [
    "mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "aiff", "aif", "wv", "ape",
];

fn main() {
    let mut files: Vec<PathBuf> = Vec::new();
    for arg in std::env::args().skip(1) {
        let p = PathBuf::from(arg);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|f| {
                    f.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
                })
                .collect();
            v.sort();
            files.extend(v);
        } else {
            files.push(p);
        }
    }
    let opts = Options::default();
    let mut reports = Vec::new();
    println!(
        "{:<52} {:>6} {:>5} {:>6} {:>3} | {:>6} {:>6} {:>5} {:>4} {:>4} {:>6} | {:>5} | verdict",
        "file", "codec", "kbps", "fs", "bit", "cut", "bw", "conf", "frm", "eff", "upFrom", "ms"
    );
    for f in &files {
        let name: String = f
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .chars()
            .take(52)
            .collect();
        let t = Instant::now();
        match analyze(f, &opts) {
            Ok(r) => {
                let ms = t.elapsed().as_millis();
                let d = &r.declared;
                let m = &r.measured;
                let opt = |v: Option<u32>| v.map_or("-".into(), |v| v.to_string());
                println!(
                    "{:<52} {:>6} {:>5} {:>6} {:>3} | {:>6} {:>6} {:>5.2} {:>4} {:>4} {:>6} | {:>5} | {:?}{}",
                    name,
                    d.codec,
                    opt(d.bitrate_kbps),
                    d.sample_rate,
                    opt(d.bit_depth),
                    opt(m.cutoff_hz),
                    m.bandwidth_hz,
                    m.confidence,
                    m.valid_frames,
                    opt(m.effective_bit_depth),
                    opt(m.upsampled_from),
                    ms,
                    r.verdict,
                    d.lame_lowpass_hz.map_or(String::new(), |lp| format!("  [LAME lp {lp}]")),
                );
                reports.push(r);
            }
            Err(e) => println!("{name:<52} ERROR {e}"),
        }
    }
    if reports.len() > 1 {
        println!("album aggregate: {:?}", aggregate_album(&reports));
    }
}
