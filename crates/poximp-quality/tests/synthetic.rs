//! Synthetic-signal tests (fakequality.md §7).

use poximp_quality::{analyze, analyze_samples, Declared, Options, Verdict};
use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

const SECS: usize = 30;

/// Deterministic xorshift64* + Box-Muller.
struct Rng(u64);
impl Rng {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        (-2.0 * self.uniform().ln()).sqrt() * (2.0 * std::f64::consts::PI * self.uniform()).cos()
    }
}

/// Noise with magnitude response `mag(f)`, built in the frequency domain (exact brickwalls),
/// with a slow loudness envelope so it is not perfectly stationary. Peak-normalised to -3 dBFS.
fn shaped_noise(fs: u32, mag: impl Fn(f64) -> f64) -> Vec<f32> {
    let n = SECS * fs as usize;
    let ifft = RealFftPlanner::<f64>::new().plan_fft_inverse(n);
    let mut spec = ifft.make_input_vec();
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let df = fs as f64 / n as f64;
    for (k, z) in spec.iter_mut().enumerate().skip(1) {
        let a = mag(k as f64 * df);
        *z = Complex::new(rng.gauss() * a, rng.gauss() * a);
    }
    let last = spec.len() - 1;
    spec[last].im = 0.0;
    let mut out = ifft.make_output_vec();
    ifft.process(&mut spec, &mut out).unwrap();
    let env = |i: usize| 0.6 + 0.4 * (i as f64 / fs as f64 * 0.7).sin();
    let peak = out
        .iter()
        .enumerate()
        .map(|(i, x)| (x * env(i)).abs())
        .fold(0.0, f64::max);
    out.iter()
        .enumerate()
        .map(|(i, x)| (x * env(i) / peak * 0.7) as f32)
        .collect()
}

fn declared(fs: u32, lossless: bool) -> Declared {
    Declared {
        codec: if lossless { "PCM" } else { "MP3" },
        container: "test",
        bitrate_kbps: Some(320),
        sample_rate: fs,
        bit_depth: lossless.then_some(24),
        channels: 1,
        lossless,
        duration_secs: Some(SECS as f64),
        lame_lowpass_hz: None,
        he_aac: false,
    }
}

#[test]
fn brickwall_16k_is_detected() {
    let x = shaped_noise(44_100, |f| if f < 16_000.0 { 1.0 } else { 0.0 });
    let r = analyze_samples(declared(44_100, false), &x);
    let fc = r.measured.cutoff_hz.expect("cutoff");
    assert!((15_800..=16_200).contains(&fc), "fc = {fc}");
    assert!(
        r.measured.confidence > 0.8,
        "conf = {}",
        r.measured.confidence
    );
    assert_eq!(
        r.verdict,
        Verdict::FakeBitrate {
            real_kbps_estimate: 128
        }
    );
}

#[test]
fn pink_noise_gentle_lowpass_has_no_cutoff() {
    // Pink (1/sqrt f amplitude) through a 1st-order lowpass at 8 kHz.
    let x = shaped_noise(44_100, |f| {
        (1.0 / f.max(20.0)).sqrt() / (1.0 + (f / 8000.0).powi(2)).sqrt()
    });
    let r = analyze_samples(declared(44_100, true), &x);
    assert_eq!(r.measured.cutoff_hz, None);
    assert_ne!(r.verdict, Verdict::LossyOrigin { est_kbps: 128 });
}

#[test]
fn full_band_noise_has_no_cutoff() {
    let x = shaped_noise(44_100, |_| 1.0);
    let r = analyze_samples(declared(44_100, true), &x);
    assert_eq!(r.measured.cutoff_hz, None);
    assert!(
        r.measured.bandwidth_hz >= 21_500,
        "bw = {}",
        r.measured.bandwidth_hz
    );
    assert_eq!(r.spectrum.len(), poximp_quality::THUMB_POINTS);
}

#[test]
fn upsampled_96k_from_44k() {
    let x = shaped_noise(96_000, |f| if f < 22_050.0 { 1.0 } else { 0.0 });
    let r = analyze_samples(declared(96_000, true), &x);
    assert_eq!(r.measured.upsampled_from, Some(44_100), "{:?}", r.measured);
    assert!(
        matches!(
            r.verdict,
            Verdict::FakeHiResSampleRate { real_rate: 44_100 }
        ),
        "{:?}",
        r.verdict
    );
}

#[test]
fn padded_24_bit_wav_reports_16_effective_bits() {
    let fs = 44_100u32;
    let x = shaped_noise(fs, |_| 1.0);
    let path =
        std::env::temp_dir().join(format!("poximp-quality-pad24-{}.wav", std::process::id()));
    write_wav24(
        &path,
        fs,
        x.iter().map(|s| ((s * 32767.0).round() as i32) << 8),
    );
    let r = analyze(&path, &Options::default());
    std::fs::remove_file(&path).ok();
    let r = r.unwrap();
    assert_eq!(r.declared.bit_depth, Some(24));
    assert_eq!(r.measured.effective_bit_depth, Some(16));
    assert_eq!(r.verdict, Verdict::FakeHiResBitDepth { real_bits: 16 });

    // Same file with real 24-bit samples.
    write_wav24(
        &path,
        fs,
        x.iter().map(|s| (s * 8_388_607.0).round() as i32),
    );
    let r = analyze(&path, &Options::default());
    std::fs::remove_file(&path).ok();
    let r = r.unwrap();
    assert_eq!(r.measured.effective_bit_depth, Some(24));
    assert_eq!(r.verdict, Verdict::Real);
}

