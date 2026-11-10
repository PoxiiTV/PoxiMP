//! PoxiMP real-quality detector. No Tauri dependency; tested with synthetic signals.
//! Algorithm: docs/investigacion/fakequality.md §3–§5 + corrections in critica.md §A.
//!
//! - `decode`   sampled decode with its own symphonia reader (pass 1, fast) + LAME tag
//! - `spectrum` realfft + Hann, 100 Hz band medians, cliff (fc), confidence
//! - `features` small per-track sound vector for radio / mixes (pass 2)
//! - `waveform` seekbar peaks of the whole file
//! - here       per-track verdict + per-album aggregation
//!
//! Wording rule: lossy files never get a positive verdict, at most `NoEvidence`;
//! `Real` only for lossless with full bandwidth. Caching (path, size, mtime) is the caller's job.

mod decode;
pub mod features;
mod spectrum;
pub mod waveform;

use serde::Serialize;
use std::path::Path;

pub use spectrum::THUMB_POINTS;

// Verdict thresholds (calibrated on the corpus, see examples/scan.rs).
/// Cliff confidence needed to call a file fake.
const CONF_FAKE: f32 = 0.70;
/// Cliff confidence for "suspicious".
const CONF_SUSPECT: f32 = 0.45;
/// A cliff this far under the expected encoder cutoff is a transcode.
const FAKE_MARGIN_HZ: f32 = 700.0;
/// Measured edge sits this far under the LAME tag lowpass; V0 (lp 24 k) is capped.
const LAME_LP_TO_EDGE_HZ: f32 = 400.0;
const LAME_MAX_EXPECTED_HZ: f32 = 20_000.0;
/// Fewer valid frames than this: not enough music to judge.
const MIN_VALID_FRAMES: u32 = 50;
/// Skip the cutoff verdict when Nyquist is below this (MP3 at 32/24/22.05 kHz).
const MIN_NYQUIST_HZ: u32 = 17_000;
/// Lossless with a cliff at or under this = lossy origin.
const LOSSY_ORIGIN_MAX_HZ: f32 = 19_500.0;
/// Cliff at or above this counts as full band (ADC / mastering filters).
const FULL_BAND_HZ: f32 = 20_800.0;
/// Opus (CELT fullband) codes up to 20.0 kHz and always decodes at 48 kHz (measured 20.07–20.08 k).
/// LAME 320 measures 20.1 k too, so this is only used for 48 kHz lossless files.
const OPUS_CUTOFF_HZ: f32 = 20_050.0;
const OPUS_TOLERANCE_HZ: f32 = 150.0;
const OPUS_RATE: u32 = 48_000;
const OPUS_MIN_CONF: f32 = 0.80;
/// Upsampling: cliff within [rate/2 · LO, rate/2 · HI] of a lower standard rate.
const UPSAMPLE_LO: f32 = 0.88;
const UPSAMPLE_HI: f32 = 1.03;
const STANDARD_RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];
/// Album verdict needs this share of the judged tracks to agree.
const ALBUM_MAJORITY: f32 = 0.6;

/// Measured cutoff → equivalent bitrate (upper bound of each row, kbps). Built from measured
/// cutoffs (critica.md §A.1), corpus: LAME 128 16.5–16.7 k, AAC 128 17.25 k, Vorbis q3 17.3–18.0 k,
/// LAME 192/V2 18.6–18.7 k, LAME 256 19.4 k, LAME 320 20.1 k. Older encoders per fakequality.md §3.7.
const CUTOFF_LABELS: [(f32, u32); 8] = [
    (11_000.0, 64),
    (13_500.0, 80),
    (15_300.0, 96),
    (17_600.0, 128),
    (18_400.0, 160),
    (19_100.0, 192),
    (19_800.0, 256),
    (f32::MAX, 320),
];

#[derive(Debug, Clone)]
pub struct Options {
    /// Max number of sampled segments for long tracks.
    pub max_segments: usize,
    /// Length of each sampled segment.
    pub segment_secs: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_segments: 24,
            segment_secs: 3.0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported format or codec")]
    Unsupported,
    #[error("no audio track")]
    NoAudioTrack,
}

/// What the file claims to be.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Declared {
    pub codec: &'static str,
    pub container: &'static str,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: u32,
    pub bit_depth: Option<u32>,
    pub channels: u32,
    pub lossless: bool,
    pub duration_secs: Option<f64>,
    /// Lowpass written in the MP3 LAME/Info tag (last encode).
    pub lame_lowpass_hz: Option<u32>,
    /// HE-AAC (SBR): the cutoff says nothing reliable.
    pub he_aac: bool,
}

