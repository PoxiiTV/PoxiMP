//! Audio output: the real-time renderer (pure, testable), exclusive-format negotiation and the
//! backends behind `Output` (cpal; WASAPI exclusive in `wasapi_out`, macOS hog mode in `hog`).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::OutputInfo;

/// Mono samples kept for the visualizer (power of two).
pub(crate) const VIZ_LEN: usize = 4096;
/// Ring length between producer and callback.
pub(crate) const RING_SECS: f64 = 0.2;
const RAMP_SECS: f32 = 0.008;
const VOLUME_SMOOTH_SECS: f32 = 0.010;
/// Range of the volume slider: 0 dB at the top, -50 dB just above zero.
const VOLUME_RANGE_DB: f32 = 50.0;

/// State shared between the handle, the producer thread and the callback. Atomics only.
pub(crate) struct Shared {
    /// Linear gain (f32 bits), already through `volume_gain`.
    pub volume: AtomicU32,
    pub playing: AtomicBool,
    /// Set by the producer; the callback fades out, empties the ring, zeroes `frames` and clears it.
    pub flush: AtomicBool,
    /// Frames the callback has taken from the ring since the last flush.
    pub frames: AtomicU64,
    pub rate: AtomicU32,
    pub viz: Box<[AtomicU32]>,
    pub viz_pos: AtomicUsize,
    /// Max |sample| per channel (L, R) since the last read, f32 bits (positive floats order like u32).
    pub peak: [AtomicU32; 2],
}

impl Shared {
    pub fn new() -> Self {
        Self {
            volume: AtomicU32::new(1.0f32.to_bits()),
            playing: AtomicBool::new(false),
            flush: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            rate: AtomicU32::new(48_000),
            viz: (0..VIZ_LEN).map(|_| AtomicU32::new(0)).collect(),
            viz_pos: AtomicUsize::new(0),
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
        }
    }

    /// Newest `VIZ_LEN` mono samples, oldest first.
    pub fn viz_samples(&self) -> impl Iterator<Item = f32> + '_ {
        let end = self.viz_pos.load(Acquire);
        (0..VIZ_LEN).map(move |i| f32::from_bits(self.viz[end.wrapping_add(i) & (VIZ_LEN - 1)].load(Relaxed)))
    }
}

/// Slider position 0..1 -> linear gain on a dB (perceptual) curve.
pub fn volume_gain(v: f32) -> f32 {
    if v <= 0.0 {
        0.0
    } else {
        10f32.powf((v.min(1.0) - 1.0) * VOLUME_RANGE_DB / 20.0)
    }
}

/// Runs inside the audio callback: no locks, no allocations, no logging.
pub(crate) struct Renderer {
    ring: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
    ramp: f32,
    ramp_step: f32,
    vol: f32,
    vol_coef: f32,
    viz_pos: usize,
    /// Bit-perfect: volume stays at 100 % whatever the slider says.
    unity: bool,
}

impl Renderer {
    pub fn new(ring: rtrb::Consumer<f32>, shared: Arc<Shared>, rate: u32, unity: bool) -> Self {
        let rate = rate as f32;
        Self {
            vol: if unity { 1.0 } else { f32::from_bits(shared.volume.load(Relaxed)) },
            unity,
            viz_pos: shared.viz_pos.load(Relaxed),
            ring,
            shared,
            ramp: 0.0,
            ramp_step: 1.0 / (rate * RAMP_SECS),
            vol_coef: 1.0 - (-1.0 / (rate * VOLUME_SMOOTH_SECS)).exp(),
        }
    }

