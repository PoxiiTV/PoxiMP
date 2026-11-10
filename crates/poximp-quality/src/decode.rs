//! Sampled decode with an own symphonia reader (fakequality.md §3.1) + MP3 LAME tag (§3.0).

use crate::spectrum::{Accumulator, Spectrum};
use crate::{Declared, Error, Options};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use symphonia::core::audio::{Audio, GenericAudioBufferRef};
use symphonia::core::codecs::audio::well_known::profiles::{
    CODEC_PROFILE_AAC_HE, CODEC_PROFILE_AAC_HE_V2,
};
use symphonia::core::codecs::audio::well_known::*;
use symphonia::core::codecs::audio::{
    AudioCodecId, AudioCodecParameters, AudioDecoder, AudioDecoderOptions,
};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::TimeBase;

/// Tracks shorter than this are analysed whole.
const FULL_SCAN_SECS: f64 = 60.0;
/// Segments are spread over this fraction of the track (skips intro and fade-out).
const SPREAD_START: f64 = 0.05;
const SPREAD_END: f64 = 0.95;
/// One segment per this many seconds of track, clamped to [MIN_SEGMENTS, opts.max_segments].
const SECS_PER_SEGMENT: f64 = 10.0;
const MIN_SEGMENTS: usize = 8;
/// After a seek the MP3 bit reservoir produces broadband garbage: drop this much.
const DISCARD_AFTER_SEEK_SECS: f64 = 0.05;
/// Bytes read from the file head when looking for the LAME tag.
const LAME_SEARCH_BYTES: usize = 256 * 1024;

pub struct Decoded {
    pub declared: Declared,
    pub spectrum: Spectrum,
    /// OR of all MSB-aligned integer samples (lossless sources only).
    pub or_bits: Option<u32>,
}

/// A file opened for decoding: its container reader and a decoder for the default audio track.
pub(crate) struct Source {
    pub format: Box<dyn FormatReader>,
    pub decoder: Box<dyn AudioDecoder>,
    pub track_id: u32,
    pub time_base: Option<TimeBase>,
    pub params: AudioCodecParameters,
    pub fs: u32,
    pub channels: u32,
    /// Seconds, when the container says.
    pub duration: Option<f64>,
    pub file_len: u64,
}

pub(crate) fn open(path: &Path) -> Result<Source, Error> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format = poximp_codecs::probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|_| Error::Unsupported)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or(Error::NoAudioTrack)?;
    let track_id = track.id;
    let time_base = track.time_base;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(Error::NoAudioTrack)?
        .clone();
    let fs = params.sample_rate.ok_or(Error::Unsupported)?;
    let channels = params.channels.as_ref().map_or(0, |c| c.count()) as u32;
    let duration = track
        .num_frames
        .zip(track.time_base)
        .map(|(n, tb)| {
            tb.calc_time_saturating(symphonia::core::units::Timestamp::new(n as i64))
                .as_secs_f64()
        })
        .or_else(|| track.num_frames.map(|n| n as f64 / fs as f64));
    let decoder = poximp_codecs::codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|_| Error::Unsupported)?;
    Ok(Source {
        format,
        decoder,
        track_id,
        time_base,
        params,
        fs,
        channels,
        duration,
        file_len,
    })
}

