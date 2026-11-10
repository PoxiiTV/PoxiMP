//! Producer-side DSP, before the ring: gain (preamp + normalization) -> parametric EQ (RBJ biquads,
//! up to 10 stereo bands) -> effects (`fx`) -> safety limiter. Settings changes glide over ~20 ms (no zipper noise);
//! a flat, unity, unarmed chain does not touch the samples at all (bit-transparent).

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::fx::{Fx, FxSettings};

pub const MAX_BANDS: usize = 10;
/// Biquad parameters are re-derived every this many frames while gliding.
const BLOCK: usize = 32;
/// One-pole time constant of every glide: ~99 % of the way in 20 ms.
const GLIDE_SECS: f64 = 0.0045;
/// Limiter ceiling (-0.3 dBFS) and release.
const CEILING: f64 = 0.966;
const RELEASE_SECS: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Peak,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

impl Kind {
    fn has_gain(self) -> bool {
        !matches!(self, Kind::LowPass | Kind::HighPass)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub kind: Kind,
    /// Hz
    pub freq: f64,
    /// dB (ignored by low/high-pass)
    pub gain: f64,
    pub q: f64,
}

/// A band that does nothing: also the padding up to `MAX_BANDS`.
const FLAT: Band = Band { kind: Kind::Peak, freq: 1000.0, gain: 0.0, q: 1.0 };

impl Band {
    fn is_identity(&self) -> bool {
        self.kind.has_gain() && self.gain == 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EqSettings {
    /// false = bypass
    pub enabled: bool,
    /// dB, used when `auto_preamp` is off
    pub preamp: f64,
    /// Preamp = -(max boost of the curve), so the EQ never pushes a full-scale tone into clipping.
    pub auto_preamp: bool,
    pub bands: Vec<Band>,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self { enabled: true, preamp: 0.0, auto_preamp: true, bands: Vec::new() }
    }
}

fn clamp(v: f64, lo: f64, hi: f64, def: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        def
    }
}

impl EqSettings {
    /// Nothing from outside is trusted: ranges every biquad is stable with.
    pub fn sanitize(&mut self) {
        self.preamp = clamp(self.preamp, -30.0, 12.0, 0.0);
        self.bands.truncate(MAX_BANDS);
        for b in &mut self.bands {
            b.freq = clamp(b.freq, 20.0, 20_000.0, 1000.0);
            b.gain = if b.kind.has_gain() { clamp(b.gain, -24.0, 24.0, 0.0) } else { 0.0 };
            b.q = clamp(b.q, 0.1, 20.0, 1.0);
        }
    }

    /// Highest point of the curve in dB (0 for a flat or cut-only curve), at 48 kHz.
    pub fn max_boost(&self) -> f64 {
        let bands: Vec<Band> = self.bands.iter().filter(|b| !b.is_identity()).copied().collect();
        if bands.is_empty() {
            return 0.0;
        }
        let coefs: Vec<Biquad> = bands.iter().map(|b| Biquad::design(b, 48_000.0)).collect();
        // A log grid plus every band centre, so narrow peaks are not missed.
        let grid = (0..240).map(|i| 20.0 * 1000f64.powf(f64::from(i) / 239.0)).chain(bands.iter().map(|b| b.freq));
        grid.map(|f| coefs.iter().map(|c| c.response_db(f, 48_000.0)).sum::<f64>()).fold(0.0, f64::max)
    }

