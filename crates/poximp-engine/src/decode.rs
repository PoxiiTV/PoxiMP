//! One open file ("deck"): symphonia decode (gapless) -> stereo f32 -> rubato to the output rate.

use std::f32::consts::FRAC_1_SQRT_2;
use std::fs::File;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::audio::{Channels, Position};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::TimeBase;

use crate::output::Format;

const OPUS_PREROLL_SECS: f64 = 0.08;

pub(crate) struct Deck {
    pub path: PathBuf,
    /// Seconds; 0 when the container does not say.
    pub duration: f64,
    /// Volume normalization (dB), applied by the producer's DSP.
    pub gain_db: f64,
    /// Seconds of crossfade into this deck when it follows another one (0 = gapless).
    pub crossfade: f64,
    /// Playback speed it was opened for (its output rate is the device's / speed).
    pub speed: f64,
    /// Source sample rate and bit depth (None for lossy codecs).
    pub rate: u32,
    pub bits: Option<u32>,
    /// Frames handed out so far, at the output rate.
    pub out_frames: u64,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    /// Seconds per timestamp tick.
    tb: f64,
    out_rate: u32,
    matrix: Vec<[f32; 2]>,
    decoded: Vec<f32>,
    stereo: Vec<f32>,
    resampler: Option<Resample>,
    /// After a seek: drop decoded audio before this time (sample-accurate seek).
    seek_to: Option<f64>,
    /// Ticks the packet timestamps run ahead of the decoded audio (see `open_inner`).
    pts_offset: i64,
    eof: bool,
}