pub fn decode(path: &Path, opts: &Options) -> Result<Decoded, Error> {
    let Source {
        mut format,
        mut decoder,
        track_id,
        time_base,
        params,
        fs,
        channels,
        duration,
        file_len,
    } = open(path)?;
    let container = format.format_info().short_name;

    // APE packets are already PCM (see poximp-codecs): name it after the container.
    let (codec, lossless) = if container == "ape" { ("APE", true) } else { codec_name(params.codec) };
    let he_aac = params.codec == CODEC_ID_AAC
        && matches!(params.profile, Some(p) if p == CODEC_PROFILE_AAC_HE || p == CODEC_PROFILE_AAC_HE_V2);
    let lame_lowpass_hz = if params.codec == CODEC_ID_MP3 {
        lame_lowpass(path)
    } else {
        None
    };

    // Segment plan: start times; None = decode sequentially from the start.
    let seg_secs = opts.segment_secs.max(0.5);
    let plan: Vec<Option<f64>> = match duration {
        Some(d) if d >= FULL_SCAN_SECS => {
            let k = ((d / SECS_PER_SEGMENT) as usize).clamp(MIN_SEGMENTS, opts.max_segments.max(1));
            let (a, b) = (d * SPREAD_START, d * SPREAD_END - seg_secs);
            (0..k)
                .map(|i| Some(a + (b - a) * i as f64 / (k - 1).max(1) as f64))
                .collect()
        }
        _ => vec![None],
    };
    let budget = match plan[0] {
        Some(_) => (seg_secs * fs as f64) as usize,
        None => (FULL_SCAN_SECS * fs as f64) as usize,
    };

    let mut acc = Accumulator::new(fs, channels as usize);
    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut or_bits = 0u32;
    let (mut bytes, mut frames_out) = (0u64, 0u64);
    for start in plan {
        let mut skip = 0usize;
        if let Some(t) = start {
            let Some(to) = poximp_codecs::seek_to(track_id, time_base, fs, t) else {
                continue;
            };
            if format.seek(SeekMode::Coarse, to).is_err() {
                continue;
            }
            decoder.reset();
            skip = (DISCARD_AFTER_SEEK_SECS * fs as f64) as usize;
        }
        acc.reset_segment();
        let mut want = budget;
        while want > 0 {
            let packet = match format.next_packet() {
                Ok(Some(p)) => p,
                _ => break,
            };
            if packet.track_id != track_id {
                continue;
            }
            let buf = match decoder.decode(&packet) {
                Ok(b) => b,
                Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => continue,
                Err(_) => break,
            };
            bytes += packet.data.len() as u64;
            frames_out += buf.frames() as u64;
            if lossless {
                or_bits |= or_samples(&buf);
            }
            buf.copy_to_vecs_planar::<f32>(&mut planes);
            let len = buf.frames();
            let from = skip.min(len);
            skip -= from;
            let to = (from + want).min(len);
            let refs: Vec<&[f32]> = planes.iter().map(|p| &p[from..to]).collect();
            acc.push(&refs);
            want -= to - from;
        }
    }

    // APE packets are decompressed PCM: only the file size tells its bitrate.
    let bitrate_kbps = (frames_out > 0 && container != "ape")
        .then(|| (bytes as f64 * 8.0 / (frames_out as f64 / fs as f64) / 1000.0).round() as u32)
        .or_else(|| duration.map(|d| (file_len as f64 * 8.0 / d / 1000.0).round() as u32));
    Ok(Decoded {
        declared: Declared {
            codec,
            container,
            bitrate_kbps,
            sample_rate: fs,
            bit_depth: params.bits_per_sample,
            channels,
            lossless,
            duration_secs: duration,
            lame_lowpass_hz,
            he_aac,
        },
        spectrum: acc.finish(),
        or_bits: lossless.then_some(or_bits),
    })
}

fn codec_name(id: AudioCodecId) -> (&'static str, bool) {
    match id {
        CODEC_ID_MP3 => ("MP3", false),
        CODEC_ID_MP2 => ("MP2", false),
        CODEC_ID_MP1 => ("MP1", false),
        CODEC_ID_AAC => ("AAC", false),
        CODEC_ID_VORBIS => ("Vorbis", false),
        CODEC_ID_OPUS => ("Opus", false),
        CODEC_ID_FLAC => ("FLAC", true),
        CODEC_ID_ALAC => ("ALAC", true),
        CODEC_ID_WAVPACK => ("WavPack", true),
        // Well-known PCM codec ids are the range just below the first ADPCM id.
        _ if id >= CODEC_ID_PCM_S32LE && id < CODEC_ID_ADPCM_G722 => ("PCM", true),
        _ => ("?", false),
    }
}

