//! Wire types of the frontend contract (src/core/types.ts) and the mappings into them.

use poximp_library as lib;
use poximp_quality as quality;
use serde::{Deserialize, Serialize, Serializer};

/// Errors as short codes for the UI to translate: never paths, SQL or stack traces.
#[derive(Debug, Clone, Copy)]
pub enum Error {
    Invalid,
    NotFound,
    Db,
    /// Download failed (offline, GitHub down) and nothing cached.
    Network,
    /// A file could not be read or written (import / export).
    Io,
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Error::Invalid => "invalid_input",
            Error::NotFound => "not_found",
            Error::Db => "library_error",
            Error::Network => "network_error",
            Error::Io => "io_error",
        })
    }
}

impl From<lib::Error> for Error {
    fn from(e: lib::Error) -> Self {
        match e {
            lib::Error::Invalid(_) => Error::Invalid,
            lib::Error::NotFound => Error::NotFound,
            lib::Error::Io(_) => Error::Io,
            lib::Error::Db(_) => Error::Db,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    pub verdict: String,
    pub confidence: f32,
    pub cutoff_hz: Option<u32>,
    pub bandwidth_hz: u32,
    pub nyquist_hz: u32,
    pub est_kbps: Option<u32>,
    pub origin: Option<String>,
    pub real_bits: Option<u32>,
    pub real_rate: Option<u32>,
}

/// Detector report -> what the UI shows. Stored as-is in `quality_json`.
pub fn report(r: &quality::Report) -> QualityReport {
    use quality::Verdict as V;
    let (d, m) = (&r.declared, &r.measured);
    let mut out = QualityReport {
        verdict: String::new(),
        confidence: m.confidence,
        cutoff_hz: m.cutoff_hz,
        bandwidth_hz: m.bandwidth_hz,
        nyquist_hz: d.sample_rate / 2,
        est_kbps: None,
        origin: None,
        real_bits: None,
        real_rate: None,
    };
    out.verdict = match r.verdict {
        V::Real => "real",
        V::NoEvidence => "noSigns",
        V::FakeBitrate { real_kbps_estimate } => {
            out.est_kbps = Some(real_kbps_estimate);
            "fake"
        }
        V::LossyOrigin { est_kbps } => {
            out.est_kbps = Some(est_kbps);
            "lossyOrigin"
        }
        V::ProbableOpusOrigin => {
            out.origin = Some("opus".into());
            "lossyOrigin"
        }
        V::FakeHiResBitDepth { real_bits } => {
            out.real_bits = Some(real_bits);
            out.real_rate = Some(d.sample_rate);
            "fakeHiRes"
        }
        V::FakeHiResSampleRate { real_rate } => {
            out.real_bits = m.effective_bit_depth.or(d.bit_depth);
            out.real_rate = Some(real_rate);
            "fakeHiRes"
        }
        V::Suspicious => {
            out.est_kbps = m.cutoff_hz.map(|f| quality::kbps_for_cutoff(f as f32));
            "suspicious"
        }
        V::Inconclusive | V::LowSampleRate => "inconclusive",
    }
    .into();
    out
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub format: String,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub bit_depth: Option<u8>,
    pub lossless: bool,
    pub quality: String,
    pub report: Option<QualityReport>,
    pub year: Option<i32>,
    pub genre: String,
    pub play_count: u32,
    /// Unix seconds
    pub last_played_at: Option<i64>,
    /// ReplayGain of the track (dB), when tagged
    pub replay_gain: Option<f64>,
    /// Opened from outside the music folders: plays, but is not in the library
    pub external: bool,
}

const LOSSLESS: &[&str] = &["FLAC", "PCM", "ALAC", "WavPack", "APE"];

/// `analyzing` = id the quality worker is on right now.
pub fn track(t: &lib::Track, analyzing: i64) -> Track {
    let lossless = LOSSLESS.contains(&t.codec.as_str());
    let quality = if t.id == analyzing {
        "analyzing".into()
    } else {
        t.quality_verdict.clone().unwrap_or_else(|| "pending".into())
    };
    Track {
        id: t.id,
        path: t.path.clone(),
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        duration: t.duration_ms as f64 / 1000.0,
        format: if t.codec == "Opus" { "OPUS".into() } else { t.container.clone() },
        bitrate: t.bitrate_kbps.unwrap_or(0),
        sample_rate: t.sample_rate.unwrap_or(0),
        bit_depth: t.bit_depth.filter(|_| lossless),
        lossless,
        quality,
        report: t.quality_json.as_deref().and_then(|j| serde_json::from_str(j).ok()),
        year: t.year,
        genre: t.genre.clone(),
        play_count: t.play_count,
        last_played_at: t.last_played_at,
        replay_gain: t.rg_track_gain,
        external: !t.in_library,
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    Artist,
    Title,
    Album,
    Duration,
    Added,
    /// File order (folder view).
    Path,
}

/// UI sort -> library sort and direction.
pub fn lib_sort(s: SortKey) -> (lib::SortKey, bool) {
    match s {
        SortKey::Artist => (lib::SortKey::Artist, false),
        SortKey::Title => (lib::SortKey::Title, false),
        SortKey::Album => (lib::SortKey::Album, false),
        SortKey::Duration => (lib::SortKey::Duration, false),
        SortKey::Added => (lib::SortKey::AddedAt, true),
        SortKey::Path => (lib::SortKey::Path, false),
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Counts {
    pub ok: u32,
    pub doubt: u32,
    pub pending: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryPage {
    pub total: u32,
    pub duration: f64,
    pub counts: Counts,
    pub tracks: Vec<Track>,
}

pub fn page(p: &lib::Page, analyzing: i64) -> LibraryPage {
    let mut counts = Counts::default();
    for (v, n) in &p.verdicts {
        match v.as_deref() {
            Some("real" | "noSigns") => counts.ok += n,
            Some("suspicious" | "fake" | "lossyOrigin" | "fakeHiRes") => counts.doubt += n,
            Some(_) => {}
            None => counts.pending += n,
        }
    }
    LibraryPage {
        total: p.total,
        duration: p.duration_ms as f64 / 1000.0,
        counts,
        tracks: p.tracks.iter().map(|t| track(t, analyzing)).collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub status: poximp_engine::PlayState,
    /// 1.0 while `volume_locked`.
    pub volume: f32,
    /// Exclusive (bit-perfect) output: the volume stays at 100 % and the slider shows it locked.
    pub volume_locked: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub index: i64,
    pub queue_length: usize,
    pub next: Option<Track>,
    /// A-B repeat of the current track: A set (b None) or looping A..B (seconds).
    pub ab: Option<AbState>,
    pub sleep: Option<SleepState>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AbState {
    pub a: f64,
    pub b: Option<f64>,
}

/// Sleep timer: pauses at `ends_at` (unix ms) or when the current track ends.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepState {
    pub ends_at: Option<u64>,
    pub end_of_track: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Position {
    pub secs: f64,
    pub duration: f64,
    /// Source seconds per second (effects speed): the UI's clock runs at this rate.
    pub speed: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayerSync {
    pub state: PlayerState,
    pub track: Option<Track>,
    pub position: Position,
}

/// A slice of the play queue, in play order (shuffle applied).
#[derive(Debug, Clone, Serialize)]
pub struct QueuePage {
    pub total: usize,
    /// Play position of the current track, -1 if none.
    pub index: i64,
    pub offset: usize,
    pub tracks: Vec<Track>,
}

/// "player:queue": contents or order changed (refetch with queue_get).
#[derive(Debug, Clone, Serialize)]
pub struct QueueEvent {
    pub total: usize,
    pub index: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub kind: lib::PlaylistKind,
    /// Smart playlists: the rule tree as sent by the UI.
    pub rules: Option<serde_json::Value>,
    pub created_at: i64,
    pub track_count: u32,
}

pub fn playlist(p: lib::Playlist) -> Playlist {
    Playlist {
        id: p.id,
        name: p.name,
        kind: p.kind,
        rules: p.rules_json.as_deref().and_then(|j| serde_json::from_str(j).ok()),
        created_at: p.created_at,
        track_count: p.track_count,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaylistPage {
    pub total: u32,
    /// Seconds, every track of the list.
    pub duration: f64,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackEvent {
    pub track: Option<Track>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityUpdate {
    pub track_id: i64,
    pub verdict: String,
    pub report: Option<QualityReport>,
}

/// "waveform:ready": fetch it with waveform_get.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveReady {
    pub track_id: i64,
}

/// Everything the sound settings show (eq_get and every EQ/normalization command return it).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqState {
    pub eq: poximp_engine::EqSettings,
    pub preset: Option<String>,
    /// Preamp actually applied (dB): the automatic one when `eq.autoPreamp`.
    pub preamp: f64,
    /// Highest boost of the curve (dB).
    pub max_boost: f64,
    pub normalization: crate::sound::Norm,
    pub presets: Vec<crate::sound::Preset>,
}

pub fn eq_state(s: &crate::sound::Sound) -> EqState {
    let c = &s.cfg;
    EqState {
        preamp: c.eq.effective_preamp(),
        max_boost: c.eq.max_boost(),
        eq: c.eq.clone(),
        preset: c.preset.clone(),
        normalization: c.normalization,
        presets: s.presets(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInfo {
    pub device: String,
    pub sample_rate: u32,
    pub bits: Option<u32>,
    /// "shared" | "exclusive" (`exclusive` spelled out)
    pub mode: &'static str,
    pub exclusive: bool,
    pub bit_perfect: bool,
    /// deviceMissing | exclusiveFailed | formatChanged (see the engine's OutputInfo)
    pub fallback: Option<&'static str>,
}

pub fn output_info(o: Option<poximp_engine::OutputInfo>) -> OutputInfo {
    match o {
        Some(o) => OutputInfo {
            // cpal formats read "f32", "i16", "i24", "u8"... (exclusive: the valid bits)
            bits: o.sample_format.trim_start_matches(char::is_alphabetic).parse().ok(),
            device: o.device,
            sample_rate: o.sample_rate,
            mode: if o.exclusive { "exclusive" } else { "shared" },
            exclusive: o.exclusive,
            bit_perfect: o.bit_perfect,
            fallback: o.fallback,
        },
        None => OutputInfo { device: String::new(), sample_rate: 0, bits: None, mode: "shared", exclusive: false, bit_perfect: false, fallback: None },
    }
}

/// The output picker: what the system offers now and what is chosen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevices {
    pub devices: Vec<poximp_engine::OutputDevice>,
    pub settings: crate::sound::Output,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_counts_and_output_bits() {
        let p = lib::Page {
            total: 9,
            duration_ms: 1500,
            verdicts: vec![
                (None, 4),
                (Some("real".into()), 1),
                (Some("noSigns".into()), 2),
                (Some("fake".into()), 1),
                (Some("inconclusive".into()), 1),
            ],
            tracks: vec![],
        };
        let c = page(&p, 0).counts;
        assert_eq!((c.ok, c.doubt, c.pending), (3, 1, 4));
        let info = |f: &str| poximp_engine::OutputInfo {
            device: "d".into(),
            device_id: "wasapi:d".into(),
            sample_rate: 48_000,
            channels: 2,
            sample_format: f.into(),
            exclusive: false,
            bit_perfect: false,
            fallback: None,
        };
        assert_eq!(output_info(Some(info("f32"))).bits, Some(32));
        assert_eq!(output_info(Some(info("i24"))).bits, Some(24));
        assert_eq!(output_info(None).bits, None);
        let ex = output_info(Some(poximp_engine::OutputInfo { exclusive: true, bit_perfect: true, ..info("i24") }));
        assert_eq!((ex.mode, ex.bits, ex.bit_perfect), ("exclusive", Some(24), true));
        assert_eq!(serde_json::to_string(&Error::NotFound).unwrap(), "\"not_found\"");
    }
}