impl Deck {
    /// Errors are short codes the UI maps to i18n text (never paths or internals).
    pub fn open(path: &Path, out_rate: u32) -> Result<Self, &'static str> {
        // Malformed files can make a decoder panic; never let that take the app down.
        catch_unwind(AssertUnwindSafe(|| Self::open_inner(path, out_rate))).unwrap_or(Err("decode_failed"))
    }

    fn open_inner(path: &Path, out_rate: u32) -> Result<Self, &'static str> {
        let file = File::open(path).map_err(|_| "open_failed")?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let fmt_opts = FormatOptions::default().seek_index_fill_period_ms(500);
        let format = poximp_codecs::probe()
            .probe(&hint, mss, fmt_opts, MetadataOptions::default())
            .map_err(|_| "unsupported_format")?;
        let track = format.default_track(TrackType::Audio).ok_or("unsupported_format")?;
        let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("unsupported_format")?;
        let rate = params.sample_rate.filter(|&r| r > 0).ok_or("unsupported_format")?;
        let decoder = poximp_codecs::codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|_| "unsupported_format")?;
        let duration = match (track.time_base, track.duration) {
            (Some(tb), Some(d)) => tb.calc_duration(d).map(|t| t.as_secs_f64()),
            _ => None,
        }
        .or(track.num_frames.map(|n| n as f64 / f64::from(rate)))
        .unwrap_or(0.0);
        let tb = track.time_base.map(f64::from).unwrap_or(1.0 / f64::from(rate));
        let (track_id, time_base, bits) = (track.id, track.time_base, params.bits_per_sample);
        // The libopus adapter drops the Ogg pre-skip itself, while the Ogg timestamps still count
        // it: without this, every Opus seek would land pre-skip (6.5 ms) early.
        let pts_offset = if params.codec == CODEC_ID_OPUS { i64::from(track.delay.unwrap_or(0)) } else { 0 };
        // ponytail: rate fixed at open; a mid-stream rate change (chained Ogg) plays at the wrong speed.
        let resampler = (rate != out_rate).then(|| Resample::new(rate, out_rate));
        Ok(Self {
            path: path.to_path_buf(),
            duration,
            gain_db: 0.0,
            crossfade: 0.0,
            speed: 1.0,
            rate,
            bits,
            out_frames: 0,
            format,
            decoder,
            track_id,
            time_base,
            tb,
            out_rate,
            matrix: Vec::new(),
            decoded: Vec::new(),
            stereo: Vec::new(),
            resampler,
            seek_to: None,
            pts_offset,
            eof: false,
        })
    }

    /// Appends the next stereo frames at the output rate to `out`. `Ok(false)` = end of file.
    pub fn read(&mut self, out: &mut Vec<f32>) -> Result<bool, &'static str> {
        let before = out.len();
        let r = catch_unwind(AssertUnwindSafe(|| self.read_inner(out))).unwrap_or(Err("decode_failed"));
        self.out_frames += (out.len().saturating_sub(before) / 2) as u64;
        r
    }

    /// What an exclusive output should open at for this file.
    pub fn format(&self) -> Format {
        Format { rate: self.rate, bits: self.bits }
    }

    /// The output reopened at another rate. Only before the first `read` (or right after a seek).
    pub fn set_out_rate(&mut self, out_rate: u32) {
        if out_rate != self.out_rate {
            self.out_rate = out_rate;
            self.resampler = (self.rate != out_rate).then(|| Resample::new(self.rate, out_rate));
        }
    }

    /// Length at the output rate; 0 when the container does not say.
    pub fn total_frames(&self) -> u64 {
        (self.duration * f64::from(self.out_rate)).round() as u64
    }

    fn read_inner(&mut self, out: &mut Vec<f32>) -> Result<bool, &'static str> {
        let start_len = out.len();
        loop {
            if self.eof {
                return Ok(out.len() > start_len);
            }
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                // Truncated files end with an IO error: treat it as the end.
                Ok(None) | Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => {
                    self.eof = true;
                    if let Some(r) = &mut self.resampler {
                        r.finish(out);
                    }
                    continue;
                }
                Err(_) => return Err("decode_failed"),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buf = match self.decoder.decode(&packet) {
                Ok(b) => b,
                // A corrupt packet: skip it, like every player does.
                Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => continue,
                Err(_) => return Err("decode_failed"),
            };
            let channels = buf.spec().channels();
            if channels.count() == 0 {
                continue;
            }
            if channels.count() != self.matrix.len() {
                self.matrix = stereo_matrix(channels);
            }
            buf.copy_to_vec_interleaved::<f32>(&mut self.decoded);
            let frames = buf.frames();
            let mut skip = 0;
            if let Some(target) = self.seek_to {
                let start = (packet.pts.get() + packet.trim_start.get() as i64 - self.pts_offset) as f64 * self.tb;
                skip = ((target - start) * f64::from(self.rate)).round().max(0.0) as usize;
                if skip >= frames {
                    continue;
                }
                self.seek_to = None;
            }
            let ch = self.matrix.len();
            self.stereo.clear();
            to_stereo(&self.decoded[skip * ch..frames * ch], &self.matrix, &mut self.stereo);
            match &mut self.resampler {
                Some(r) => r.push(&self.stereo, out),
                None => out.extend_from_slice(&self.stereo),
            }
            if out.len() > start_len {
                return Ok(true);
            }
        }
    }

    /// Sample-accurate seek. Returns the position it landed on (seconds).
    pub fn seek(&mut self, secs: f64) -> f64 {
        let fallback = secs;
        catch_unwind(AssertUnwindSafe(|| self.seek_inner(secs))).unwrap_or_else(|_| {
            self.eof = true;
            fallback
        })
    }

    fn seek_inner(&mut self, secs: f64) -> f64 {
        let mut secs = secs.max(0.0);
        if self.duration > 0.0 {
            secs = secs.min(self.duration);
        }
        self.out_frames = (secs * f64::from(self.out_rate)).round() as u64;
        // Opus needs ~80 ms of pre-roll to converge after a reset (RFC 7845 §4.6): start that
        // much earlier; the audio before `secs` is dropped below anyway.
        let preroll = if self.pts_offset > 0 { OPUS_PREROLL_SECS } else { 0.0 };
        let to = poximp_codecs::seek_to(self.track_id, self.time_base, self.rate, (secs - preroll).max(0.0));
        self.decoder.reset();
        if let Some(r) = &mut self.resampler {
            r.reset();
        }
        match to.map(|to| self.format.seek(SeekMode::Accurate, to)) {
            Some(Ok(_)) => {
                self.seek_to = Some(secs);
                self.eof = false;
            }
            // Past the end (or unseekable): finish the track instead of looping.
            _ => self.eof = true,
        }
        secs
    }
}

/// Source channel -> [left, right] gains. Mono goes to both sides at full level; surround is
/// folded with -3 dB centre/surrounds, LFE dropped, and scaled so the sum never clips.
pub(crate) fn stereo_matrix(ch: &Channels) -> Vec<[f32; 2]> {
    let n = ch.count();
    if n == 1 {
        return vec![[1.0, 1.0]];
    }
    let positions = match ch {
        Channels::Positioned(p) => Some(*p),
        _ => Position::from_wave_channel_count(n as u32),
    };
    let mut m: Vec<[f32; 2]> = match positions {
        Some(p) => p.iter().map(gains).collect(),
        // Unknown layout: alternate left/right.
        None => (0..n).map(|i| if i % 2 == 0 { [1.0, 0.0] } else { [0.0, 1.0] }).collect(),
    };
    m.resize(n, [0.0, 0.0]);
    let peak = m.iter().fold([0.0f32; 2], |s, g| [s[0] + g[0], s[1] + g[1]]);
    let norm = peak[0].max(peak[1]);
    if norm > 1.0 {
        m.iter_mut().for_each(|g| *g = [g[0] / norm, g[1] / norm]);
    }
    m
}