/// OR of all samples aligned to the MSB of an i32 (so `32 - trailing_zeros` = used bits).
pub fn or_samples(buf: &GenericAudioBufferRef) -> u32 {
    fn fold<S: Copy>(planes: impl Iterator<Item = S>, f: impl Fn(S) -> u32) -> u32 {
        planes.fold(0, |a, s| a | f(s))
    }
    macro_rules! planes {
        ($b:expr) => {
            (0..$b.num_planes()).flat_map(|i| $b.plane(i).unwrap_or(&[]).iter().copied())
        };
    }
    match buf {
        GenericAudioBufferRef::S8(b) => fold(planes!(b), |s| ((s as i32) << 24) as u32),
        GenericAudioBufferRef::S16(b) => fold(planes!(b), |s| ((s as i32) << 16) as u32),
        GenericAudioBufferRef::S24(b) => fold(planes!(b), |s| (s.inner() << 8) as u32),
        GenericAudioBufferRef::S32(b) => fold(planes!(b), |s| s as u32),
        GenericAudioBufferRef::U8(b) => fold(planes!(b), |s| ((s as i32 - 0x80) << 24) as u32),
        GenericAudioBufferRef::U16(b) => fold(planes!(b), |s| ((s as i32 - 0x8000) << 16) as u32),
        GenericAudioBufferRef::U24(b) => {
            fold(planes!(b), |s| ((s.inner() as i32 - 0x80_0000) << 8) as u32)
        }
        GenericAudioBufferRef::U32(b) => fold(planes!(b), |s| s ^ 0x8000_0000),
        GenericAudioBufferRef::F32(b) => fold(planes!(b), |s| float_bits(s as f64)),
        GenericAudioBufferRef::F64(b) => fold(planes!(b), float_bits),
    }
}

/// Float sample as an MSB-aligned i32; values that are not on the 2^-31 grid count as full 32 bits.
pub fn float_bits(x: f64) -> u32 {
    let v = x * 2147483648.0;
    let r = v.round().clamp(i32::MIN as f64, i32::MAX as f64);
    (r as i32 as u32) | u32::from(r != v)
}

/// Lowpass byte of the LAME/Info tag (units of 100 Hz), as written by LAME and ffmpeg's libmp3lame.
fn lame_lowpass(path: &Path) -> Option<u32> {
    let mut head = Vec::with_capacity(LAME_SEARCH_BYTES);
    File::open(path)
        .ok()?
        .take(LAME_SEARCH_BYTES as u64)
        .read_to_end(&mut head)
        .ok()?;
    let mut pos = 0;
    if head.len() >= 10 && &head[..3] == b"ID3" {
        let sz = head[6..10]
            .iter()
            .fold(0usize, |a, &b| (a << 7) | (b & 0x7f) as usize);
        pos = 10 + sz + if head[5] & 0x10 != 0 { 10 } else { 0 };
    }
    // First frame sync, then the Xing/Info header inside that frame.
    let sync = (pos..head.len().saturating_sub(1))
        .find(|&i| head[i] == 0xff && head[i + 1] & 0xe0 == 0xe0)?;
    let win = &head[sync..(sync + 200).min(head.len())];
    let x = win.windows(4).position(|w| w == b"Xing" || w == b"Info")? + sync;
    let flags = u32::from_be_bytes(head.get(x + 4..x + 8)?.try_into().ok()?);
    let mut lame = x + 8;
    for (bit, size) in [(1, 4), (2, 4), (4, 100), (8, 4)] {
        if flags & bit != 0 {
            lame += size;
        }
    }
    let tag = head.get(lame..lame + 11)?;
    if !tag[..4].iter().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    match tag[10] {
        0 => None,
        v => Some(v as u32 * 100),
    }
}
