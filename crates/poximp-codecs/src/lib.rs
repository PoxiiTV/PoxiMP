//! Codec registry shared by playback and the quality detector: symphonia's own formats and
//! decoders plus Opus (libopus, preferred tier), WavPack and Monkey's Audio (APE).

use std::sync::OnceLock;

use symphonia::core::codecs::registry::CodecRegistry;
#[cfg(feature = "opus")]
use symphonia::core::common::Tier;
use symphonia::core::formats::probe::Probe;
use symphonia::core::formats::SeekTo;
use symphonia::core::units::{Time, TimeBase, Timestamp};

mod ape;

pub fn codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut r = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut r);
        #[cfg(feature = "opus")]
        r.register_audio_decoder_at_tier::<symphonia_adapter_libopus::OpusDecoder>(Tier::Preferred);
        r.register_audio_decoder::<symphonia_codec_wavpack::WavPackDecoder>();
        r
    })
}

pub fn probe() -> &'static Probe {
    static PROBE: OnceLock<Probe> = OnceLock::new();
    PROBE.get_or_init(|| {
        let mut p = Probe::default();
        symphonia::default::register_enabled_formats(&mut p);
        p.register_format::<symphonia_codec_wavpack::WavPackReader<'_>>();
        p.register_format::<ape::ApeReader<'_>>();
        p
    })
}

/// Seek target for `secs`: by time when the track has a time base, by sample otherwise (the
/// WavPack reader gives none, and then refuses time seeks).
pub fn seek_to(track_id: u32, time_base: Option<TimeBase>, rate: u32, secs: f64) -> Option<SeekTo> {
    let secs = secs.max(0.0);
    Some(match time_base {
        Some(_) => SeekTo::Time { time: Time::try_from_secs_f64(secs)?, track_id: Some(track_id) },
        None => SeekTo::Timestamp { ts: Timestamp::new((secs * f64::from(rate)).round() as i64), track_id },
    })
}
