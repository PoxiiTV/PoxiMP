//! OS integration: media controls (SMTC on Windows, Now Playing on macOS; the keyboard media keys
//! arrive through them, so they are never registered as shortcuts too) and global shortcuts.

use std::path::PathBuf;
use std::sync::Mutex;

use poximp_engine::PlayState;
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::App;

const VOLUME_STEP: f32 = 0.05;
const SEEK_STEP: f64 = 10.0;

pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub cover: Option<PathBuf>,
}

pub enum Update {
    Track(Option<NowPlaying>),
    Playback { status: PlayState, secs: f64 },
}

#[cfg(any(windows, target_os = "macos"))]
mod os {
    use std::cell::RefCell;
    use std::time::Duration;

    use souvlaki::{MediaControlEvent as E, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
    use tauri::{AppHandle, Manager};

    use super::{NowPlaying, Update, SEEK_STEP};
    use crate::App;
    use poximp_engine::PlayState;

    thread_local! {
        // Main thread only: Now Playing must live there on macOS, and every call is routed there.
        static CONTROLS: RefCell<Option<MediaControls>> = const { RefCell::new(None) };
    }

    fn on_event(app: &AppHandle, e: E) {
        let s = app.state::<App>();
        let _ = match e {
            E::Play => s.play(),
            E::Pause => s.pause(),
            E::Toggle => s.toggle(),
            E::Next => s.next(),
            E::Previous => s.previous(),
            E::Stop => s.stop(),
            E::Seek(SeekDirection::Forward) => s.seek_by(SEEK_STEP),
            E::Seek(SeekDirection::Backward) => s.seek_by(-SEEK_STEP),
            E::SeekBy(SeekDirection::Forward, d) => s.seek_by(d.as_secs_f64()),
            E::SeekBy(SeekDirection::Backward, d) => s.seek_by(-d.as_secs_f64()),
            E::SetPosition(MediaPosition(d)) => s.seek(d.as_secs_f64()),
            E::SetVolume(v) if v.is_finite() => s.set_volume(v.clamp(0.0, 1.0) as f32),
            E::Raise => {
                crate::show_window(app);
                Ok(())
            }
            _ => Ok(()),
        };
    }

    pub fn init(app: &AppHandle) {
        #[cfg(windows)]
        let hwnd = match app.get_webview_window("main").and_then(|w| w.hwnd().ok()) {
            // As an integer: raw pointers cannot cross into the main-thread closure.
            Some(h) => Some(h.0 as usize),
            None => return,
        };
        #[cfg(not(windows))]
        let hwnd: Option<usize> = None;
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let config = PlatformConfig { display_name: "PoxiMP", dbus_name: "poximp", hwnd: hwnd.map(|h| h as *mut std::ffi::c_void) };
            // No media controls (old OS, policy): the app works the same without them.
            let Ok(mut c) = MediaControls::new(config) else { return };
            let events = handle.clone();
            if c.attach(move |e| on_event(&events, e)).is_ok() {
                let _ = c.set_playback(MediaPlayback::Stopped);
                CONTROLS.with(|slot| *slot.borrow_mut() = Some(c));
            }
        });
    }

    /// `file://` URL souvlaki loads: Windows takes the raw path after the prefix, macOS a real URL.
    fn file_url(p: &std::path::Path) -> String {
        let s = p.to_string_lossy();
        if cfg!(windows) {
            format!("file://{s}")
        } else {
            let enc: String = s
                .bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                    _ => format!("%{b:02X}"),
                })
                .collect();
            format!("file://{enc}")
        }
    }

    fn apply(c: &mut MediaControls, u: Update) {
        match u {
            Update::Track(Some(NowPlaying { title, artist, album, duration, cover })) => {
                let cover = cover.filter(|p| p.is_file()).map(|p| file_url(&p));
                let _ = c.set_metadata(MediaMetadata {
                    title: Some(&title),
                    artist: Some(&artist),
                    album: Some(&album),
                    cover_url: cover.as_deref(),
                    duration: (duration > 0.0).then(|| Duration::from_secs_f64(duration)),
                });
            }
            Update::Track(None) => {
                let _ = c.set_metadata(MediaMetadata::default());
                let _ = c.set_playback(MediaPlayback::Stopped);
            }
            Update::Playback { status, secs } => {
                let progress = Some(MediaPosition(Duration::from_secs_f64(secs.max(0.0))));
                let _ = c.set_playback(match status {
                    PlayState::Playing => MediaPlayback::Playing { progress },
                    PlayState::Paused => MediaPlayback::Paused { progress },
                    PlayState::Stopped => MediaPlayback::Stopped,
                });
            }
        }
    }

    pub fn update(app: &AppHandle, u: Update) {
        let _ = app.run_on_main_thread(move || {
            CONTROLS.with(|slot| {
                // Busy = a call is already in progress further up this same thread: skip, the next update fixes it.
                if let Ok(mut slot) = slot.try_borrow_mut() {
                    if let Some(c) = slot.as_mut() {
                        apply(c, u);
                    }
                }
            })
        });
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod os {
    pub fn init(_: &tauri::AppHandle) {}
    pub fn update(_: &tauri::AppHandle, _: super::Update) {}
}

pub use os::{init, update};

// --- global shortcuts ---

#[derive(Debug, Clone, Serialize)]
pub struct ShortcutInfo {
    /// Action: playPause, next, previous, volumeUp, volumeDown (i18n key `shortcuts.<id>`).
    pub id: &'static str,
    /// Key tokens for the UI to render: ctrl/cmd, alt, space, right, left, up, down.
    pub keys: Vec<&'static str>,
    /// false = another app already holds this combination.
    pub active: bool,
}

const SHORTCUTS: [(&str, Code, &str); 5] = [
    ("playPause", Code::Space, "space"),
    ("next", Code::ArrowRight, "right"),
    ("previous", Code::ArrowLeft, "left"),
    ("volumeUp", Code::ArrowUp, "up"),
    ("volumeDown", Code::ArrowDown, "down"),
];

fn run(app: &AppHandle, id: &str) {
    use tauri::Manager;
    let s = app.state::<App>();
    let volume = || s.player.lock().unwrap_or_else(|e| e.into_inner()).volume;
    let _ = match id {
        "playPause" => s.toggle(),
        "next" => s.next(),
        "previous" => s.previous(),
        "volumeUp" => s.set_volume((volume() + VOLUME_STEP).min(1.0)),
        "volumeDown" => s.set_volume((volume() - VOLUME_STEP).max(0.0)),
        _ => Ok(()),
    };
}

/// Ctrl+Alt (Cmd+Alt on macOS) + Space / arrows. A combination another app holds is skipped.
pub fn register_shortcuts(app: &AppHandle, out: &Mutex<Vec<ShortcutInfo>>) {
    let (mods, first) = if cfg!(target_os = "macos") { (Modifiers::SUPER | Modifiers::ALT, "cmd") } else { (Modifiers::CONTROL | Modifiers::ALT, "ctrl") };
    let list = SHORTCUTS
        .iter()
        .map(|&(id, code, key)| {
            let active = app
                .global_shortcut()
                .on_shortcut(Shortcut::new(Some(mods), code), move |app, _, ev| {
                    if ev.state == ShortcutState::Pressed {
                        run(app, id);
                    }
                })
                .is_ok();
            ShortcutInfo { id, keys: vec![first, "alt", key], active }
        })
        .collect();
    *out.lock().unwrap_or_else(|e| e.into_inner()) = list;
}