/// What the audio actually contains.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    /// Hard (codec-like) cutoff, if any.
    pub cutoff_hz: Option<u32>,
    pub bandwidth_hz: u32,
    /// Confidence of the cutoff; without cutoff, how much usable signal there was (0..1).
    pub confidence: f32,
    pub valid_frames: u32,
    /// Bits actually used by integer samples (lossless only).
    pub effective_bit_depth: Option<u32>,
    pub upsampled_from: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Verdict {
    /// Lossless with full bandwidth (the only positive verdict).
    Real,
    /// Lossy file consistent with its bitrate (never "genuine").
    NoEvidence,
    /// Lossy file whose bandwidth matches a lower bitrate.
    FakeBitrate {
        real_kbps_estimate: u32,
    },
    /// Lossless container with lossy content.
    LossyOrigin {
        est_kbps: u32,
    },
    FakeHiResBitDepth {
        real_bits: u32,
    },
    FakeHiResSampleRate {
        real_rate: u32,
    },
    /// Clean cliff at 20.0 kHz: probably decoded from Opus.
    ProbableOpusOrigin,
    Suspicious,
    Inconclusive,
    /// Nyquist too low for the cutoff method.
    LowSampleRate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub declared: Declared,
    pub measured: Measured,
    pub verdict: Verdict,
    /// Averaged spectrum 0..fs/2, `THUMB_POINTS` values, 0 = 96 dB under the peak.
    pub spectrum: Vec<u8>,
}

/// Analyse a file with its own decoder (never share it with playback).
pub fn analyze(path: &Path, opts: &Options) -> Result<Report, Error> {
    let d = decode::decode(path, opts)?;
    Ok(build_report(d.declared, &d.spectrum, d.or_bits))
}

/// Analyse already-decoded interleaved samples (tests, or callers with their own decode).
/// `declared.sample_rate` and `declared.channels` describe the buffer.
pub fn analyze_samples(declared: Declared, interleaved: &[f32]) -> Report {
    let ch = declared.channels.max(1) as usize;
    let mut acc = spectrum::Accumulator::new(declared.sample_rate, ch);
    let planes: Vec<Vec<f32>> = (0..ch.min(2))
        .map(|c| interleaved.iter().skip(c).step_by(ch).copied().collect())
        .collect();
    let refs: Vec<&[f32]> = planes.iter().map(|p| p.as_slice()).collect();
    acc.push(&refs);
    let or_bits = declared.lossless.then(|| {
        interleaved
            .iter()
            .fold(0, |a, &s| a | decode::float_bits(s as f64))
    });
    build_report(declared, &acc.finish(), or_bits)
}

fn build_report(declared: Declared, sp: &spectrum::Spectrum, or_bits: Option<u32>) -> Report {
    let a = spectrum::analyse(sp);
    let fc = a.cliff.as_ref().map(|c| c.fc);
    let conf = a.cliff.as_ref().map_or(0.0, |c| c.conf);
    let fs = declared.sample_rate;
    let upsampled_from = fc
        .filter(|_| conf >= CONF_FAKE && fs > 50_000)
        .and_then(|fc| {
            // Lowest standard rate whose Nyquist window holds the cliff.
            // ponytail: 44.1 vs 48 k overlap at 21.1–22.7 k resolves to 44.1 k.
            STANDARD_RATES.into_iter().filter(|&r| r < fs).find(|&r| {
                (r as f32 / 2.0 * UPSAMPLE_LO..=r as f32 / 2.0 * UPSAMPLE_HI).contains(&fc)
            })
        });
    let measured = Measured {
        cutoff_hz: fc.map(|f| f.round() as u32),
        bandwidth_hz: a.bandwidth_hz.round() as u32,
        confidence: a.confidence,
        valid_frames: sp.valid as u32,
        effective_bit_depth: or_bits.filter(|&o| o != 0).map(|o| 32 - o.trailing_zeros()),
        upsampled_from,
    };
    let verdict = judge(&declared, &measured, fc, conf);
    Report {
        declared,
        measured,
        verdict,
        spectrum: a.thumb,
    }
}

fn judge(d: &Declared, m: &Measured, fc: Option<f32>, conf: f32) -> Verdict {
    if d.sample_rate / 2 < MIN_NYQUIST_HZ {
        return Verdict::LowSampleRate;
    }
    if m.valid_frames < MIN_VALID_FRAMES || d.he_aac {
        return Verdict::Inconclusive;
    }
    let near_opus = |f: f32| {
        d.sample_rate == OPUS_RATE
            && (f - OPUS_CUTOFF_HZ).abs() <= OPUS_TOLERANCE_HZ
            && conf >= OPUS_MIN_CONF
    };

    if d.lossless {
        if let Some(f) = fc {
            if f <= LOSSY_ORIGIN_MAX_HZ {
                return match conf {
                    c if c >= CONF_FAKE => Verdict::LossyOrigin {
                        est_kbps: kbps_for_cutoff(f),
                    },
                    c if c >= CONF_SUSPECT => Verdict::Suspicious,
                    _ => Verdict::Inconclusive,
                };
            }
            if near_opus(f) {
                return Verdict::ProbableOpusOrigin;
            }
        }
        if let (Some(bits), Some(eff)) = (d.bit_depth, m.effective_bit_depth) {
            if bits > 16 && eff <= 16 {
                return Verdict::FakeHiResBitDepth { real_bits: eff };
            }
        }
        if let Some(rate) = m.upsampled_from {
            return Verdict::FakeHiResSampleRate { real_rate: rate };
        }
        return match fc {
            // Ambiguous zone (LAME 320, AAC 256, Vorbis q5… but also 20 kHz masters): never "fake".
            Some(f) if f < FULL_BAND_HZ && conf >= CONF_FAKE => Verdict::Suspicious,
            Some(f) if f < FULL_BAND_HZ && conf >= CONF_SUSPECT => Verdict::Inconclusive,
            _ => Verdict::Real,
        };
    }

    // Lossy.
    let Some(f) = fc else {
        return Verdict::NoEvidence;
    };
    // An MP3 320 made from Opus (YouTube rips) measures the same 20.1 k as a genuine LAME 320:
    // not detectable by cutoff (critica.md §A.4), so it stays NoEvidence.
    let expected = expected_cutoff(d);
    if f < expected - FAKE_MARGIN_HZ {
        if conf >= CONF_FAKE {
            return Verdict::FakeBitrate {
                real_kbps_estimate: kbps_for_cutoff(f),
            };
        }
        if conf >= CONF_SUSPECT {
            return Verdict::Suspicious;
        }
    }
    Verdict::NoEvidence
}