fn gains(p: Position) -> [f32; 2] {
    const H: f32 = FRAC_1_SQRT_2;
    let left = Position::REAR_LEFT
        | Position::FRONT_LEFT_CENTER
        | Position::SIDE_LEFT
        | Position::TOP_FRONT_LEFT
        | Position::TOP_REAR_LEFT
        | Position::TOP_SIDE_LEFT
        | Position::BOTTOM_FRONT_LEFT
        | Position::FRONT_LEFT_WIDE;
    let right = Position::REAR_RIGHT
        | Position::FRONT_RIGHT_CENTER
        | Position::SIDE_RIGHT
        | Position::TOP_FRONT_RIGHT
        | Position::TOP_REAR_RIGHT
        | Position::TOP_SIDE_RIGHT
        | Position::BOTTOM_FRONT_RIGHT
        | Position::FRONT_RIGHT_WIDE;
    if p == Position::FRONT_LEFT {
        [1.0, 0.0]
    } else if p == Position::FRONT_RIGHT {
        [0.0, 1.0]
    } else if p.intersects(Position::LFE1 | Position::LFE2) {
        [0.0, 0.0]
    } else if p.intersects(left) {
        [H, 0.0]
    } else if p.intersects(right) {
        [0.0, H]
    } else {
        [H, H] // centres
    }
}

pub(crate) fn to_stereo(src: &[f32], m: &[[f32; 2]], out: &mut Vec<f32>) {
    for frame in src.chunks_exact(m.len()) {
        let (mut l, mut r) = (0.0, 0.0);
        for (s, g) in frame.iter().zip(m) {
            l += s * g[0];
            r += s * g[1];
        }
        out.extend_from_slice(&[l, r]);
    }
}

/// Fixed-ratio stereo resampler for a whole track: compensates the filter delay and emits
/// exactly round(n_in * to / from) frames, so gapless transitions stay sample-exact.
pub(crate) struct Resample {
    fft: Fft<f32>,
    from: u64,
    to: u64,
    input: Vec<f32>,
    scratch: Vec<f32>,
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

impl Resample {
    pub fn new(from: u32, to: u32) -> Self {
        let fft = Fft::<f32>::new(from as usize, to as usize, 1024, 2, FixedSync::Input)
            .expect("valid non-zero rates");
        let scratch = vec![0.0; fft.output_frames_max() * 2];
        Self {
            delay_left: fft.output_delay(),
            fft,
            from: from.into(),
            to: to.into(),
            input: Vec::new(),
            scratch,
            in_total: 0,
            out_total: 0,
        }
    }

    pub fn reset(&mut self) {
        self.fft.reset();
        self.input.clear();
        self.delay_left = self.fft.output_delay();
        self.in_total = 0;
        self.out_total = 0;
    }

    pub fn push(&mut self, stereo: &[f32], out: &mut Vec<f32>) {
        self.input.extend_from_slice(stereo);
        self.in_total += (stereo.len() / 2) as u64;
        let need = self.fft.input_frames_next();
        let mut used = 0;
        while self.input.len() / 2 - used >= need {
            self.run(used, out);
            used += need;
        }
        self.input.drain(..used * 2);
    }

    /// End of track: flush the tail (zero padded) and cut to the exact length.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        let expected = (self.in_total * self.to + self.from / 2) / self.from;
        while self.out_total < expected {
            let need = self.fft.input_frames_next();
            self.input.resize(need * 2, 0.0);
            self.run(0, out);
            self.input.clear();
        }
        let extra = (self.out_total - expected) as usize;
        out.truncate(out.len() - extra * 2);
        self.out_total = expected;
    }

