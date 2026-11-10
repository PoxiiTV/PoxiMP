//! What the listener tunes: EQ curve, presets, volume normalization, output device, bit-perfect
//! mode, crossfade, the online-lyrics switch and where the phone remote listens. Persisted as JSON
//! in app data (sound.json) and pushed to the engine on startup and on every change.

use std::path::{Path, PathBuf};

use poximp_engine::{Band, EqSettings, FxSettings, Kind};
use serde::{Deserialize, Serialize};

/// The classic 10-band graphic layout.
pub const GRAPHIC: [f64; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
const GRAPHIC_Q: f64 = 1.41;
const MAX_USER_PRESETS: usize = 50;
pub const MAX_NAME: usize = 40;
pub const MAX_CROSSFADE: f64 = 12.0;
pub const MAX_DEVICE_ID: usize = 512;

/// Built-in presets: id (i18n key `eq.preset.<id>`) and gains on `GRAPHIC`.
const BUILTIN: [(&str, [f64; 10]); 8] = [
    ("flat", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("bass", [6.0, 5.0, 4.0, 2.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("treble", [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.5, 3.0, 4.5, 5.5]),
    ("vocal", [-2.0, -2.0, -1.0, 0.0, 1.5, 3.0, 3.5, 2.5, 1.0, 0.0]),
    ("rock", [4.5, 3.5, 2.0, 0.5, -1.0, -1.0, 0.5, 2.0, 3.5, 4.0]),
    ("electronic", [5.0, 4.5, 2.0, 0.0, -1.0, 0.5, 1.0, 2.0, 3.5, 4.0]),
    ("acoustic", [2.5, 2.0, 1.5, 1.0, 1.5, 1.5, 2.0, 2.5, 2.0, 1.5]),
    // Loudness contour: at low volume the ear loses lows and highs first.
    ("night", [5.0, 4.0, 2.5, 1.0, 0.0, 0.0, 0.0, 1.0, 2.5, 3.5]),
];

pub fn graphic(gains: [f64; 10]) -> Vec<Band> {
    GRAPHIC.iter().zip(gains).map(|(&freq, gain)| Band { kind: Kind::Peak, freq, gain, q: GRAPHIC_Q }).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Norm {
    #[default]
    Off,
    Track,
    Album,
    /// Only positive gain (quiet tracks come up, loud ones are left alone), limiter as safety net.
    BoostQuiet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    /// `b:<key>` built-in, `u:<n>` user
    pub id: String,
    /// i18n key for built-ins, the user's text otherwise
    pub name: String,
    pub builtin: bool,
    pub eq: EqSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub eq: EqSettings,
    /// Preset (or `autoeq:<name>`) the curve came from; None once edited by hand.
    pub preset: Option<String>,
    pub user_presets: Vec<Preset>,
    pub normalization: Norm,
    /// Listening effects (crossfeed, width, balance, bass, speed)
    pub fx: FxSettings,
    pub output: Output,
    /// Ask LRCLIB for lyrics the files do not carry (the only lookup that leaves the machine).
    pub online_lyrics: bool,
    /// Phone remote: last port it listened on, and the LAN address chosen (it starts off every
    /// launch; only where it listens is kept).
    pub remote_port: u16,
    pub remote_ip: Option<String>,
    pub quality: QualitySettings,
}

/// The fake-quality detector. Off = no background analysis and no badges anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QualitySettings {
    pub analyze: bool,
    /// Show the neutral "no signs" badge on lossy files (off = only the warnings).
    pub show_no_signs: bool,
}

impl Default for QualitySettings {
    fn default() -> Self {
        Self { analyze: true, show_no_signs: true }
    }
}

/// Where and how the sound goes out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Output {
    /// Engine device id; None = follow the system default.
    pub device_id: Option<String>,
    /// Exclusive at each file's own format: no EQ, normalization, crossfade or volume.
    pub bit_perfect: bool,
    /// Whole seconds, 1..=12; 0 = off (gapless).
    pub crossfade: f64,
    /// No crossfade between tracks of the same album (keeps gapless albums intact).
    pub keep_albums_gapless: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self { device_id: None, bit_perfect: false, crossfade: 0.0, keep_albums_gapless: true }
    }
}

impl Output {
    /// In range or off; never trusts the file (or the UI).
    pub fn sanitize(&mut self) {
        self.crossfade = if self.crossfade.is_finite() { self.crossfade.round().clamp(0.0, MAX_CROSSFADE) } else { 0.0 };
        if self.device_id.as_ref().is_some_and(|d| d.is_empty() || d.len() > MAX_DEVICE_ID || d.chars().any(char::is_control)) {
            self.device_id = None;
        }
    }

    /// Crossfade seconds from a track into the next one (album keys; empty = unknown album).
    pub fn crossfade_between(&self, cur: &str, next: &str) -> f64 {
        let same_album = !cur.is_empty() && cur == next;
        if self.bit_perfect || (self.keep_albums_gapless && same_album) { 0.0 } else { self.crossfade }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            eq: EqSettings { bands: graphic([0.0; 10]), ..Default::default() },
            preset: Some("b:flat".into()),
            user_presets: Vec::new(),
            normalization: Norm::Off,
            fx: FxSettings::default(),
            output: Output::default(),
            online_lyrics: true,
            remote_port: crate::remote::DEFAULT_PORT,
            remote_ip: None,
            quality: QualitySettings::default(),
        }
    }
}

pub fn builtins() -> Vec<Preset> {
    BUILTIN
        .iter()
        .map(|(key, gains)| Preset {
            id: format!("b:{key}"),
            name: (*key).into(),
            builtin: true,
            eq: EqSettings { bands: graphic(*gains), ..Default::default() },
        })
        .collect()
}

/// Trimmed, no control characters, 1..=MAX_NAME characters.
pub fn clean_name(name: &str) -> Option<String> {
    let n: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(MAX_NAME).collect();
    (!n.is_empty()).then_some(n)
}

pub struct Sound {
    path: PathBuf,
    pub cfg: Config,
}

impl Sound {
    /// A missing or damaged file means defaults (never a startup failure).
    pub fn load(path: &Path) -> Self {
        let mut cfg: Config = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        cfg.eq.sanitize();
        cfg.fx.sanitize();
        cfg.output.sanitize();
        if cfg.remote_port < 1024 {
            cfg.remote_port = crate::remote::DEFAULT_PORT;
        }
        if cfg.remote_ip.as_deref().is_some_and(|ip| ip.parse::<std::net::Ipv4Addr>().is_err()) {
            cfg.remote_ip = None;
        }
        cfg.user_presets.truncate(MAX_USER_PRESETS);
        cfg.user_presets.retain(|p| !p.builtin && p.id.starts_with("u:") && p.id.len() <= 32);
        for p in &mut cfg.user_presets {
            p.name = clean_name(&p.name).unwrap_or_else(|| "?".into());
            p.eq.sanitize();
        }
        Self { path: path.to_path_buf(), cfg }
    }

    /// Write-then-rename, so a crash mid-write never leaves half a file.
    pub fn save(&self) {
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string_pretty(&self.cfg) {
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    pub fn presets(&self) -> Vec<Preset> {
        builtins().into_iter().chain(self.cfg.user_presets.iter().cloned()).collect()
    }

    /// Saves the current curve under `name` (same name = overwrite). Returns its id.
    pub fn save_preset(&mut self, name: String) -> Option<String> {
        let eq = self.cfg.eq.clone();
        if let Some(p) = self.cfg.user_presets.iter_mut().find(|p| p.name.to_lowercase() == name.to_lowercase()) {
            p.eq = eq;
            return Some(p.id.clone());
        }
        if self.cfg.user_presets.len() >= MAX_USER_PRESETS {
            return None;
        }
        let n = self.cfg.user_presets.iter().filter_map(|p| p.id.strip_prefix("u:")?.parse::<u32>().ok()).max().map_or(1, |m| m + 1);
        let id = format!("u:{n}");
        self.cfg.user_presets.push(Preset { id: id.clone(), name, builtin: false, eq });
        Some(id)
    }

    pub fn delete_preset(&mut self, id: &str) -> bool {
        let before = self.cfg.user_presets.len();
        self.cfg.user_presets.retain(|p| p.id != id);
        if self.cfg.preset.as_deref() == Some(id) {
            self.cfg.preset = None;
        }
        self.cfg.user_presets.len() != before
    }
}

/// Normalization gain (dB) for a track. No ReplayGain tags = 0 (R128 scanning comes later).
pub fn gain_db(t: &poximp_library::Track, mode: Norm) -> f64 {
    let track = (t.rg_track_gain, t.rg_track_peak);
    let album = (t.rg_album_gain, t.rg_album_peak);
    let (gain, peak) = match mode {
        Norm::Off => return 0.0,
        Norm::Track | Norm::BoostQuiet => if track.0.is_some() { track } else { album },
        Norm::Album => if album.0.is_some() { album } else { track },
    };
    let Some(g) = gain.filter(|g| g.is_finite()) else { return 0.0 };
    let g = match mode {
        // The limiter catches the peaks: quiet tracks really come up.
        Norm::BoostQuiet => g.max(0.0),
        // Classic ReplayGain "prevent clipping": never lift the track's own peak past full scale.
        _ => match peak.filter(|p| p.is_finite() && *p > 0.0) {
            Some(p) if g > 0.0 => g.min((-20.0 * p.log10()).max(0.0)),
            _ => g,
        },
    };
    g.clamp(-24.0, 15.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(tg: Option<f64>, tp: Option<f64>, ag: Option<f64>) -> poximp_library::Track {
        poximp_library::Track { rg_track_gain: tg, rg_track_peak: tp, rg_album_gain: ag, ..Default::default() }
    }

    #[test]
    fn normalization_modes() {
        let loud = track(Some(-8.0), Some(1.0), Some(-6.0));
        let quiet = track(Some(5.0), Some(0.5), None);
        assert_eq!(gain_db(&loud, Norm::Off), 0.0);
        assert_eq!(gain_db(&loud, Norm::Track), -8.0);
        assert_eq!(gain_db(&loud, Norm::Album), -6.0);
        assert_eq!(gain_db(&loud, Norm::BoostQuiet), 0.0, "loud tracks are never turned down");
        assert_eq!(gain_db(&quiet, Norm::BoostQuiet), 5.0);
        assert_eq!(gain_db(&quiet, Norm::Track), 5.0);
        let g = gain_db(&track(Some(9.0), Some(0.5), None), Norm::Track);
        assert!((g - 6.0206).abs() < 0.001, "peak 0.5 allows +6 dB at most, got {g}");
        assert_eq!(gain_db(&quiet, Norm::Album), 5.0, "no album gain: track gain");
        assert_eq!(gain_db(&track(None, None, None), Norm::Track), 0.0, "untagged: untouched");
        assert_eq!(gain_db(&track(Some(f64::NAN), None, None), Norm::Track), 0.0);
    }

    #[test]
    fn presets_and_persistence() {
        let dir = std::env::temp_dir().join(format!("poximp-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sound.json");
        let _ = std::fs::remove_file(&path);
        let mut s = Sound::load(&path);
        assert_eq!(s.cfg.eq.bands.len(), 10);
        assert_eq!(s.presets().len(), BUILTIN.len());
        s.cfg.eq.bands[0].gain = 3.0;
        let id = s.save_preset(clean_name("  Mis cascos\u{7}  ").unwrap()).unwrap();
        assert_eq!(s.save_preset("mis CASCOS".into()), Some(id.clone()), "same name overwrites");
        s.cfg.normalization = Norm::BoostQuiet;
        s.cfg.remote_ip = Some("192.168.1.20".into());
        s.save();
        let back = Sound::load(&path);
        assert_eq!(back.cfg.user_presets.len(), 1);
        assert_eq!(back.cfg.user_presets[0].name, "Mis cascos");
        assert_eq!(back.cfg.user_presets[0].eq.bands[0].gain, 3.0);
        assert_eq!(back.cfg.normalization, Norm::BoostQuiet);
        assert_eq!((back.cfg.remote_port, back.cfg.remote_ip.as_deref()), (crate::remote::DEFAULT_PORT, Some("192.168.1.20")));
        let mut back = back;
        assert!(back.delete_preset(&id));
        assert!(!back.delete_preset("b:flat"), "built-ins cannot be deleted");
        std::fs::write(&path, "{ broken").unwrap();
        assert_eq!(Sound::load(&path).cfg.preset.as_deref(), Some("b:flat"), "damaged file = defaults");
        std::fs::write(&path, r#"{"remotePort":80,"remoteIp":"evil"}"#).unwrap();
        let odd = Sound::load(&path).cfg;
        assert_eq!((odd.remote_port, odd.remote_ip), (crate::remote::DEFAULT_PORT, None));
        assert_eq!(clean_name("   "), None);
    }

    #[test]
    fn output_settings() {
        let mut o = Output { device_id: Some("wasapi:{x}".into()), bit_perfect: false, crossfade: 30.0, keep_albums_gapless: true };
        o.sanitize();
        assert_eq!(o.crossfade, MAX_CROSSFADE);
        assert_eq!(o.device_id.as_deref(), Some("wasapi:{x}"));
        assert_eq!(o.crossfade_between("a", "a"), 0.0, "same album stays gapless");
        assert_eq!(o.crossfade_between("a", "b"), MAX_CROSSFADE);
        assert_eq!(o.crossfade_between("", ""), MAX_CROSSFADE, "unknown albums are not the same album");
        o.keep_albums_gapless = false;
        assert_eq!(o.crossfade_between("a", "a"), MAX_CROSSFADE);
        o.bit_perfect = true;
        assert_eq!(o.crossfade_between("a", "b"), 0.0, "never in bit-perfect mode");
        o.crossfade = f64::NAN;
        o.device_id = Some("\u{0}x".into());
        o.sanitize();
        assert_eq!((o.crossfade, o.device_id), (0.0, None));
        assert_eq!(clean_name(&"x".repeat(99)).unwrap().len(), MAX_NAME);
    }
}
