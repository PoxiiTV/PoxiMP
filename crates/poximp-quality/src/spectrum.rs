//! STFT accumulation (Hann, hop = N, 100 Hz bands with per-band median) and cliff detection.
//! fakequality.md §3.2–§3.4.

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const BAND_HZ: f32 = 100.0;
/// Power floor so silent bins give a finite (very low) dB value.
const EPS: f32 = 1e-20;

// Frame validity (§3.2).
const MIN_FRAME_RMS_DB: f32 = -50.0;
/// The 2–8 kHz bands must be within this many dB of the loudest band (excludes bass-only frames).
const MAX_MID_BELOW_PEAK_DB: f32 = 50.0;
/// Bands whose level barely moves over time are stationary noise (noise-shaped dither), not content.
const STATIONARY_STD_DB: f32 = 1.5;

// Cliff detection thresholds (§3.3).
const DROP_DB: f32 = 20.0;
const STEEP_DB: f32 = 12.0;
const BACK_DB: f32 = 15.0;
const MAX_BELOW_UNDER_REF_DB: f32 = 45.0;
/// Lowest frequency the cliff search goes down to.
const SEARCH_LO_HZ: f32 = 10_000.0;
/// Highest cliff we look for, as distance below fs/2 (needs some "above" bands to compare).
const SEARCH_TOP_MARGIN_HZ: f32 = 800.0;
/// Per-frame consistency: frame counts if it has signal under the cliff and drops at least this much.
const FRAME_SIGNAL_OVER_FLOOR_DB: f32 = 20.0;
const FRAME_DROP_DB: f32 = 15.0;
/// fc = where the spectrum falls this far under the plateau just below the cliff.
const EDGE_DB: f32 = 15.0;
/// How far under the first passing band to look for the steepest point of the slope.
const SLOPE_SEARCH_BANDS: usize = 8;
/// A cliff that falls all the way to the noise floor (within this) counts as a full drop.
const FLOOR_TOLERANCE_DB: f32 = 6.0;
/// Content considered "present" for the bandwidth figure when there is no cliff.
const BANDWIDTH_UNDER_REF_DB: f32 = 70.0;
/// Frames needed for full confidence.
const FULL_CONF_FRAMES: f32 = 300.0;

/// Number of points in the UI spectrum thumbnail (0..fs/2).
pub const THUMB_POINTS: usize = 64;
const THUMB_RANGE_DB: f32 = 96.0;

pub fn fft_size(fs: u32) -> usize {
    match fs {
        0..=48_000 => 4096,
        48_001..=96_000 => 8192,
        _ => 16384,
    }
}

pub struct Accumulator {
    n: usize,
    fs: u32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// Time-domain buffers, max 2 channels.
    buf: [Vec<f32>; 2],
    nch: usize,
    fill: usize,
    input: Vec<f32>,
    out: Vec<Complex<f32>>,
    power: Vec<f32>,
    band_edges: Vec<usize>,
    tmp: Vec<f32>,
    frame_db: Vec<f32>,
    /// Valid frames × bands, dB.
    frames_db: Vec<f32>,
    band_lin_sum: Vec<f64>,
    bin_lin_sum: Vec<f64>,
    valid: usize,
}

pub struct Spectrum {
    pub fs: u32,
    pub nbands: usize,
    /// dB of mean linear power per band over valid frames.
    pub s: Vec<f32>,
    /// 90th percentile of band dB over frames.
    pub h: Vec<f32>,
    /// Temporal std of band dB.
    pub v: Vec<f32>,
    /// Mean power per FFT bin (dB) and bin width.
    pub bins_db: Vec<f32>,
    pub df: f32,
    pub frames_db: Vec<f32>,
    pub valid: usize,
}

