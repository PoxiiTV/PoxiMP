//! Seekbar waveform: the whole file decoded (no seeking, no resampling), downmixed, folded into
//! `BUCKETS` columns. Output = `BUCKETS` peak bytes then `BUCKETS` RMS bytes, both relative to the
//! track's own loudest peak (255), so quiet intros show up lower than the choruses.

use crate::{decode, Error};
use std::path::Path;
use symphonia::core::errors::Error as SymError;

/// Columns stored per track (the UI draws about one per 2 px, a 1920 px seekbar needs < 1000).
pub const BUCKETS: usize = 1024;
/// Frames per block: the decode is folded into blocks first, so the real length (not the
/// container's, often wrong for VBR MP3) decides where each column starts.
const BLOCK: usize = 256;

/// Max |sample| and sum of squares per `BLOCK` frames of a mono downmix.
#[derive(Default)]
pub struct Accumulator {
    blocks: Vec<(f32, f32)>,
    /// Frames already in the last (partial) block.
    fill: usize,
}

impl Accumulator {
    /// Planar samples of one decoded buffer (any channel count).
    pub fn push(&mut self, planes: &[&[f32]]) {
        let Some(len) = planes.iter().map(|p| p.len()).min() else { return };
        let k = 1.0 / planes.len() as f32;
        for i in 0..len {
            let m = planes.iter().map(|p| p[i]).sum::<f32>() * k;
            if self.fill == 0 {
                self.blocks.push((0.0, 0.0));
            }
            let b = self.blocks.last_mut().expect("block pushed above");
            b.0 = b.0.max(m.abs());
            b.1 += m * m;
            self.fill = (self.fill + 1) % BLOCK;
        }
    }

    /// `BUCKETS` peaks then `BUCKETS` RMS values; empty when nothing was decoded.
    pub fn finish(self) -> Vec<u8> {
        let n = self.blocks.len();
        if n == 0 {
            return Vec::new();
        }
        let frames = (n - 1) * BLOCK + if self.fill == 0 { BLOCK } else { self.fill };
        let (mut peak, mut rms) = (vec![0f32; BUCKETS], vec![0f32; BUCKETS]);
        for (c, (p, r)) in peak.iter_mut().zip(rms.iter_mut()).enumerate() {
            // Shorter than BUCKETS blocks: neighbouring columns share a block.
            let a = (c * n / BUCKETS).min(n - 1);
            let b = ((c + 1) * n / BUCKETS).max(a + 1);
            let span = &self.blocks[a..b];
            *p = span.iter().fold(0.0, |m, x| m.max(x.0));
            let count = if b == n { frames - a * BLOCK } else { (b - a) * BLOCK };
            *r = (span.iter().map(|x| x.1).sum::<f32>() / count.max(1) as f32).sqrt();
        }
        let top = peak.iter().fold(0f32, |m, &x| m.max(x));
        let scale = if top > 1e-6 { 255.0 / top } else { 0.0 };
        let q = |x: f32| (x * scale).round().clamp(0.0, 255.0) as u8;
        peak.iter().chain(&rms).map(|&x| q(x)).collect()
    }
}

/// Decodes the whole file. `Ok(empty)` = no audio in it.
pub fn waveform(path: &Path) -> Result<Vec<u8>, Error> {
    let mut src = decode::open(path)?;
    let mut acc = Accumulator::default();
    let mut planes: Vec<Vec<f32>> = Vec::new();
    while let Ok(Some(packet)) = src.format.next_packet() {
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
        let refs: Vec<&[f32]> = planes.iter().map(|p| &p[..len]).collect();
        acc.push(&refs);
    }
    Ok(acc.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loud_middle_quiet_ends() {
        // 30 s at 8 kHz: 0.1 amplitude for the first and last 10 s, 0.8 in between.
        let fs = 8000;
        let x: Vec<f32> = (0..30 * fs)
            .map(|i| {
                let amp = if (10 * fs..20 * fs).contains(&i) { 0.8 } else { 0.1 };
                amp * (i as f32 * 0.37).sin()
            })
            .collect();
        let mut acc = Accumulator::default();
        // Uneven buffer sizes, stereo with the same signal on both sides.
        for chunk in x.chunks(1151) {
            acc.push(&[chunk, chunk]);
        }
        let w = acc.finish();
        assert_eq!(w.len(), 2 * BUCKETS);
        let (peak, rms) = w.split_at(BUCKETS);
        let at = |v: &[u8], f: f32| v[(f * BUCKETS as f32) as usize] as f32;
        assert!(at(peak, 0.5) >= 250.0, "middle is the loudest: {}", at(peak, 0.5));
        for f in [0.05, 0.2, 0.8, 0.95] {
            assert!((at(peak, f) - 255.0 / 8.0).abs() < 3.0, "quiet end at {f}: {}", at(peak, f));
        }
        // Sine RMS = peak / sqrt 2.
        assert!((at(rms, 0.5) - 255.0 / 2f32.sqrt()).abs() < 4.0, "rms {}", at(rms, 0.5));
        assert!(rms.iter().zip(peak).all(|(r, p)| r <= p));
    }

    #[test]
    fn short_and_silent() {
        let mut acc = Accumulator::default();
        acc.push(&[&[0.0; 100]]);
        assert_eq!(acc.finish(), vec![0; 2 * BUCKETS]);
        assert!(Accumulator::default().finish().is_empty());
        let mut acc = Accumulator::default();
        acc.push(&[&[0.5; 10]]);
        assert!(acc.finish()[..BUCKETS].iter().all(|&p| p == 255), "one block spread over every column");
        assert!(waveform(Path::new("/definitely/not/here.flac")).is_err());
    }
}
