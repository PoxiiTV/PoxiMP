//! Listening effects, after the EQ and before the limiter: bass boost (one low shelf), stereo
//! width / mono, headphone crossfeed (Bauer, bs2b) and balance. Speed lives in the producer (it
//! is the decks' resampling ratio). Every change glides; neutral settings leave the samples alone.

use serde::{Deserialize, Serialize};

use crate::eq::{Band, Biquad, Kind};

/// Shelf corner of the bass boost (Hz).
const BASS_HZ: f64 = 105.0;
pub const MAX_BASS_DB: f64 = 12.0;
pub const SPEED_RANGE: (f64, f64) = (0.5, 1.5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Crossfeed {
    #[default]
    Off,
    /// bs2b "Jan Meier" (650 Hz, 9.5 dB): barely there
    Soft,
    /// bs2b "Chu Moy" (700 Hz, 6 dB)
    Medium,
    /// bs2b default (700 Hz, 4.5 dB): the most
    Strong,
}

impl Crossfeed {
    /// (cut-off Hz, feed dB)
    fn params(self) -> Option<(f64, f64)> {
        match self {
            Crossfeed::Off => None,
            Crossfeed::Soft => Some((650.0, 9.5)),
            Crossfeed::Medium => Some((700.0, 6.0)),
            Crossfeed::Strong => Some((700.0, 4.5)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FxSettings {
    pub crossfeed: Crossfeed,
    /// Side level: 0 = mono, 1 = as recorded, 2 = twice as wide.
    pub width: f64,
    /// Both ears get the same (overrides `width`).
    pub mono: bool,
    /// -1 = left only .. 0 = centre .. 1 = right only.
    pub balance: f64,
    /// Low shelf, 0..=MAX_BASS_DB dB.
    pub bass: f64,
    /// Playback rate, pitch follows (like a turntable). 1 = normal.
    pub speed: f64,
}

impl Default for FxSettings {
    fn default() -> Self {
        Self { crossfeed: Crossfeed::Off, width: 1.0, mono: false, balance: 0.0, bass: 0.0, speed: 1.0 }
    }
}

fn clamp(v: f64, lo: f64, hi: f64, def: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        def
    }
}

impl FxSettings {
    pub fn sanitize(&mut self) {
        self.width = clamp(self.width, 0.0, 2.0, 1.0);
        self.balance = clamp(self.balance, -1.0, 1.0, 0.0);
        self.bass = clamp(self.bass, 0.0, MAX_BASS_DB, 0.0);
        // Steps of 1 %, so two near-identical values never reopen the decks.
        self.speed = (clamp(self.speed, SPEED_RANGE.0, SPEED_RANGE.1, 1.0) * 100.0).round() / 100.0;
    }

    /// Can make peaks louder (the limiter has to be armed).
    pub(crate) fn boosts(&self) -> bool {
        self.bass > 0.0 || (!self.mono && self.width > 1.0) || self.crossfeed != Crossfeed::Off
    }
}

/// bs2b crossfeed filter (one-pole low-pass to the other ear, high-shelf on its own).
#[derive(Debug, Clone, Copy, Default)]
struct Bs2b {
    a0_lo: f64,
    b1_lo: f64,
    a0_hi: f64,
    a1_hi: f64,
    b1_hi: f64,
    gain: f64,
    lo: [f64; 2],
    hi: [f64; 2],
    asis: [f64; 2],
}

impl Bs2b {
    fn new(fcut: f64, feed_db: f64, fs: f64) -> Self {
        let gb_lo = feed_db * -5.0 / 6.0 - 3.0;
        let gb_hi = feed_db / 6.0 - 3.0;
        let g_lo = 10f64.powf(gb_lo / 20.0);
        let g_hi = 1.0 - 10f64.powf(gb_hi / 20.0);
        let fc_hi = fcut * 2f64.powf((gb_lo - 20.0 * g_hi.log10()) / 12.0);
        let x = (-2.0 * std::f64::consts::PI * fcut.min(0.45 * fs) / fs).exp();
        let (b1_lo, a0_lo) = (x, g_lo * (1.0 - x));
        let x = (-2.0 * std::f64::consts::PI * fc_hi.min(0.45 * fs) / fs).exp();
        Self { a0_lo, b1_lo, a0_hi: 1.0 - g_hi * (1.0 - x), a1_hi: -x, b1_hi: x, gain: 1.0 / (1.0 - g_hi + g_lo), ..Default::default() }
    }

    #[inline]
    fn tick(&mut self, l: f64, r: f64) -> (f64, f64) {
        for (c, x) in [(0, l), (1, r)] {
            self.lo[c] = self.a0_lo * x + self.b1_lo * self.lo[c];
            self.hi[c] = self.a0_hi * x + self.a1_hi * self.asis[c] + self.b1_hi * self.hi[c];
            self.asis[c] = x;
        }
        ((self.hi[0] + self.lo[1]) * self.gain, (self.hi[1] + self.lo[0]) * self.gain)
    }
}

/// Gliding values, one step per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Live {
    width: f64,
    left: f64,
    right: f64,
    /// Crossfeed wet level 0..1 (fades in and out, and over a change of strength)
    wet: f64,
    bass: f64,
}

pub(crate) struct Fx {
    rate: f64,
    pub(crate) settings: FxSettings,
    target: Live,
    cur: Live,
    bs2b: Bs2b,
    /// Strength the filter was built for (it changes at zero wet level)
    built: Crossfeed,
    shelf: Biquad,
    shelf_z: [[f64; 2]; 2],
    shelf_db: f64,
}

impl Fx {
    pub(crate) fn new(rate: f64) -> Self {
        let neutral = Live { width: 1.0, left: 1.0, right: 1.0, wet: 0.0, bass: 0.0 };
        let mut f = Self {
            rate,
            settings: FxSettings::default(),
            target: neutral,
            cur: neutral,
            bs2b: Bs2b::default(),
            built: Crossfeed::Off,
            shelf: Biquad::design(&shelf(0.0), rate),
            shelf_z: [[0.0; 2]; 2],
            shelf_db: 0.0,
        };
        f.set_rate(rate);
        f
    }

    pub(crate) fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        self.built = Crossfeed::Off;
        self.rebuild_crossfeed();
        self.shelf = Biquad::design(&shelf(self.shelf_db), rate);
        self.shelf_z = [[0.0; 2]; 2];
    }

    fn rebuild_crossfeed(&mut self) {
        if let Some((fc, feed)) = self.settings.crossfeed.params() {
            self.bs2b = Bs2b::new(fc, feed, self.rate);
            self.built = self.settings.crossfeed;
        }
    }

    pub(crate) fn set(&mut self, mut s: FxSettings) {
        s.sanitize();
        let width = if s.mono { 0.0 } else { s.width };
        // Pan law that keeps the centre at unity: only the other side goes down.
        let (left, right) = (1.0 - s.balance.max(0.0), 1.0 + s.balance.min(0.0));
        let wet = if s.crossfeed == Crossfeed::Off { 0.0 } else { 1.0 };
        self.target = Live { width, left, right, wet, bass: s.bass };
        self.settings = s;
        if self.cur.wet == 0.0 && s.crossfeed != self.built {
            self.rebuild_crossfeed();
        }
    }

    pub(crate) fn idle(&self) -> bool {
        self.cur == self.target && self.cur == Live { width: 1.0, left: 1.0, right: 1.0, wet: 0.0, bass: 0.0 }
    }

    /// Once per block: the bass shelf follows its gliding gain.
    pub(crate) fn block(&mut self, k: f64) {
        let t = self.target.bass;
        if self.cur.bass != t {
            self.cur.bass += (t - self.cur.bass) * k;
            if (self.cur.bass - t).abs() < 0.005 {
                self.cur.bass = t;
            }
        }
        if self.cur.bass != self.shelf_db {
            self.shelf_db = self.cur.bass;
            self.shelf = Biquad::design(&shelf(self.shelf_db), self.rate);
            if self.shelf_db == 0.0 {
                self.shelf_z = [[0.0; 2]; 2];
            }
        }
    }

    #[inline]
    pub(crate) fn frame(&mut self, mut l: f64, mut r: f64, glide: f64) -> (f64, f64) {
        let (c, t) = (&mut self.cur, self.target);
        let step = |v: &mut f64, to: f64| {
            *v += (to - *v) * glide;
            if (*v - to).abs() < 1e-6 {
                *v = to;
            }
        };
        step(&mut c.width, t.width);
        step(&mut c.left, t.left);
        step(&mut c.right, t.right);
        // A new strength: fade the old one out, then in with the new filter.
        let wet_to = if self.built == self.settings.crossfeed { t.wet } else { 0.0 };
        step(&mut c.wet, wet_to);
        if c.wet == 0.0 && self.built != self.settings.crossfeed && self.settings.crossfeed != Crossfeed::Off {
            self.rebuild_crossfeed();
        }
        if self.shelf_db != 0.0 {
            l = self.shelf.tick(&mut self.shelf_z[0], l);
            r = self.shelf.tick(&mut self.shelf_z[1], r);
        }
        let c = self.cur;
        if c.width != 1.0 {
            let (m, s) = ((l + r) * 0.5, (l - r) * 0.5 * c.width);
            (l, r) = (m + s, m - s);
        }
        if c.wet > 0.0 {
            let (wl, wr) = self.bs2b.tick(l, r);
            (l, r) = (l + (wl - l) * c.wet, r + (wr - r) * c.wet);
        }
        (l * c.left, r * c.right)
    }
}

fn shelf(gain: f64) -> Band {
    Band { kind: Kind::LowShelf, freq: BASS_HZ, gain, q: 0.707 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48_000.0;

    fn run(fx: &mut Fx, frames: &[(f64, f64)]) -> Vec<(f64, f64)> {
        let glide = 1.0 - (-1.0 / (FS * 0.0045f64)).exp();
        frames
            .chunks(32)
            .flat_map(|b| {
                fx.block(1.0 - (1.0 - glide).powi(32));
                b.iter().map(|&(l, r)| fx.frame(l, r, glide)).collect::<Vec<_>>()
            })
            .collect()
    }

    fn tone(f: f64, n: usize, l: f64, r: f64) -> Vec<(f64, f64)> {
        (0..n).map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / FS).sin()).map(|s| (s * l, s * r)).collect()
    }

    fn rms(x: &[(f64, f64)], ch: usize) -> f64 {
        (x.iter().map(|p| if ch == 0 { p.0 } else { p.1 }.powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn neutral_is_idle_and_effects_do_what_they_say() {
        let mut fx = Fx::new(FS);
        fx.set(FxSettings::default());
        assert!(fx.idle());
        let half = 24_000;

        // Mono: a left-only tone comes out on both sides, at half level each.
        fx.set(FxSettings { mono: true, ..Default::default() });
        let out = run(&mut fx, &tone(440.0, 48_000, 0.5, 0.0));
        let (l, r) = (rms(&out[half..], 0), rms(&out[half..], 1));
        assert!((l - r).abs() < 1e-6 && (l - 0.25 / 2f64.sqrt()).abs() < 1e-3, "{l} {r}");

        // Balance fully left: the right side is silent, the left untouched.
        let mut fx = Fx::new(FS);
        fx.set(FxSettings { balance: -1.0, ..Default::default() });
        let out = run(&mut fx, &tone(440.0, 48_000, 0.5, 0.5));
        assert!(rms(&out[half..], 1) < 1e-6 && (rms(&out[half..], 0) - 0.5 / 2f64.sqrt()).abs() < 1e-3);

        // Bass +6 dB: a 40 Hz tone gains ~6 dB, a 5 kHz one stays.
        let mut fx = Fx::new(FS);
        fx.set(FxSettings { bass: 6.0, ..Default::default() });
        let low = run(&mut fx, &tone(40.0, 48_000, 0.1, 0.1));
        let db = 20.0 * (rms(&low[half..], 0) / (0.1 / 2f64.sqrt())).log10();
        assert!((db - 6.0).abs() < 0.5, "bass {db}");
        let high = run(&mut fx, &tone(5000.0, 48_000, 0.1, 0.1));
        let db = 20.0 * (rms(&high[half..], 0) / (0.1 / 2f64.sqrt())).log10();
        assert!(db.abs() < 0.3, "treble {db}");

        // Crossfeed: a left-only low tone reaches the right ear, but quieter than the left.
        let mut fx = Fx::new(FS);
        fx.set(FxSettings { crossfeed: Crossfeed::Medium, ..Default::default() });
        let out = run(&mut fx, &tone(200.0, 48_000, 0.5, 0.0));
        let (l, r) = (rms(&out[half..], 0), rms(&out[half..], 1));
        assert!(r > 0.05 && r < l, "{l} {r}");
        // A centred (mono) signal keeps about its level (bs2b dips ~1 dB in the mids by design).
        let out = run(&mut fx, &tone(1000.0, 48_000, 0.3, 0.3));
        let db = 20.0 * (rms(&out[half..], 0) / (0.3 / 2f64.sqrt())).log10();
        assert!(db.abs() < 1.5, "centre {db}");

        // Back to neutral: it glides out and goes idle again.
        fx.set(FxSettings::default());
        run(&mut fx, &tone(1000.0, 48_000, 0.3, 0.3));
        assert!(fx.idle());
    }

    #[test]
    fn wild_settings_are_tamed() {
        let mut s = FxSettings { width: f64::NAN, balance: 9.0, bass: -3.0, speed: 7.0, ..Default::default() };
        s.sanitize();
        assert_eq!((s.width, s.balance, s.bass, s.speed), (1.0, 1.0, 0.0, 1.5));
        let mut fx = Fx::new(8000.0);
        fx.set(FxSettings { crossfeed: Crossfeed::Strong, bass: 12.0, width: 2.0, ..Default::default() });
        assert!(run(&mut fx, &tone(3000.0, 8000, 1.0, -1.0)).iter().all(|p| p.0.is_finite() && p.1.is_finite()));
    }
}