#[test]
fn album_majority_and_outliers() {
    let base = analyze_samples(declared(44_100, false), &shaped_noise(44_100, |_| 1.0));
    let mut fake = base.clone();
    fake.verdict = Verdict::FakeBitrate {
        real_kbps_estimate: 128,
    };
    let mut tracks = vec![fake.clone(); 4];
    tracks.push(base.clone());
    let a = poximp_quality::aggregate_album(&tracks);
    assert_eq!(a.verdict, Some(fake.verdict));
    assert_eq!(a.outliers, vec![4]);

    let mixed = vec![fake.clone(), base.clone()];
    assert_eq!(poximp_quality::aggregate_album(&mixed).verdict, None);
}

#[test]
fn features_read_the_middle_of_a_file() {
    let fs = 44_100u32;
    let x = shaped_noise(fs, |f| if f < 8_000.0 { 1.0 } else { 0.0 });
    let path = std::env::temp_dir().join(format!("poximp-features-{}.wav", std::process::id()));
    // 60 s: long enough to seek to the middle.
    write_wav24(&path, fs, x.iter().chain(&x).map(|s| (s * 8_388_607.0).round() as i32));
    let f = poximp_quality::features::features(&path);
    std::fs::remove_file(&path).ok();
    let f = f.expect("features");
    assert_eq!(f.len(), poximp_quality::features::DIMS);
    assert!(f.iter().all(|v| v.is_finite()));
}

#[test]
fn waveform_follows_the_loudness_of_a_file() {
    use poximp_quality::waveform::{waveform, BUCKETS};
    let fs = 22_050u32;
    // 12 s: quiet intro, loud middle, quiet outro (same tones, different gain).
    let tone = |i: usize| {
        let t = i as f64 / fs as f64;
        (0.6 * (std::f64::consts::TAU * 440.0 * t).sin() + 0.4 * (std::f64::consts::TAU * 3_000.0 * t).sin()) as f32
    };
    let gain = |i: usize| if (4 * fs as usize..8 * fs as usize).contains(&i) { 0.9 } else { 0.15 };
    let path = std::env::temp_dir().join(format!("poximp-waveform-{}.wav", std::process::id()));
    write_wav24(&path, fs, (0..12 * fs as usize).map(|i| (tone(i) * gain(i) * 8_388_607.0).round() as i32));
    let w = waveform(&path);
    std::fs::remove_file(&path).ok();
    let w = w.unwrap();
    assert_eq!(w.len(), 2 * BUCKETS);
    let mean = |a: f32, b: f32| {
        let s = &w[BUCKETS + (a * BUCKETS as f32) as usize..BUCKETS + (b * BUCKETS as f32) as usize];
        s.iter().map(|&v| v as f32).sum::<f32>() / s.len() as f32
    };
    let (intro, middle, outro) = (mean(0.02, 0.3), mean(0.36, 0.64), mean(0.7, 0.98));
    assert!(middle > 4.0 * intro && middle > 4.0 * outro, "rms {intro} {middle} {outro}");
    assert!(w[..BUCKETS].iter().max() == Some(&255));
}

fn write_wav24(path: &std::path::Path, fs: u32, samples: impl Iterator<Item = i32>) {
    let data: Vec<u8> = samples
        .flat_map(|s| s.to_le_bytes()[..3].to_vec())
        .collect();
    let mut w = Vec::with_capacity(44 + data.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&fs.to_le_bytes());
    w.extend_from_slice(&(fs * 3).to_le_bytes());
    w.extend_from_slice(&3u16.to_le_bytes());
    w.extend_from_slice(&24u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    std::fs::write(path, w).unwrap();
}

/// Budget check (fakequality.md §6: < 0.3 s per 4-min track). Run in release:
/// `cargo test --release -p poximp-quality -- --ignored --nocapture timing`
/// Set POXIMP_QUALITY_TIMING_FILE to time a real file (e.g. a 4-min MP3) instead of a synthetic WAV.
#[test]
#[ignore]
fn timing_4_min_track() {
    let (path, temp) = match std::env::var_os("POXIMP_QUALITY_TIMING_FILE") {
        Some(p) => (std::path::PathBuf::from(p), false),
        None => {
            let fs = 44_100u32;
            let one = shaped_noise(fs, |f| if f < 16_000.0 { 1.0 } else { 0.0 });
            let p = std::env::temp_dir()
                .join(format!("poximp-quality-timing-{}.wav", std::process::id()));
            // 8 × 30 s = 4 min, mono 24-bit.
            let it = (0..8).flat_map(|_| one.iter().map(|s| (s * 8_388_607.0).round() as i32));
            write_wav24(&p, fs, it);
            (p, true)
        }
    };
    let opts = Options::default();
    analyze(&path, &opts).unwrap(); // warm the OS file cache
    let t = std::time::Instant::now();
    let runs = 5;
    for _ in 0..runs {
        analyze(&path, &opts).unwrap();
    }
    let per = t.elapsed().as_secs_f64() / runs as f64;
    if temp {
        std::fs::remove_file(&path).ok();
    }
    println!("analyze: {:.1} ms per track", per * 1000.0);
    assert!(per < 0.3, "{per:.3} s per track");
}