    /// The preamp that is applied (dB).
    pub fn effective_preamp(&self) -> f64 {
        if self.auto_preamp {
            -self.max_boost()
        } else {
            self.preamp
        }
    }
}

/// RBJ cookbook biquad, transposed direct form II.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self { b: [b[0] / a[0], b[1] / a[0], b[2] / a[0]], a: [a[1] / a[0], a[2] / a[0]] }
    }

    pub(crate) fn design(band: &Band, fs: f64) -> Self {
        // Keep the centre below Nyquist whatever the device rate.
        let w = 2.0 * PI * band.freq.min(0.45 * fs) / fs;
        let (c, al) = (w.cos(), w.sin() / (2.0 * band.q));
        let a = 10f64.powf(band.gain / 40.0);
        let sq = 2.0 * a.sqrt() * al;
        match band.kind {
            Kind::Peak => Self::new([1.0 + al * a, -2.0 * c, 1.0 - al * a], [1.0 + al / a, -2.0 * c, 1.0 - al / a]),
            Kind::LowShelf => Self::new(
                [a * (a + 1.0 - (a - 1.0) * c + sq), 2.0 * a * (a - 1.0 - (a + 1.0) * c), a * (a + 1.0 - (a - 1.0) * c - sq)],
                [a + 1.0 + (a - 1.0) * c + sq, -2.0 * (a - 1.0 + (a + 1.0) * c), a + 1.0 + (a - 1.0) * c - sq],
            ),
            Kind::HighShelf => Self::new(
                [a * (a + 1.0 + (a - 1.0) * c + sq), -2.0 * a * (a - 1.0 + (a + 1.0) * c), a * (a + 1.0 + (a - 1.0) * c - sq)],
                [a + 1.0 - (a - 1.0) * c + sq, 2.0 * (a - 1.0 - (a + 1.0) * c), a + 1.0 - (a - 1.0) * c - sq],
            ),
            Kind::LowPass => Self::new([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al]),
            Kind::HighPass => Self::new([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al]),
        }
    }

    fn response_db(&self, f: f64, fs: f64) -> f64 {
        let w = 2.0 * PI * f / fs;
        let mag = |k: [f64; 3]| {
            let re = k[0] + k[1] * w.cos() + k[2] * (2.0 * w).cos();
            let im = -(k[1] * w.sin() + k[2] * (2.0 * w).sin());
            re * re + im * im
        };
        10.0 * (mag(self.b) / mag([1.0, self.a[0], self.a[1]])).max(1e-30).log10()
    }

    #[inline]
    pub(crate) fn tick(&self, z: &mut [f64; 2], x: f64) -> f64 {
        let y = self.b[0] * x + z[0];
        z[0] = self.b[1] * x - self.a[0] * y + z[1];
        z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

pub(crate) fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

struct Slot {
    cur: Band,
    target: Band,
    coef: Biquad,
    /// Filter state per channel (L, R).
    z: [[f64; 2]; 2],
}

/// The chain. Owned by the producer thread; nothing here runs in the audio callback.
pub(crate) struct Dsp {
    rate: f64,
    slots: Vec<Slot>,
    settings: EqSettings,
    /// Highest gain of preamp + curve (dB): above 0 the limiter is armed.
    eq_peak_db: f64,
    /// Linear gain (preamp x normalization), gliding to `gain_target`.
    gain: f64,
    gain_target: f64,
    limiter: f64,
    armed: bool,
    /// A crossfade sums two tracks: keep the limiter armed so the overlap never clips.
    pub fading: bool,
    glide: f64,
    release: f64,
    fx: Fx,
}

impl Dsp {
    pub fn new(rate: u32) -> Self {
        let mut d = Self {
            rate: 48_000.0,
            slots: (0..MAX_BANDS).map(|_| Slot { cur: FLAT, target: FLAT, coef: Biquad::design(&FLAT, 48_000.0), z: [[0.0; 2]; 2] }).collect(),
            settings: EqSettings::default(),
            eq_peak_db: 0.0,
            gain: 1.0,
            gain_target: 1.0,
            limiter: 1.0,
            armed: false,
            fading: false,
            glide: 0.0,
            release: 0.0,
            fx: Fx::new(48_000.0),
        };
        d.set_rate(rate);
        d
    }

    /// Device rate changed: coefficients follow, old filter state is meaningless.
    pub fn set_rate(&mut self, rate: u32) {
        self.rate = f64::from(rate.max(8000));
        self.glide = 1.0 - (-1.0 / (self.rate * GLIDE_SECS)).exp();
        self.release = 1.0 - (-1.0 / (self.rate * RELEASE_SECS)).exp();
        self.fx.set_rate(self.rate);
        for s in &mut self.slots {
            s.cur = s.target;
            s.coef = Biquad::design(&s.cur, self.rate);
            s.z = [[0.0; 2]; 2];
        }
    }

    pub fn set_eq(&mut self, mut eq: EqSettings) {
        eq.sanitize();
        let bands = eq.bands.iter().copied().chain(std::iter::repeat(FLAT));
        for (s, b) in self.slots.iter_mut().zip(bands) {
            s.target = match (eq.enabled, b.kind.has_gain()) {
                (true, _) => b,
                (false, true) => Band { gain: 0.0, ..b },
                (false, false) => FLAT,
            };
        }
        self.eq_peak_db = if eq.enabled { eq.max_boost() + eq.effective_preamp() } else { 0.0 };
        self.settings = eq;
    }

    pub fn set_fx(&mut self, fx: FxSettings) {
        self.fx.set(fx);
    }

    pub fn fx(&self) -> FxSettings {
        self.fx.settings
    }

    fn preamp_db(&self) -> f64 {
        if self.settings.enabled {
            self.settings.effective_preamp()
        } else {
            0.0
        }
    }

    /// Glides one band one block towards its target. Returns whether it moved.
    fn step(slot: &mut Slot, k: f64) -> bool {
        let (c, t) = (&mut slot.cur, slot.target);
        if *c == t {
            return false;
        }
        if c.is_identity() && t.is_identity() {
            *c = t;
            return true;
        }
        if c.kind != t.kind {
            if !(c.kind.has_gain() && t.kind.has_gain()) {
                // ponytail: low/high-pass in or out snaps (a click is possible); glide via a crossfade if it bothers.
                *c = t;
                return true;
            }
            // Peak <-> shelf: fade to 0 dB (where every shape is a wire), swap, fade up.
            c.gain -= c.gain * k;
            if c.gain.abs() < 0.01 {
                *c = Band { gain: 0.0, ..t };
            }
            return true;
        }
        let near = |a: f64, b: f64, eps: f64| (a - b).abs() < eps;
        c.gain += (t.gain - c.gain) * k;
        c.freq *= (t.freq / c.freq).powf(k);
        c.q *= (t.q / c.q).powf(k);
        if near(c.gain, t.gain, 0.005) && near(c.freq, t.freq, 0.05) && near(c.q, t.q, 0.0005) {
            *c = t;
        }
        true
    }

    /// Moves the gain to `norm_db` at once, no glide: a crossfade hands over from one deck's
    /// normalization to the next and pre-scales the outgoing deck to match.
    pub fn jump_gain(&mut self, norm_db: f64) {
        self.gain = db_to_lin(self.preamp_db() + norm_db);
        self.gain_target = self.gain;
    }

    /// Processes interleaved stereo `buf` in place. `norm_db` = normalization gain of the deck it came from.
    pub fn process(&mut self, buf: &mut [f32], norm_db: f64) {
        let total_db = self.preamp_db() + norm_db;
        self.gain_target = db_to_lin(total_db);
        self.armed = self.fading || self.eq_peak_db + norm_db > 1e-9 || self.fx.settings.boosts();
        let idle = self.slots.iter().all(|s| s.cur == s.target && s.cur.is_identity()) && self.fx.idle();
        if idle && self.gain == 1.0 && self.gain_target == 1.0 && !self.armed && self.limiter == 1.0 {
            return;
        }
        let kb = 1.0 - (1.0 - self.glide).powi(BLOCK as i32);
        for block in buf.chunks_mut(BLOCK * 2) {
            for s in &mut self.slots {
                if Self::step(s, kb) {
                    s.coef = Biquad::design(&s.cur, self.rate);
                    if s.cur.is_identity() {
                        s.z = [[0.0; 2]; 2];
                    }
                }
            }
            self.fx.block(kb);
            for frame in block.as_chunks_mut::<2>().0 {
                self.gain += (self.gain_target - self.gain) * self.glide;
                if (self.gain - self.gain_target).abs() < 1e-7 {
                    self.gain = self.gain_target;
                }
                let mut l = f64::from(frame[0]) * self.gain;
                let mut r = f64::from(frame[1]) * self.gain;
                for s in self.slots.iter_mut().filter(|s| !s.cur.is_identity()) {
                    l = s.coef.tick(&mut s.z[0], l);
                    r = s.coef.tick(&mut s.z[1], r);
                }
                (l, r) = self.fx.frame(l, r, self.glide);
                if self.armed || self.limiter < 1.0 {
                    // Instant attack, smooth release, stereo-linked: the output never passes the ceiling.
                    let peak = l.abs().max(r.abs());
                    let need = if peak > CEILING { CEILING / peak } else { 1.0 };
                    self.limiter = (self.limiter + (1.0 - self.limiter) * self.release).min(need);
                    if self.limiter > 0.999_999 {
                        self.limiter = 1.0;
                    }
                    l *= self.limiter;
                    r *= self.limiter;
                }
                frame[0] = l as f32;
                frame[1] = r as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: u32 = 48_000;

    fn graphic(gains: [f64; 10]) -> EqSettings {
        let freqs = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
        EqSettings {
            bands: freqs.iter().zip(gains).map(|(&freq, gain)| Band { kind: Kind::Peak, freq, gain, q: 1.41 }).collect(),
            ..Default::default()
        }
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 40) as f32 / (1u64 << 24) as f32 - 0.5
            })
            .collect()
    }

    fn sine(f: f64, amp: f64, frames: usize) -> Vec<f32> {
        (0..frames).flat_map(|i| [(amp * (2.0 * PI * f * i as f64 / f64::from(FS)).sin()) as f32; 2]).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn flat_eq_is_bit_transparent() {
        let input = noise(20_000);
        for eq in [EqSettings::default(), graphic([0.0; 10]), EqSettings { enabled: false, ..graphic([6.0; 10]) }] {
            let mut d = Dsp::new(FS);
            d.set_eq(eq);
            let mut buf = input.clone();
            d.process(&mut buf, 0.0);
            assert!(buf.iter().zip(&input).all(|(a, b)| a.to_bits() == b.to_bits()));
        }
    }

    #[test]
    fn peak_at_1k_measures_6_db() {
        let mut d = Dsp::new(FS);
        d.set_eq(EqSettings {
            auto_preamp: false,
            bands: vec![Band { kind: Kind::Peak, freq: 1000.0, gain: 6.0, q: 1.0 }],
            ..Default::default()
        });
        let input = sine(1000.0, 0.25, FS as usize);
        let mut buf = input.clone();
        d.process(&mut buf, 0.0);
        let tail = FS as usize; // last half second, long after the glide and the filter settle
        let db = 20.0 * (rms(&buf[tail..]) / rms(&input[tail..])).log10();
        assert!((db - 6.0).abs() <= 0.2, "measured {db:.3} dB");
    }

    #[test]
    fn auto_preamp_cancels_the_boost() {
        let eq = EqSettings { bands: vec![Band { kind: Kind::Peak, freq: 1000.0, gain: 6.0, q: 1.0 }], ..Default::default() };
        assert!((eq.effective_preamp() + 6.0).abs() < 0.05, "{}", eq.effective_preamp());
        assert_eq!(graphic([-3.0; 10]).effective_preamp(), 0.0, "cuts need no preamp");
        let mut d = Dsp::new(FS);
        d.set_eq(eq);
        let input = sine(1000.0, 0.9, FS as usize / 2);
        let mut buf = input.clone();
        d.process(&mut buf, 0.0);
        let tail = FS as usize / 2;
        let db = 20.0 * (rms(&buf[tail..]) / rms(&input[tail..])).log10();
        assert!(db.abs() < 0.1, "the peak frequency comes out at unity: {db:.3}");
    }

    #[test]
    fn extreme_params_never_produce_nan() {
        let wild = [f64::NAN, f64::INFINITY, -1e9, 0.0, 1e9, -0.0];
        let kinds = [Kind::Peak, Kind::LowShelf, Kind::HighShelf, Kind::LowPass, Kind::HighPass];
        let bands = (0..12)
            .map(|i| Band { kind: kinds[i % 5], freq: wild[i % 6], gain: wild[(i + 1) % 6], q: wild[(i + 2) % 6] })
            .collect();
        let mut eq = EqSettings { preamp: f64::NAN, auto_preamp: false, bands, ..Default::default() };
        for rate in [8000, 44_100, 192_000] {
            let mut d = Dsp::new(rate);
            d.set_eq(eq.clone());
            let mut buf: Vec<f32> = noise(40_000).iter().map(|v| v * 2.0).collect();
            d.process(&mut buf, 24.0);
            assert!(buf.iter().all(|v| v.is_finite() && v.abs() <= CEILING as f32 + 1e-6));
        }
        eq.sanitize();
        assert_eq!(eq.bands.len(), MAX_BANDS);
        assert!(eq.bands.iter().all(|b| (20.0..=20_000.0).contains(&b.freq) && (0.1..=20.0).contains(&b.q)));
    }

    #[test]
    fn limiter_keeps_boosted_audio_under_the_ceiling() {
        let mut d = Dsp::new(FS);
        let mut buf = sine(200.0, 0.8, FS as usize / 2);
        d.process(&mut buf, 12.0);
        assert!(buf.iter().all(|v| v.abs() <= CEILING as f32 + 1e-6));
        // Quiet audio boosted below the ceiling passes untouched by the limiter.
        let mut d = Dsp::new(FS);
        let input = sine(200.0, 0.1, FS as usize / 2);
        let mut buf = input.clone();
        d.process(&mut buf, 6.0);
        let tail = FS as usize / 2;
        let db = 20.0 * (rms(&buf[tail..]) / rms(&input[tail..])).log10();
        assert!((db - 6.0).abs() < 0.01, "{db}");
    }

    #[test]
    fn gain_changes_glide_without_steps() {
        // A 100 Hz tone: consecutive samples differ by at most ~2*pi*100/48000*amp at steady state.
        let mut d = Dsp::new(FS);
        d.set_eq(EqSettings { auto_preamp: false, ..graphic([0.0; 10]) });
        let mut buf = sine(100.0, 0.2, FS as usize / 4);
        d.process(&mut buf, 0.0);
        d.set_eq(EqSettings { auto_preamp: false, ..graphic([12.0, 12.0, 12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]) });
        let mut after = sine(100.0, 0.2, FS as usize / 4);
        d.process(&mut after, 0.0);
        let max_step = after.chunks(2).collect::<Vec<_>>().windows(2).map(|w| (w[1][0] - w[0][0]).abs()).fold(0.0, f32::max);
        let steady = 2.0 * PI as f32 * 100.0 / FS as f32 * 0.2 * 10f32.powf(14.0 / 20.0);
        assert!(max_step <= steady * 1.5, "step {max_step} vs steady bound {steady}");
        let half = after.len() / 2;
        assert!(rms(&after[half..]) > 3.0 * rms(&buf[half..]), "the boost arrived");
    }
}
