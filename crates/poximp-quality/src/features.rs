//! Sound features for radio and mixes (analysis pass 2): a small vector per track from
//! `EXCERPT_SECS` in the middle. Cheap on purpose: tempo, loudness, timbre (spectral shape) and a
//! key-free chroma. Values are raw; the library z-scores them over the whole collection.

use crate::decode;
use realfft::RealFftPlanner;
use std::path::Path;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::SeekMode;

/// Length of the vector `features` returns.
pub const DIMS: usize = 20;
const EXCERPT_SECS: f64 = 30.0;
/// Tracks shorter than this are read from the start.
const MIDDLE_FROM_SECS: f64 = EXCERPT_SECS + 10.0;
const DISCARD_AFTER_SEEK_SECS: f64 = 0.05;
/// Frames quieter than this are left out of the spectral means.
const SILENT_DB: f32 = -60.0;
/// Fewer usable frames (~1 s): not enough to describe the track.
const MIN_FRAMES: usize = 40;
const BPM_MIN: f32 = 60.0;
const BPM_MAX: f32 = 200.0;
const CHROMA_LO_HZ: f32 = 55.0;
const CHROMA_HI_HZ: f32 = 5000.0;
const BAND_LO_HZ: f32 = 50.0;
const BAND_HI_HZ: f32 = 16_000.0;

/// `None` = unreadable, too short or silent.
pub fn features(path: &Path) -> Option<Vec<f32>> {
    let mut src = decode::open(path).ok()?;
    let fs = src.fs;
    let mut skip = 0usize;
    // From the middle; if the file cannot seek, from the start.
    let seeked = src
        .duration
        .filter(|d| *d >= MIDDLE_FROM_SECS)
        .and_then(|d| poximp_codecs::seek_to(src.track_id, src.time_base, fs, (d - EXCERPT_SECS) / 2.0))
        .is_some_and(|to| src.format.seek(SeekMode::Coarse, to).is_ok());
    if seeked {
        src.decoder.reset();
        skip = (DISCARD_AFTER_SEEK_SECS * fs as f64) as usize;
    }
    let want = (EXCERPT_SECS * fs as f64) as usize;
    let mut mono = Vec::with_capacity(want);
    let mut planes: Vec<Vec<f32>> = Vec::new();
    while mono.len() < want {
        let packet = match src.format.next_packet() {
            Ok(Some(p)) => p,
            _ => break,
        };
        if packet.track_id != src.track_id {
            continue;
        }
        let buf = match src.decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => continue,
            Err(_) => break,
        };
        buf.copy_to_vecs_planar::<f32>(&mut planes);
        let len = buf.frames();
        let from = skip.min(len);
        skip -= from;
        let n = planes.len().max(1) as f32;
        let to = (from + want - mono.len()).min(len);
        mono.extend((from..to).map(|i| planes.iter().map(|p| p[i]).sum::<f32>() / n));
    }
    from_samples(fs, &mono)
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    }
}

/// Tempo (BPM) and pulse clarity (0..1) from an onset envelope at `rate` frames per second:
/// the strongest autocorrelation lag, with a mild preference for ~120 BPM (how people tap).
fn tempo(onset: &[f32], rate: f32) -> (f32, f32) {
    let m = mean(onset);
    let o: Vec<f32> = onset.iter().map(|x| x - m).collect();
    let ac = |lag: usize| o.iter().zip(&o[lag.min(o.len())..]).map(|(a, b)| a * b).sum::<f32>();
    let r0 = ac(0);
    let (lo, hi) = ((60.0 * rate / BPM_MAX).floor() as usize, (60.0 * rate / BPM_MIN).ceil() as usize);
    if r0 <= 0.0 || hi + 1 >= o.len() || lo < 1 {
        return (120.0, 0.0);
    }
    let weight = |lag: f32| {
        let octaves = (60.0 * rate / lag / 120.0).log2();
        (-0.5 * octaves * octaves).exp()
    };
    let best = (lo..=hi).max_by(|&a, &b| (ac(a) * weight(a as f32)).total_cmp(&(ac(b) * weight(b as f32)))).unwrap_or(lo);
    // Parabolic peak between lags for a finer BPM.
    let (y0, y1, y2) = (ac(best - 1), ac(best), ac(best + 1));
    let d = y0 - 2.0 * y1 + y2;
    let shift = if d < 0.0 { (0.5 * (y0 - y2) / d).clamp(-0.5, 0.5) } else { 0.0 };
    let bpm = 60.0 * rate / (best as f32 + shift);
    (bpm.clamp(BPM_MIN, BPM_MAX), (y1 / r0).clamp(0.0, 1.0))
}