impl Accumulator {
    pub fn new(fs: u32, nch: usize) -> Self {
        let n = fft_size(fs);
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
        let window = (0..n)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos())
            .collect();
        let df = fs as f32 / n as f32;
        let nbands = (fs as f32 / 2.0 / BAND_HZ) as usize;
        let nbins = n / 2 + 1;
        let band_edges = (0..=nbands)
            .map(|b| ((b as f32 * BAND_HZ / df) as usize).min(nbins))
            .collect();
        Accumulator {
            n,
            fs,
            input: fft.make_input_vec(),
            out: fft.make_output_vec(),
            fft,
            window,
            buf: [vec![0.0; n], vec![0.0; n]],
            nch: nch.clamp(1, 2),
            fill: 0,
            power: vec![0.0; nbins],
            band_edges,
            tmp: Vec::with_capacity(64),
            frame_db: vec![0.0; nbands],
            frames_db: Vec::new(),
            band_lin_sum: vec![0.0; nbands],
            bin_lin_sum: vec![0.0; nbins],
            valid: 0,
        }
    }

    /// Drop the partial frame (call at segment boundaries so frames never straddle a seek).
    pub fn reset_segment(&mut self) {
        self.fill = 0;
    }

    /// Feed planar samples (only the first two planes are used).
    pub fn push(&mut self, planes: &[&[f32]]) {
        let len = planes.first().map_or(0, |p| p.len());
        let mut i = 0;
        while i < len {
            let take = (self.n - self.fill).min(len - i);
            for c in 0..self.nch {
                let src = planes.get(c).unwrap_or(&planes[0]);
                self.buf[c][self.fill..self.fill + take].copy_from_slice(&src[i..i + take]);
            }
            self.fill += take;
            i += take;
            if self.fill == self.n {
                self.process_frame();
                self.fill = 0;
            }
        }
    }

    fn process_frame(&mut self) {
        let n = self.n;
        let mut sq = 0.0f64;
        self.power.iter_mut().for_each(|p| *p = 0.0);
        for c in 0..self.nch {
            for (k, x) in self.buf[c].iter().enumerate() {
                sq += (*x as f64) * (*x as f64);
                self.input[k] = x * self.window[k];
            }
            if self.fft.process(&mut self.input, &mut self.out).is_err() {
                return;
            }
            let norm = 1.0 / (n as f32 * n as f32);
            for (p, z) in self.power.iter_mut().zip(&self.out) {
                *p += z.norm_sqr() * norm;
            }
        }
        let rms_db = 10.0 * ((sq / (n * self.nch) as f64).max(1e-30)).log10() as f32;

        let mut peak = f32::MIN;
        for b in 0..self.frame_db.len() {
            let (lo, hi) = (
                self.band_edges[b],
                self.band_edges[b + 1].max(self.band_edges[b] + 1),
            );
            self.tmp.clear();
            self.tmp
                .extend_from_slice(&self.power[lo..hi.min(self.power.len())]);
            let m = median(&mut self.tmp);
            self.frame_db[b] = db(m);
            peak = peak.max(self.frame_db[b]);
        }
        let mid = mean(&self.frame_db[band(2000.0)..band(8000.0).min(self.frame_db.len())]);
        if rms_db < MIN_FRAME_RMS_DB || mid < peak - MAX_MID_BELOW_PEAK_DB {
            return;
        }
        self.valid += 1;
        self.frames_db.extend_from_slice(&self.frame_db);
        for (s, d) in self.band_lin_sum.iter_mut().zip(&self.frame_db) {
            *s += 10f64.powf(*d as f64 / 10.0);
        }
        for (s, p) in self.bin_lin_sum.iter_mut().zip(&self.power) {
            *s += *p as f64;
        }
    }

    pub fn finish(self) -> Spectrum {
        let nb = self.frame_db.len();
        let nf = self.valid.max(1) as f64;
        let s = self
            .band_lin_sum
            .iter()
            .map(|x| db((x / nf) as f32))
            .collect();
        let mut h = vec![0.0; nb];
        let mut v = vec![0.0; nb];
        let mut col = Vec::with_capacity(self.valid);
        for b in 0..nb {
            col.clear();
            col.extend((0..self.valid).map(|f| self.frames_db[f * nb + b]));
            if col.is_empty() {
                h[b] = db(0.0);
                continue;
            }
            let m = mean(&col);
            v[b] = (col.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / col.len() as f32).sqrt();
            h[b] = percentile(&mut col, 0.9);
        }
        Spectrum {
            fs: self.fs,
            nbands: nb,
            s,
            h,
            v,
            bins_db: self
                .bin_lin_sum
                .iter()
                .map(|x| db((x / nf) as f32))
                .collect(),
            df: self.fs as f32 / self.n as f32,
            frames_db: self.frames_db,
            valid: self.valid,
        }
    }
}

pub struct Cliff {
    pub fc: f32,
    pub conf: f32,
}

pub struct Analysis {
    pub cliff: Option<Cliff>,
    pub bandwidth_hz: f32,
    /// Cliff confidence, or the data-amount factor when there is no cliff.
    pub confidence: f32,
    pub thumb: Vec<u8>,
}