    /// Fills interleaved `out` with `channels` per frame from the stereo ring.
    pub fn render<T: Copy>(&mut self, out: &mut [T], channels: usize, conv: impl Fn(f32) -> T) {
        let flush = self.shared.flush.load(Acquire);
        let target = if !flush && self.shared.playing.load(Relaxed) { 1.0 } else { 0.0 };
        let vol_target = if self.unity { 1.0 } else { f32::from_bits(self.shared.volume.load(Relaxed)) };
        let frames = out.len() / channels;
        let mut take = (self.ring.slots() / 2).min(frames);
        if target == 0.0 {
            // Only consume what the fade-out needs, so pause resumes where it stopped.
            take = take.min((self.ramp / self.ramp_step).ceil() as usize);
        }
        let chunk = self.ring.read_chunk(take * 2).expect("take <= slots");
        let (a, b) = chunk.as_slices();
        let at = |i: usize| if i < a.len() { a[i] } else { b[i - a.len()] };
        let (mut pl, mut pr) = (0f32, 0f32);
        for (i, frame) in out.chunks_exact_mut(channels).enumerate() {
            let (l, r) = if i < take {
                self.ramp = if self.ramp < target {
                    (self.ramp + self.ramp_step).min(1.0)
                } else {
                    (self.ramp - self.ramp_step).max(target)
                };
                (at(2 * i), at(2 * i + 1))
            } else {
                // No audio (paused, flushed or underrun): next audio fades in, never clicks in.
                self.ramp = 0.0;
                (0.0, 0.0)
            };
            self.vol += (vol_target - self.vol) * self.vol_coef;
            // Visualizer tap before volume, so it does not die at low volume.
            self.shared.viz[self.viz_pos & (VIZ_LEN - 1)].store((0.5 * (l + r)).to_bits(), Relaxed);
            self.viz_pos = self.viz_pos.wrapping_add(1);
            pl = pl.max(l.abs());
            pr = pr.max(r.abs());
            let g = self.vol * self.ramp;
            match frame {
                [m] => *m = conv(0.5 * (l + r) * g),
                [fl, fr, rest @ ..] => {
                    *fl = conv(l * g);
                    *fr = conv(r * g);
                    rest.iter_mut().for_each(|s| *s = conv(0.0));
                }
                [] => {}
            }
        }
        chunk.commit_all();
        self.shared.viz_pos.store(self.viz_pos, Release);
        self.shared.peak[0].fetch_max(pl.to_bits(), Relaxed);
        self.shared.peak[1].fetch_max(pr.to_bits(), Relaxed);
        self.shared.frames.fetch_add(take as u64, Release);
        if flush && self.ramp == 0.0 {
            let rest = self.ring.slots();
            if let Ok(c) = self.ring.read_chunk(rest) {
                c.commit_all();
            }
            self.shared.frames.store(0, Release);
            self.shared.flush.store(false, Release);
        }
    }
}

/// An output backend: cpal (shared mode, and the macOS hog-mode path) or WASAPI exclusive.
pub(crate) trait Output {
    fn info(&self) -> &OutputInfo;
    fn info_mut(&mut self) -> &mut OutputInfo;
    /// The stream died (unplugged, invalidated, callback stopped): the engine rebuilds it.
    fn broken(&self) -> bool;
}

pub(crate) type Opened = (Box<dyn Output>, rtrb::Producer<f32>);

/// What a file asks of an exclusive output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Format {
    pub rate: u32,
    /// None for lossy sources (their decoders output float).
    pub bits: Option<u32>,
}

/// Rates exclusive mode falls back to when the file's own one is refused.
#[cfg_attr(not(windows), allow(dead_code))] // integer formats: WASAPI only
const RATES: [u32; 8] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000];

/// Integer formats to try in exclusive mode, best first: (rate, container bits, valid bits).
/// The file's rate before any other, then the nearest ones; per rate, the depths that carry
/// every source bit first (24-bit audio in 32-bit containers first: many DACs only take that).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn exclusive_candidates(f: Format) -> Vec<(u32, u16, u16)> {
    let depths: &[(u16, u16)] = match f.bits {
        Some(b) if b <= 16 => &[(16, 16), (32, 24), (24, 24), (32, 32)],
        Some(b) if b <= 24 => &[(32, 24), (24, 24), (32, 32), (16, 16)],
        _ => &[(32, 32), (32, 24), (24, 24), (16, 16)],
    };
    let distance = |r: u32| (f64::from(r) / f64::from(f.rate.max(1))).ln().abs();
    let mut rates: Vec<u32> = RATES.iter().copied().filter(|&r| r != f.rate).collect();
    // Nearest first; on a tie the higher rate (never throw bandwidth away).
    rates.sort_by(|&a, &b| distance(a).total_cmp(&distance(b)).then(b.cmp(&a)));
    rates.insert(0, f.rate);
    rates.into_iter().flat_map(|r| depths.iter().map(move |&(s, v)| (r, s, v))).collect()
}

