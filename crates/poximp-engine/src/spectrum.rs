//! Pull visualizer: Hann + realfft over the newest samples -> log-spaced bands as bytes.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::output::VIZ_LEN;

const FLOOR_DB: f32 = -80.0;

pub(crate) struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// 2 / sum(window): a full-scale sine reads 0 dB.
    norm: f32,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
}

impl Analyzer {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(VIZ_LEN);
        let window: Vec<f32> = (0..VIZ_LEN)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / VIZ_LEN as f32).cos())
            .collect();
        let norm = 2.0 / window.iter().sum::<f32>();
        Self { input: fft.make_input_vec(), output: fft.make_output_vec(), fft, window, norm }
    }

    /// `samples`: the newest `VIZ_LEN` mono samples, oldest first.
    pub fn bands(&mut self, samples: impl Iterator<Item = f32>, rate: u32, bands: usize) -> Vec<u8> {
        for ((x, s), w) in self.input.iter_mut().zip(samples).zip(&self.window) {
            *x = s * w;
        }
        self.fft.process(&mut self.input, &mut self.output).expect("buffers from the plan");
        band_bins(bands, rate)
            .map(|(lo, hi)| {
                let peak = self.output[lo..hi].iter().map(|c| c.norm()).fold(0.0, f32::max) * self.norm;
                let db = 20.0 * peak.max(1e-9).log10();
                ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0).mul_add(255.0, 0.5) as u8
            })
            .collect()
    }
}

/// FFT bin range `[lo, hi)` of each log-spaced band from 20 Hz to 20 kHz (capped at Nyquist).
/// Low bands narrower than one bin repeat their bin.
pub(crate) fn band_bins(bands: usize, rate: u32) -> impl Iterator<Item = (usize, usize)> {
    let nyquist_bin = VIZ_LEN / 2;
    let top = 20_000f32.min(rate as f32 / 2.0);
    let bin = move |f: f32| (f * VIZ_LEN as f32 / rate as f32) as usize;
    (0..bands).map(move |k| {
        let edge = |k: usize| 20.0 * (top / 20.0).powf(k as f32 / bands as f32);
        let lo = bin(edge(k)).min(nyquist_bin - 1);
        let hi = bin(edge(k + 1)).clamp(lo + 1, nyquist_bin);
        (lo, hi)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_edges_cover_the_range_in_order() {
        let bins: Vec<_> = band_bins(96, 48_000).collect();
        assert_eq!(bins.len(), 96);
        assert_eq!(bins[0].0, (20.0 * VIZ_LEN as f32 / 48_000.0) as usize);
        assert!(bins.windows(2).all(|w| w[0].0 <= w[1].0 && w[0].1 <= w[1].1));
        assert!(bins.iter().all(|&(lo, hi)| lo < hi && hi <= VIZ_LEN / 2));
        let last = bins.last().unwrap().1;
        assert_eq!(last, (20_000.0 * VIZ_LEN as f32 / 48_000.0) as usize);
        // 22.05 kHz device: capped at Nyquist.
        assert!(band_bins(64, 22_050).all(|(_, hi)| hi <= VIZ_LEN / 2));
    }

    #[test]
    fn full_scale_sine_lights_its_band_only() {
        let rate = 48_000;
        let f = 1_000.0;
        let sine = (0..VIZ_LEN).map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / rate as f32).sin());
        let out = Analyzer::new().bands(sine, rate, 64);
        let (peak_band, &peak) = out.iter().enumerate().max_by_key(|b| b.1).unwrap();
        let (lo, hi) = band_bins(64, rate).nth(peak_band).unwrap();
        let bin_hz = rate as f32 / VIZ_LEN as f32;
        assert!(lo as f32 * bin_hz <= f + bin_hz && f <= hi as f32 * bin_hz + bin_hz);
        assert!(peak >= 250, "0 dBFS reads near 255, got {peak}");
        let ten_k = band_bins(64, rate).position(|(lo, _)| lo as f32 * bin_hz >= 10_000.0).unwrap();
        assert!(out[ten_k] < 40, "far band stays low, got {}", out[ten_k]);
        assert!(Analyzer::new().bands(std::iter::repeat(0.0), rate, 64).iter().all(|&b| b == 0));
    }
}