/// Lowest cutoff a genuine encode at the declared bitrate shows (measured, not encoder lowpass:
/// LAME 128 has lowpass 17 k but measures 16.5–16.7 k).
fn expected_cutoff(d: &Declared) -> f32 {
    let kbps = d.bitrate_kbps.unwrap_or(0);
    // ponytail: bitrate-only tables; per-encoder tables (FDK vs FFmpeg AAC) if mislabels show up.
    let table: &[(u32, f32)] = match d.codec {
        "MP3" => &[
            (300, 20_000.0),
            (240, 19_400.0),
            (180, 18_600.0),
            (150, 17_500.0),
            (112, 16_000.0),
        ],
        // FDK-AAC 192/256 can cut at 17 k (fakequality.md §5.6), FFmpeg AAC 128 measures 17.25 k.
        "AAC" => &[(160, 16_500.0), (112, 15_000.0)],
        "Vorbis" => &[
            (180, 19_500.0),
            (150, 19_000.0),
            (120, 17_500.0),
            (96, 16_500.0),
        ],
        _ => &[],
    };
    let by_rate = table
        .iter()
        .find(|(k, _)| kbps >= *k)
        .map_or(0.0, |(_, hz)| *hz);
    // The tag holds the lowpass of the last encode; the measured edge sits a bit under it
    // (LAME 128: lp 17.0 k → 16.5–16.7 k; 320: lp 20.5 k → 20.1 k).
    d.lame_lowpass_hz.map_or(by_rate, |lp| {
        (lp as f32 - LAME_LP_TO_EDGE_HZ).min(LAME_MAX_EXPECTED_HZ)
    })
}

/// Equivalent bitrate label (kbps) for a measured cutoff.
pub fn kbps_for_cutoff(fc: f32) -> u32 {
    CUTOFF_LABELS
        .iter()
        .find(|(hz, _)| fc < *hz)
        .map_or(320, |(_, k)| *k)
}

/// Album-level aggregation (critica.md §A.8): one rip is judged far more reliably than one track.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumVerdict {
    /// Majority verdict, or `None` when tracks disagree (mixed) or none could be judged.
    pub verdict: Option<Verdict>,
    pub tracks: usize,
    /// Tracks that were not Inconclusive / LowSampleRate.
    pub judged: usize,
    /// Indices (into the input) of judged tracks that disagree with the album verdict.
    pub outliers: Vec<usize>,
}

pub fn aggregate_album(reports: &[Report]) -> AlbumVerdict {
    let judged: Vec<(usize, Verdict)> = reports
        .iter()
        .enumerate()
        .map(|(i, r)| (i, r.verdict))
        .filter(|(_, v)| !matches!(v, Verdict::Inconclusive | Verdict::LowSampleRate))
        .collect();
    let same_kind =
        |a: &Verdict, b: &Verdict| std::mem::discriminant(a) == std::mem::discriminant(b);
    // Most common kind, then the most common exact verdict inside it.
    let best = judged
        .iter()
        .max_by_key(|(_, v)| judged.iter().filter(|(_, w)| same_kind(v, w)).count())
        .map(|(_, v)| *v);
    let verdict = best.filter(|b| {
        let n = judged.iter().filter(|(_, w)| same_kind(b, w)).count();
        n as f32 >= judged.len() as f32 * ALBUM_MAJORITY
    });
    let verdict = verdict.map(|b| {
        judged
            .iter()
            .filter(|(_, w)| same_kind(&b, w))
            .max_by_key(|(_, v)| judged.iter().filter(|(_, w)| w == v).count())
            .map_or(b, |(_, v)| *v)
    });
    let outliers = match verdict {
        Some(v) => judged
            .iter()
            .filter(|(_, w)| !same_kind(&v, w))
            .map(|(i, _)| *i)
            .collect(),
        None => Vec::new(),
    };
    AlbumVerdict {
        verdict,
        tracks: reports.len(),
        judged: judged.len(),
        outliers,
    }
}