/// The device gets the file's samples untouched: same rate and every source bit carried. The
/// ring is f32, exact for integers of up to 24 bits, so 32-bit integer files never qualify.
/// Lossy files (no bit depth) count when the decoder's output reaches the DAC at 24 bits or more.
pub(crate) fn is_bit_perfect(f: Format, rate: u32, valid_bits: u16) -> bool {
    let bits = f.bits.unwrap_or(24);
    rate == f.rate && bits <= 24 && bits <= u32::from(valid_bits)
}

/// f32 -> little-endian signed integer `out.len()` bytes wide (2, 3 or 4), MSB aligned.
/// Exact for samples that came from integers of up to 24 bits.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn to_int_le(s: f32, out: &mut [u8]) {
    let n = out.len();
    let scale = (1i64 << (8 * n - 1)) as f64;
    let v = (f64::from(s) * scale).round().clamp(-scale, scale - 1.0) as i64;
    out.copy_from_slice(&v.to_le_bytes()[..n]);
}

/// cpal id ("wasapi:{...}", "coreaudio:<uid>") of the system default output.
pub(crate) fn default_id() -> Option<String> {
    cpal::default_host().default_output_device().and_then(|d| d.id().ok()).map(|id| id.to_string())
}

/// Output devices for the settings list: (cpal id, name, is the system default).
pub(crate) fn devices() -> Vec<(String, String, bool)> {
    let default = default_id();
    let Ok(list) = cpal::default_host().output_devices() else { return Vec::new() };
    list.filter_map(|d| {
        let id = d.id().ok()?.to_string();
        let name = d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| id.clone());
        let is_default = default.as_deref() == Some(id.as_str());
        Some((id, name, is_default))
    })
    .collect()
}

/// Opens `device` (cpal id; None or gone = the system default). With `exclusive` it tries
/// bit-perfect first and falls back to shared mode; `OutputInfo::fallback` says what was lost.
pub(crate) fn open(shared: &Arc<Shared>, device: Option<&str>, exclusive: Option<Format>) -> Result<Opened, &'static str> {
    let host = cpal::default_host();
    let chosen = device.and_then(|id| id.parse::<cpal::DeviceId>().ok()).and_then(|id| host.device_by_id(&id));
    let mut fallback = (device.is_some() && chosen.is_none()).then_some("deviceMissing");
    let device = chosen.or_else(|| host.default_output_device()).ok_or("output_unavailable")?;
    if let Some(f) = exclusive {
        match open_exclusive(shared, &device, f) {
            Ok(mut o) => {
                let info = o.0.info_mut();
                info.fallback = fallback.or(info.fallback);
                return Ok(o);
            }
            Err(why) => fallback = Some(why),
        }
    }
    open_cpal(shared, &device, None, fallback)
}

/// Busy, refused or no usable format: shared mode instead.
const EXCLUSIVE_FAILED: &str = "exclusiveFailed";

/// Err = the `OutputInfo::fallback` code saying why.
#[cfg(windows)]
fn open_exclusive(shared: &Arc<Shared>, device: &cpal::Device, f: Format) -> Result<Opened, &'static str> {
    let id = device.id().map_err(|_| EXCLUSIVE_FAILED)?;
    crate::wasapi_out::open(shared, id.id(), id.to_string(), f)
}

/// Hogs the device and switches its nominal rate to the file's, then streams float at unity
/// gain through cpal (CoreAudio turns float into the DAC's integer format losslessly).
#[cfg(target_os = "macos")]
fn open_exclusive(shared: &Arc<Shared>, device: &cpal::Device, f: Format) -> Result<Opened, &'static str> {
    let id = device.id().map_err(|_| EXCLUSIVE_FAILED)?;
    let (hog, rate) = crate::hog::Hog::take(id.id(), f.rate).ok_or(EXCLUSIVE_FAILED)?;
    let mut o = open_cpal(shared, device, Some((hog, rate)), None).map_err(|_| EXCLUSIVE_FAILED)?;
    let info = o.0.info_mut();
    info.bit_perfect = is_bit_perfect(f, info.sample_rate, 24);
    info.fallback = (!info.bit_perfect).then_some("formatChanged");
    Ok(o)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn open_exclusive(_: &Arc<Shared>, _: &cpal::Device, _: Format) -> Result<Opened, &'static str> {
    Err(EXCLUSIVE_FAILED)
}