pub fn analyse(sp: &Spectrum) -> Analysis {
    let nb = sp.nbands;
    let s = &sp.s;
    let thumb = thumbnail(s);
    let c_frames = (sp.valid as f32 / FULL_CONF_FRAMES).clamp(0.0, 1.0);
    if sp.valid == 0 || nb < band(SEARCH_LO_HZ) + 20 {
        return Analysis {
            cliff: None,
            bandwidth_hz: 0.0,
            confidence: 0.0,
            thumb,
        };
    }
    let reference = median(&mut s[band(2000.0)..band(8000.0)].to_vec());
    let top_n = (nb * 3 / 100).max(2);
    let floor = percentile(&mut s[nb - top_n..].to_vec(), 0.1);

    let bands_hz = |b: usize| b as f32 * BAND_HZ;
    // (below, above, drop, steep) for an edge at band b.
    let shape = |b: usize| {
        let below = mean(&s[b - 10..b]);
        let above = mean(&s[b + 3..(b + 13).min(nb)]);
        (
            below,
            above,
            below - above,
            s[b - 3] - s[(b + 3).min(nb - 1)],
        )
    };
    let top = nb.saturating_sub((SEARCH_TOP_MARGIN_HZ / BAND_HZ) as usize);
    let lo = band(SEARCH_LO_HZ);
    let found = (lo..=top).rev().find(|&b| {
        let (below, _, drop, steep) = shape(b);
        let back = (b + 3..nb.saturating_sub(2).max(b + 4))
            .filter(|&k| k < nb && sp.v[k] >= STATIONARY_STD_DB)
            .map(|k| sp.h[k])
            .fold(f32::MIN, f32::max);
        drop >= DROP_DB
            && steep >= STEEP_DB
            && back <= below - BACK_DB
            && below >= reference - MAX_BELOW_UNDER_REF_DB
    });

    let Some(first) = found else {
        let bw = (0..nb)
            .rev()
            .find(|&k| s[k] >= reference - BANDWIDTH_UNDER_REF_DB)
            .unwrap_or(0);
        return Analysis {
            cliff: None,
            bandwidth_hz: bands_hz(bw + 1).min(sp.fs as f32 / 2.0),
            confidence: c_frames,
            thumb,
        };
    };
    // The first band that passes sits somewhere on the slope: move to the steepest point.
    let b = (first.saturating_sub(SLOPE_SEARCH_BANDS).max(lo)..=(first + 3).min(top))
        .max_by(|&x, &y| (s[x - 2] - s[x + 2]).total_cmp(&(s[y - 2] - s[y + 2])))
        .unwrap_or(first);
    let (below, above, drop, steep) = shape(b);

    // Refine to bin resolution: fc = where the (3-bin smoothed) spectrum falls EDGE_DB under the
    // plateau just below the cliff. Independent of how deep the floor above is (16-bit vs float).
    let plateau = mean(&s[b - 12..b - 4]);
    let bin = |hz: f32| ((hz / sp.df) as usize).min(sp.bins_db.len() - 2).max(1);
    let (blo, bhi) = (bin(bands_hz(b) - 600.0), bin(bands_hz(b) + 600.0));
    let smooth = |k: usize| (sp.bins_db[k - 1] + sp.bins_db[k] + sp.bins_db[k + 1]) / 3.0;
    let edge = (blo..=bhi)
        .rev()
        .find(|&k| smooth(k) >= plateau - EDGE_DB)
        .unwrap_or(blo);
    let fc = (edge as f32 + 0.5) * sp.df;

    // Per-frame consistency at fc (a codec is a wall in every frame; natural roll-off is not).
    let fb = band(fc).clamp(10, nb - 4);
    let (mut with_signal, mut dropping) = (0usize, 0usize);
    for f in 0..sp.valid {
        let fr = &sp.frames_db[f * nb..(f + 1) * nb];
        let bf = mean(&fr[fb - 10..fb]);
        let af = mean(&fr[fb + 3..(fb + 13).min(nb)]);
        if bf >= floor + FRAME_SIGNAL_OVER_FLOOR_DB {
            with_signal += 1;
            if bf - af >= FRAME_DROP_DB {
                dropping += 1;
            }
        }
    }
    let c_consist = if with_signal == 0 {
        0.0
    } else {
        dropping as f32 / with_signal as f32
    };
    // A 16-bit decode of a lossy file has its floor ~20–30 dB under the HF content: the drop
    // cannot be deeper than that, so reaching the floor is as good as a 40 dB drop.
    let c_drop = if above <= floor + FLOOR_TOLERANCE_DB {
        1.0
    } else {
        ((drop - 15.0) / 25.0).clamp(0.0, 1.0)
    };
    let c_steep = ((steep / 0.6 - 10.0) / 30.0).clamp(0.0, 1.0);
    let c_signal = ((below - (reference - 50.0)) / 20.0).clamp(0.0, 1.0);
    let conf = (0.30 * c_drop + 0.20 * c_steep + 0.30 * c_consist + 0.20 * c_signal) * c_frames;

    Analysis {
        cliff: Some(Cliff { fc, conf }),
        bandwidth_hz: fc,
        confidence: conf,
        thumb,
    }
}

fn thumbnail(s: &[f32]) -> Vec<u8> {
    if s.is_empty() {
        return vec![0; THUMB_POINTS];
    }
    let max = s.iter().copied().fold(f32::MIN, f32::max);
    (0..THUMB_POINTS)
        .map(|i| {
            let lo = i * s.len() / THUMB_POINTS;
            let hi = ((i + 1) * s.len() / THUMB_POINTS).max(lo + 1).min(s.len());
            let v = mean(&s[lo..hi]);
            (((v - (max - THUMB_RANGE_DB)) / THUMB_RANGE_DB).clamp(0.0, 1.0) * 255.0) as u8
        })
        .collect()
}

fn band(hz: f32) -> usize {
    (hz / BAND_HZ) as usize
}

fn db(p: f32) -> f32 {
    10.0 * (p + EPS).log10()
}

fn mean(x: &[f32]) -> f32 {
    if x.is_empty() {
        return db(0.0);
    }
    x.iter().sum::<f32>() / x.len() as f32
}

fn median(x: &mut [f32]) -> f32 {
    percentile(x, 0.5)
}

fn percentile(x: &mut [f32], p: f32) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    let k = ((x.len() - 1) as f32 * p).round() as usize;
    *x.select_nth_unstable_by(k, f32::total_cmp).1
}
