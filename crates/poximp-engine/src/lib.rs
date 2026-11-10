//! PoxiMP playback engine: symphonia (decode) -> rubato -> rtrb (SPSC ring) -> cpal (output).
//!
//! - `decode`   one open file: gapless decode, sample-accurate seek, channel mapping, resampling
//! - `producer` engine thread: commands, decks A/B (gapless preload, crossfade), ring filling,
//!   bit-perfect reopening per file format, events
//! - `output`   real-time renderer (volume, fades, frame counter, viz tap) + `Output` backends:
//!   cpal, `wasapi_out` (Windows exclusive), `hog` (macOS hog mode + nominal rate)
//! - `eq`       gain (preamp + normalization), parametric EQ and safety limiter, before the ring
//! - `fx`       listening effects inside that chain (bass, width/mono, crossfeed, balance)
//! - `spectrum` pull visualizer bands
//!
//! The play queue lives in the app: on `TrackChanged` it enqueues the following track.

use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use serde::Serialize;

mod decode;
mod eq;
mod fx;
#[cfg(target_os = "macos")]
mod hog;
mod output;
mod producer;
mod spectrum;
#[cfg(windows)]
mod wasapi_out;

pub use eq::{Band, EqSettings, Kind, MAX_BANDS};
pub use fx::{Crossfeed, FxSettings, MAX_BASS_DB, SPEED_RANGE};
pub use output::volume_gain;

/// What the UI asks the engine to do.
#[derive(Debug, Clone)]
pub enum Command {
    /// Open a file (drops any enqueued next track). `gain_db` = volume normalization for it.
    Load { path: PathBuf, autoplay: bool, gain_db: f64 },
    /// Preload the track that follows the current one; playback continues into it with no gap,
    /// or with a `crossfade` of that many seconds (0 = gapless; ignored in bit-perfect mode).
    /// Replaces any previously enqueued track.
    Enqueue { path: PathBuf, gain_db: f64, crossfade: f64 },
    /// New normalization gain for a loaded or enqueued file (glides, no click).
    SetGain { path: PathBuf, gain_db: f64 },
    /// New EQ curve: applied in ~20 ms, heard after the ring (~200 ms).
    SetEq(EqSettings),
    /// Effects: applied like the EQ, except `speed`, which reopens the decks where they were
    /// (a short gap). Ignored in bit-perfect mode.
    SetFx(FxSettings),
    /// Forget the enqueued track (no-op once playback already continued into it).
    ClearNext,
    Play,
    Pause,
    Toggle,
    Stop,
    /// Position in seconds.
    Seek(f64),
    /// A-B repeat on the audible track (seconds, `a < b`): when playback reaches B it carries on
    /// from A with no gap, sample-accurate. None = off. Cleared by `Load`, `Stop` and track changes.
    SetLoop(Option<(f64, f64)>),
    /// Slider position 0.0..=1.0 (perceptual curve, see [`volume_gain`]).
    SetVolume(f32),
    /// Output device (cpal id from [`output_devices`]; None = follow the system default) and
    /// bit-perfect mode: exclusive at each file's own format, no DSP, volume fixed at 100 %.
    SetOutput { device: Option<String>, bit_perfect: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Event {
    StateChanged(PlayState),
    /// About 4 Hz while playing, and right after load/seek/pause. The UI interpolates.
    /// `speed`: source seconds per second (effects speed), for the UI's clock between events.
    Position { secs: f64, duration: f64, speed: f64 },
    /// The audible track changed: on load, and exactly when a gapless transition reaches the output.
    TrackChanged { path: PathBuf },
    /// The last track finished and nothing was enqueued.
    TrackEnded,
    /// Short code (`open_failed`, `unsupported_format`, `decode_failed`, `output_unavailable`,
    /// `output_unsupported`) for the UI to translate. Never paths or internals.
    Error(String),
    /// The output was (re)opened or lost: read [`Engine::output_info`].
    OutputChanged,
}

/// Output device as shown in the status line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInfo {
    pub device: String,
    /// cpal id, as in [`output_devices`].
    pub device_id: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// cpal names ("f32", "i16"...); exclusive mode gives the valid bits ("i24" in 32-bit slots).
    pub sample_format: String,
    /// WASAPI exclusive / CoreAudio hog mode: no system mixer, volume fixed at 100 %.
    pub exclusive: bool,
    /// Exclusive at the file's own rate and depth with nothing in between.
    pub bit_perfect: bool,
    /// What could not be honoured: `deviceMissing` (chosen device gone, default used),
    /// `exclusiveFailed` (busy or refused, shared mode instead), `exclusiveNotAllowed` (Windows
    /// has exclusive control switched off for the device), `formatChanged` (exclusive, but at
    /// the nearest format the device takes).
    pub fallback: Option<&'static str>,
}

/// A device for the output picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Output devices the system offers now.
pub fn output_devices() -> Vec<OutputDevice> {
    output::devices().into_iter().map(|(id, name, is_default)| OutputDevice { id, name, is_default }).collect()
}

/// Handle to the engine thread. Dropping it stops the engine.
pub struct Engine {
    tx: Sender<Command>,
    shared: Arc<output::Shared>,
    info: Arc<Mutex<Option<OutputInfo>>>,
    analyzer: Mutex<spectrum::Analyzer>,
}

impl Engine {
    pub fn new() -> (Self, Receiver<Event>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(output::Shared::new());
        let info = Arc::new(Mutex::new(None));
        producer::spawn(rx, ev_tx, shared.clone(), info.clone());
        let engine = Self { tx, shared, info, analyzer: Mutex::new(spectrum::Analyzer::new()) };
        (engine, ev_rx)
    }

    pub fn send(&self, cmd: Command) {
        match cmd {
            // Straight to the callback: volume responds instantly.
            Command::SetVolume(v) => self.shared.volume.store(volume_gain(v).to_bits(), Relaxed),
            cmd => {
                let _ = self.tx.send(cmd);
            }
        }
    }

    /// `bands` levels 0..=255, log-spaced 20 Hz - 20 kHz, from the last ~85 ms of output.
    pub fn spectrum(&self, bands: usize) -> Vec<u8> {
        let rate = self.shared.rate.load(Relaxed);
        let mut analyzer = self.analyzer.lock().unwrap_or_else(|e| e.into_inner());
        analyzer.bands(self.shared.viz_samples(), rate, bands)
    }

    /// Peak level per channel (L, R), linear 0..1, since the previous call (meters).
    pub fn peaks(&self) -> [f32; 2] {
        self.shared.peak.each_ref().map(|p| f32::from_bits(p.swap(0, Relaxed)))
    }

    /// `None` while no output device could be opened.
    pub fn output_info(&self) -> Option<OutputInfo> {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