/// macOS exclusive: the hog (released when the output drops) and the rate the device runs at.
#[cfg(target_os = "macos")]
type Hogged = Option<(crate::hog::Hog, u32)>;
#[cfg(not(target_os = "macos"))]
type Hogged = Option<(std::convert::Infallible, u32)>;

struct CpalOutput {
    _stream: cpal::Stream,
    /// Declared after the stream: fields drop in order, so the hog goes once the stream stopped.
    #[cfg(target_os = "macos")]
    _hog: Option<crate::hog::Hog>,
    broken: Arc<AtomicBool>,
    info: OutputInfo,
}

impl Output for CpalOutput {
    fn info(&self) -> &OutputInfo {
        &self.info
    }

    fn info_mut(&mut self) -> &mut OutputInfo {
        &mut self.info
    }

    fn broken(&self) -> bool {
        self.broken.load(Relaxed)
    }
}

/// Shared mode at the device mix rate or, when `hogged`, stereo f32 at the rate the device was
/// switched to and at unity gain.
fn open_cpal(shared: &Arc<Shared>, device: &cpal::Device, hogged: Hogged, fallback: Option<&'static str>) -> Result<Opened, &'static str> {
    let (format, config, unity) = match &hogged {
        Some((_, rate)) => {
            let config = cpal::StreamConfig { channels: 2, sample_rate: *rate, buffer_size: cpal::BufferSize::Default };
            (SampleFormat::F32, config, true)
        }
        None => {
            let supported = device.default_output_config().map_err(|_| "output_unavailable")?;
            (supported.sample_format(), supported.config(), false)
        }
    };
    let rate = config.sample_rate;
    let cap = ((f64::from(rate) * RING_SECS) as usize) * 2;
    let (producer, consumer) = rtrb::RingBuffer::new(cap);
    shared.rate.store(rate, Relaxed);
    shared.flush.store(false, Relaxed);
    shared.frames.store(0, Relaxed);
    let renderer = Renderer::new(consumer, shared.clone(), rate, unity);
    let broken = Arc::new(AtomicBool::new(false));
    let info = OutputInfo {
        device: device.description().map(|d| d.name().to_string()).unwrap_or_default(),
        device_id: device.id().map(|id| id.to_string()).unwrap_or_default(),
        sample_rate: rate,
        channels: config.channels,
        sample_format: format.to_string(),
        exclusive: unity,
        bit_perfect: false,
        fallback,
    };
    let stream = match format {
        SampleFormat::F32 => build::<f32>(device, config, renderer, &broken),
        SampleFormat::F64 => build::<f64>(device, config, renderer, &broken),
        SampleFormat::I16 => build::<i16>(device, config, renderer, &broken),
        SampleFormat::I24 => build::<cpal::I24>(device, config, renderer, &broken),
        SampleFormat::I32 => build::<i32>(device, config, renderer, &broken),
        SampleFormat::U16 => build::<u16>(device, config, renderer, &broken),
        SampleFormat::U8 => build::<u8>(device, config, renderer, &broken),
        _ => return Err("output_unsupported"),
    }
    .map_err(|_| "output_unavailable")?;
    stream.play().map_err(|_| "output_unavailable")?;
    let out = CpalOutput {
        _stream: stream,
        #[cfg(target_os = "macos")]
        _hog: hogged.map(|(h, _)| h),
        broken,
        info,
    };
    Ok((Box::new(out), producer))
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut renderer: Renderer,
    broken: &Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = config.channels as usize;
    let broken = broken.clone();
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| renderer.render(data, channels, T::from_sample),
        move |e: cpal::Error| {
            if !matches!(e.kind(), cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied) {
                broken.store(true, Relaxed);
            }
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(rate: u32) -> (Arc<Shared>, rtrb::Producer<f32>, Renderer) {
        let shared = Arc::new(Shared::new());
        let (p, c) = rtrb::RingBuffer::new(8192);
        let r = Renderer::new(c, shared.clone(), rate, false);
        (shared, p, r)
    }

    #[test]
    fn exclusive_formats_try_the_file_first_then_the_nearest() {
        let cd = exclusive_candidates(Format { rate: 44_100, bits: Some(16) });
        assert_eq!(cd[0], (44_100, 16, 16), "the file's own format first");
        assert_eq!(cd[1], (44_100, 32, 24), "then 16 bits padded into 24-in-32 (still bit-perfect)");
        assert!(cd.iter().take(4).all(|c| c.0 == 44_100), "every depth at the file's rate before another rate");
        assert_eq!(cd[4].0, 48_000, "nearest rate next");
        assert_eq!(cd.len(), RATES.len() * 4, "no duplicate rates");

        let hi = exclusive_candidates(Format { rate: 96_000, bits: Some(24) });
        assert_eq!(&hi[..3], &[(96_000, 32, 24), (96_000, 24, 24), (96_000, 32, 32)]);
        assert_eq!(hi[3], (96_000, 16, 16), "truncating to 16 bits is the last resort");
        assert_eq!(hi[4].0, 88_200, "88.2 k is nearer to 96 k than 192 k");

        // 88.2 k: 96 k is nearer than 44.1 k or 176.4 k.
        assert_eq!(exclusive_candidates(Format { rate: 88_200, bits: Some(24) })[4].0, 96_000);
        // A rate outside the list still comes first; distance is a ratio (octaves), not Hz.
        let odd = exclusive_candidates(Format { rate: 22_050, bits: None });
        assert_eq!(odd[0], (22_050, 32, 32));
        assert_eq!(odd[4].0, 44_100);
        assert_eq!(exclusive_candidates(Format { rate: 66_000, bits: None })[4].0, 88_200);
    }

    #[test]
    fn bit_perfect_needs_the_rate_and_every_bit() {
        let f16 = Format { rate: 44_100, bits: Some(16) };
        assert!(is_bit_perfect(f16, 44_100, 16));
        assert!(is_bit_perfect(f16, 44_100, 24), "padding adds no change");
        assert!(!is_bit_perfect(f16, 48_000, 24), "resampled");
        let f24 = Format { rate: 96_000, bits: Some(24) };
        assert!(is_bit_perfect(f24, 96_000, 24));
        assert!(!is_bit_perfect(f24, 96_000, 16), "truncated");
        assert!(!is_bit_perfect(Format { rate: 48_000, bits: Some(32) }, 48_000, 32), "f32 ring cannot carry 32 bits");
        assert!(is_bit_perfect(Format { rate: 48_000, bits: None }, 48_000, 24), "lossy: decoder output at 24 bits");
        assert!(!is_bit_perfect(Format { rate: 48_000, bits: None }, 48_000, 16));
    }

    #[test]
    fn int_conversion_round_trips_16_and_24_bit_samples() {
        let mut b2 = [0u8; 2];
        let mut b3 = [0u8; 3];
        let mut b4 = [0u8; 4];
        for k in [-32_768i32, -12_345, -1, 0, 1, 777, 32_767] {
            let s = k as f32 / 32_768.0; // as symphonia hands 16-bit samples over
            to_int_le(s, &mut b2);
            assert_eq!(i16::from_le_bytes(b2), k as i16);
            to_int_le(s, &mut b4);
            assert_eq!(i32::from_le_bytes(b4), k << 16, "16 bits MSB-aligned in 32");
        }
        for k in [-8_388_608i32, -4_000_001, -1, 0, 1, 5_555_555, 8_388_607] {
            let s = k as f32 / 8_388_608.0;
            to_int_le(s, &mut b4);
            assert_eq!(i32::from_le_bytes(b4), k << 8, "24-in-32, low byte zero");
            to_int_le(s, &mut b3);
            assert_eq!(i32::from_le_bytes([0, b3[0], b3[1], b3[2]]) >> 8, k, "packed 24");
        }
        to_int_le(1.5, &mut b2);
        assert_eq!(i16::from_le_bytes(b2), i16::MAX, "clipped, never wrapped");
        to_int_le(-1.5, &mut b2);
        assert_eq!(i16::from_le_bytes(b2), i16::MIN);
    }

    #[test]
    fn unity_renderer_ignores_volume_and_passes_samples_untouched() {
        let shared = Arc::new(Shared::new());
        let (mut p, c) = rtrb::RingBuffer::new(8192);
        let mut r = Renderer::new(c, shared.clone(), 1000, true);
        shared.playing.store(true, Relaxed);
        shared.volume.store(0.1f32.to_bits(), Relaxed);
        let src: Vec<f32> = (0..400).map(|i| ((i * 37) % 200) as f32 / 256.0 - 0.4).collect();
        p.push_entire_slice(&src).unwrap();
        let mut out = [0.0f32; 400];
        r.render(&mut out, 2, |s| s);
        assert_eq!(&out[2 * 8..], &src[2 * 8..], "after the 8-frame fade-in: bit-identical");
    }

    #[test]
    fn volume_curve() {
        assert_eq!(volume_gain(0.0), 0.0);
        assert_eq!(volume_gain(-1.0), 0.0);
        assert!((volume_gain(1.0) - 1.0).abs() < 1e-6);
        assert!((volume_gain(2.0) - 1.0).abs() < 1e-6);
        let half_db = 20.0 * volume_gain(0.5).log10();
        assert!((half_db + 25.0).abs() < 0.01, "half slider = -25 dB, got {half_db}");
        let mut last = 0.0;
        for i in 1..=100 {
            let g = volume_gain(i as f32 / 100.0);
            assert!(g > last);
            last = g;
        }
    }

    #[test]
    fn plays_with_fade_in_and_counts_frames() {
        let (shared, mut p, mut r) = setup(1000); // 8 frames of ramp
        shared.playing.store(true, Relaxed);
        p.push_entire_slice(&[1.0; 40]).unwrap(); // 20 frames
        let mut out = [0.0f32; 60]; // 30 frames
        r.render(&mut out, 2, |s| s);
        assert_eq!(shared.frames.load(Relaxed), 20);
        assert!(out[0] > 0.0 && out[0] < 0.2, "starts faded, got {}", out[0]);
        assert!((out[2 * 15] - 1.0).abs() < 1e-6, "full level after the ramp");
        assert!(out[40..].iter().all(|&s| s == 0.0), "underrun = silence");
    }

    #[test]
    fn flush_fades_out_empties_ring_and_resets() {
        let (shared, mut p, mut r) = setup(1000);
        shared.playing.store(true, Relaxed);
        p.push_entire_slice(&[1.0; 4000]).unwrap();
        let mut out = [0.0f32; 40];
        r.render(&mut out, 2, |s| s); // reach full level
        shared.flush.store(true, Release);
        r.render(&mut out, 2, |s| s);
        assert!(out[0] < 1.0 && out[0] > 0.8, "fade-out starts from full level");
        assert!(out[2 * 8..].iter().all(|&s| s == 0.0), "silent after the ramp");
        assert!(!shared.flush.load(Acquire), "flag cleared");
        assert_eq!(shared.frames.load(Acquire), 0);
        assert_eq!(p.slots(), 8192, "ring emptied");
    }

    #[test]
    fn pause_consumes_only_the_fade() {
        let (shared, mut p, mut r) = setup(1000);
        shared.playing.store(true, Relaxed);
        p.push_entire_slice(&[0.5; 400]).unwrap();
        let mut out = [0.0f32; 40];
        r.render(&mut out, 2, |s| s);
        shared.playing.store(false, Relaxed);
        r.render(&mut out, 2, |s| s);
        assert_eq!(shared.frames.load(Relaxed), 20 + 8);
        r.render(&mut out, 2, |s| s);
        assert_eq!(shared.frames.load(Relaxed), 28, "paused: ring untouched");
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn maps_stereo_to_device_channels_and_taps_viz_before_volume() {
        let (shared, mut p, mut r) = setup(1000);
        shared.playing.store(true, Relaxed);
        shared.volume.store(0.0f32.to_bits(), Relaxed);
        p.push_entire_slice(&[0.2, 0.4].repeat(50)).unwrap();
        let mut out = [9.0f32; 4 * 20];
        r.render(&mut out, 4, |s| s);
        assert!(out.chunks(4).all(|f| f[2] == 0.0 && f[3] == 0.0), "extra channels silent");
        let last = shared.viz_samples().last().unwrap();
        assert!((last - 0.3).abs() < 1e-6, "viz gets the mono mix at volume 0");
        let peaks = shared.peak.each_ref().map(|p| f32::from_bits(p.swap(0, Relaxed)));
        assert_eq!(peaks, [0.2, 0.4], "L/R peaks before volume");
        let mut mono = [9.0f32; 10];
        r.render(&mut mono, 1, |s| s);
        assert!(mono.iter().all(|&s| s.abs() < 0.3));
    }
}