    fn run(&mut self, offset: usize, out: &mut Vec<f32>) {
        let total = self.input.len() / 2;
        let input = InterleavedSlice::new(&self.input[..], 2, total).expect("sized input");
        let out_frames = self.scratch.len() / 2;
        let mut output = InterleavedSlice::new_mut(&mut self.scratch[..], 2, out_frames).expect("sized output");
        let idx = rubato::Indexing::new().input_offset(offset);
        let (_, produced) =
            self.fft.process_into_buffer(&input, &mut output, Some(&idx)).expect("buffers sized from the resampler");
        let skip = self.delay_left.min(produced);
        self.delay_left -= skip;
        out.extend_from_slice(&self.scratch[skip * 2..produced * 2]);
        self.out_total += (produced - skip) as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampled_length_is_exact_and_delay_compensated() {
        for (from, to, n) in [(44_100u32, 48_000u32, 100_000usize), (48_000, 44_100, 77_777), (96_000, 48_000, 12_345)] {
            let mut r = Resample::new(from, to);
            // Impulse at a known frame, fed in uneven chunks.
            let at = n / 3;
            let mut input = vec![0.0f32; n * 2];
            input[at * 2] = 1.0;
            input[at * 2 + 1] = 1.0;
            let mut out = Vec::new();
            for chunk in input.chunks(2 * 333) {
                r.push(chunk, &mut out);
            }
            r.finish(&mut out);
            let expected = (n as u64 * to as u64 + from as u64 / 2) / from as u64;
            assert_eq!(out.len() / 2, expected as usize, "{from}->{to}");
            let peak = out.iter().step_by(2).enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
            let want = at as f64 * to as f64 / from as f64;
            assert!((peak as f64 - want).abs() <= 2.0, "{from}->{to}: impulse at {peak}, want {want}");
        }
    }

    #[test]
    fn channel_mapping() {
        let mono = stereo_matrix(&Channels::Discrete(1));
        let mut out = Vec::new();
        to_stereo(&[0.5, -0.25], &mono, &mut out);
        assert_eq!(out, [0.5, 0.5, -0.25, -0.25]);

        let st = stereo_matrix(&Channels::Discrete(2));
        assert_eq!(st, [[1.0, 0.0], [0.0, 1.0]]);

        // 5.1 WAVE order: FL FR FC LFE RL RR.
        let m = stereo_matrix(&Channels::Discrete(6));
        assert_eq!(m.len(), 6);
        assert_eq!(m[3], [0.0, 0.0], "LFE dropped");
        assert!(m[0][0] > m[2][0] && m[2][0] == m[2][1], "centre split evenly, quieter than front");
        assert_eq!(m[4][1], 0.0);
        assert_eq!(m[5][0], 0.0);
        let sum_l: f32 = m.iter().map(|g| g[0]).sum();
        assert!((sum_l - 1.0).abs() < 1e-6, "full-scale on every channel cannot clip");
        out.clear();
        to_stereo(&[1.0; 6], &m, &mut out);
        assert!(out.iter().all(|s| (s - 1.0).abs() < 1e-6));
    }

    /// Real-file check, run with POXIMP_TEST_FILE=<audio file>. Compares audio after a seek with
    /// the same span decoded straight from the start.
    #[test]
    fn seek_is_sample_accurate_on_real_file() {
        let Ok(path) = std::env::var("POXIMP_TEST_FILE") else { return };
        let rate = 48_000;
        let target = 20.0;
        let mut straight = Deck::open(Path::new(&path), rate).unwrap();
        let mut all = Vec::new();
        while all.len() < ((target + 1.0) * rate as f64) as usize * 2 && straight.read(&mut all).unwrap() {}
        let mut seeked = Deck::open(Path::new(&path), rate).unwrap();
        assert_eq!(seeked.seek(target), target);
        let mut after = Vec::new();
        while after.len() < 8192 && seeked.read(&mut after).unwrap() {}
        let start = (target * rate as f64) as usize;
        // Best lag of the seeked audio within +-50 ms of where it should be.
        let win = 4096;
        let probe = &after[..win * 2];
        let max_lag = rate as i64 / 20;
        let mut best = (f32::MAX, 0i64);
        for lag in -max_lag..=max_lag {
            let s = (start as i64 + lag) as usize * 2;
            let err: f32 = all[s..s + win * 2].iter().zip(probe).map(|(a, b)| (a - b).abs()).sum();
            if err < best.0 {
                best = (err, lag);
            }
        }
        let energy: f32 = probe.iter().map(|s| s.abs()).sum();
        println!("seek lag {} frames ({:.2} ms), residual {:.4} of {:.1}", best.1, best.1 as f64 * 1000.0 / rate as f64, best.0, energy);
        assert!(energy > 1.0, "test span is silent; pick another file or target");
        assert!(best.1.abs() <= rate as i64 / 1000 * 2, "seek off by {} frames", best.1);
    }
}