/// The vector for mono samples at `fs` Hz. `None` when there are not enough audible frames.
pub fn from_samples(fs: u32, x: &[f32]) -> Option<Vec<f32>> {
    // ~46 ms frames at any rate, half overlap: about 43 frames per second.
    let n = ((fs as f32 * 0.046) as usize).next_power_of_two().max(256);
    let hop = n / 2;
    if x.len() < n * 2 {
        return None;
    }
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
    let (mut input, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
    let win: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
    let bin_hz = fs as f32 / n as f32;
    let bin = |hz: f32| ((hz / bin_hz) as usize).min(n / 2);
    let (lo, hi) = (bin(BAND_LO_HZ).max(1), bin(BAND_HI_HZ.min(fs as f32 / 2.0)));
    // Pitch class of each bin (C = 0), only where notes are well resolved.
    let class: Vec<Option<usize>> = (0..=n / 2)
        .map(|k| {
            let f = k as f32 * bin_hz;
            (CHROMA_LO_HZ..=CHROMA_HI_HZ).contains(&f).then(|| ((12.0 * (f / 440.0).log2()).round() as i32 + 9).rem_euclid(12) as usize)
        })
        .collect();

    let mut prev = vec![0f32; n / 2 + 1];
    let (mut onset, mut loud) = (Vec::new(), Vec::new());
    let (mut centroid, mut rolloff, mut flatness) = (Vec::new(), Vec::new(), Vec::new());
    let mut chroma = [0f32; 12];
    for frame in x.windows(n).step_by(hop) {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        let db = 20.0 * rms.max(1e-9).log10();
        for ((i, s), w) in input.iter_mut().zip(frame).zip(&win) {
            *i = s * w;
        }
        if fft.process(&mut input, &mut spec).is_err() {
            return None;
        }
        let mag: Vec<f32> = spec.iter().map(|c| c.norm()).collect();
        onset.push((lo..hi).map(|k| ((1.0 + 100.0 * mag[k]).ln() - (1.0 + 100.0 * prev[k]).ln()).max(0.0)).sum::<f32>() / (hi - lo).max(1) as f32);
        prev.copy_from_slice(&mag);
        if db < SILENT_DB {
            continue;
        }
        let pow: Vec<f32> = mag[lo..hi].iter().map(|m| m * m).collect();
        let total: f32 = pow.iter().sum();
        if total <= 1e-12 {
            continue;
        }
        loud.push(db);
        let c = pow.iter().enumerate().map(|(i, p)| (lo + i) as f32 * bin_hz * p).sum::<f32>() / total;
        centroid.push((c / 1000.0).max(1e-3).log2());
        let mut acc = 0.0;
        let roll = pow.iter().position(|p| {
            acc += p;
            acc >= 0.85 * total
        });
        rolloff.push(((lo + roll.unwrap_or(0)) as f32 * bin_hz / 1000.0).max(1e-3).log2());
        let geo = (pow.iter().map(|p| (p + 1e-12).ln()).sum::<f32>() / pow.len() as f32).exp();
        flatness.push((geo / (total / pow.len() as f32)).max(1e-6).log10());
        for (k, m) in mag.iter().enumerate() {
            if let Some(c) = class[k] {
                chroma[c] += m;
            }
        }
    }
    if loud.len() < MIN_FRAMES {
        return None;
    }
    let (bpm, clarity) = tempo(&onset, fs as f32 / hop as f32);
    let loud_mean = mean(&loud);
    let loud_std = (loud.iter().map(|d| (d - loud_mean).powi(2)).sum::<f32>() / loud.len() as f32).sqrt();
    // Key-free: rotated so the strongest pitch class comes first (keeps the major/minor shape).
    let sum: f32 = chroma.iter().sum::<f32>().max(1e-9);
    let top = (0..12).max_by(|&a, &b| chroma[a].total_cmp(&chroma[b])).unwrap_or(0);
    let mut out = vec![(bpm / 120.0).log2(), clarity, loud_mean, loud_std, mean(&centroid), mean(&rolloff), mean(&flatness), mean(&onset)];
    out.extend((0..12).map(|i| chroma[(top + i) % 12] / sum));
    debug_assert_eq!(out.len(), DIMS);
    out.iter().all(|v| v.is_finite()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: u32 = 44_100;

    fn noise(len: usize, seed: u64) -> Vec<f32> {
        let mut s = seed | 1;
        (0..len)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
            })
            .collect()
    }

    fn sine(hz: f32, secs: f32) -> Vec<f32> {
        (0..(secs * FS as f32) as usize).map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / FS as f32).sin()).collect()
    }

    /// Noise bursts (fast decay) every 60/bpm seconds over a quiet bed.
    fn clicks(bpm: f32, secs: f32) -> Vec<f32> {
        let period = (60.0 / bpm * FS as f32) as usize;
        let n = noise((secs * FS as f32) as usize, 9);
        n.iter().enumerate().map(|(i, s)| s * (0.02 + 0.9 * (-((i % period) as f32) / (0.01 * FS as f32)).exp())).collect()
    }

    #[test]
    fn tempo_of_a_click_track() {
        for bpm in [90.0, 120.0, 150.0] {
            let f = from_samples(FS, &clicks(bpm, 20.0)).unwrap();
            let got = 120.0 * 2f32.powf(f[0]);
            assert!((got - bpm).abs() < bpm * 0.04, "{bpm}: {got}");
            assert!(f[1] > 0.3, "clear pulse: {}", f[1]);
        }
    }

    #[test]
    fn timbre_and_chroma() {
        let a = from_samples(FS, &sine(440.0, 10.0)).unwrap();
        let n = from_samples(FS, &noise(10 * FS as usize, 3)).unwrap();
        assert_eq!((a.len(), n.len()), (DIMS, DIMS));
        assert!(a[8] > 0.6, "one pitch class dominates: {:?}", &a[8..]);
        assert!(n[8] < 0.2, "noise has no key: {:?}", &n[8..]);
        assert!(n[4] > a[4] + 2.0, "noise is brighter: {} vs {}", n[4], a[4]);
        assert!(n[6] > a[6] + 1.0, "noise is flatter: {} vs {}", n[6], a[6]);
        assert!((a[2] - 20.0 * (0.5f32 / 2f32.sqrt()).log10()).abs() < 0.5, "loudness in dBFS: {}", a[2]);
        assert!(a[3] < 0.5, "steady level: {}", a[3]);
    }

    #[test]
    fn silence_and_short_give_nothing() {
        assert!(from_samples(FS, &vec![0.0; 10 * FS as usize]).is_none());
        assert!(from_samples(FS, &sine(440.0, 0.05)).is_none());
        assert!(features(Path::new("/definitely/not/here.flac")).is_none());
    }
}
