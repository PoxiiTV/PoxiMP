//! Background threads, each with its own SQLite connection (WAL: UI queries never wait on them):
//! library scans/imports, and the analysis worker: quality detector (pass 1), then the sound
//! features for radio / mixes (pass 2, once every track has its verdict), then seekbar waveforms.
//! The playing and the next track get their waveform first, from a thread of their own.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use poximp_library::{Library, QualityJob, ScanProgress};
use tauri::{AppHandle, Emitter, Manager};

use crate::model::{self, QualityUpdate, WaveReady};
use crate::App;

/// Let playback settle before the detector reads the disk.
const QUALITY_DELAY: Duration = Duration::from_secs(2);
const QUALITY_LOOKAHEAD: usize = 20;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

pub enum ScanJob {
    /// Incremental rescan of every music folder.
    Rescan,
    /// Read these opened files (external tracks, outside the library), then play them.
    Import(Vec<PathBuf>),
}

/// Background priority (CPU and disk) for the current thread. Plain OS calls, no crate needed.
fn lower_priority() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> *mut std::ffi::c_void;
            fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
        }
        const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;
        // SAFETY: pseudo-handle of the calling thread, always valid.
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
        }
        const PRIO_DARWIN_THREAD: i32 = 3;
        const PRIO_DARWIN_BG: i32 = 0x1000;
        // SAFETY: plain libc call on the calling thread.
        unsafe { setpriority(PRIO_DARWIN_THREAD, 0, PRIO_DARWIN_BG) };
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            lower_priority();
            f()
        })
        .expect("spawn worker thread");
}

pub fn spawn_scanner(app: AppHandle, mut lib: Library, rx: Receiver<ScanJob>) {
    spawn("poximp-scan", move || {
        let never = AtomicBool::new(false);
        for job in rx {
            let state = app.state::<App>();
            let mut last = Instant::now();
            let progress = |p: ScanProgress| {
                if p.done >= p.total || last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    let _ = app.emit("library:progress", p);
                }
            };
            match job {
                ScanJob::Rescan => {
                    let roots = lib.roots().unwrap_or_default();
                    let _ = lib.scan(&roots, &state.covers, &never, progress);
                }
                ScanJob::Import(files) => {
                    let _ = lib.scan_external(&files, &state.covers);
                    let ids: Vec<i64> = files
                        .iter()
                        .filter_map(|f| lib.id_for_path(&f.to_string_lossy()).ok().flatten())
                        .collect();
                    if !ids.is_empty() {
                        let _ = state.play_ids(ids, 0);
                    }
                }
            }
            let _ = state.wake_quality.send(());
        }
    });
}

enum Job {
    Quality(QualityJob),
    Features(QualityJob),
    Waveform(QualityJob),
}

/// What to analyse next: quality of the playing track (once it has played for a moment), then
/// the queue, then the rest of the library; then features. With the detector switched off only
/// the features (radio, mixes) are worked on. `Err(wait)` = nothing to do for now.
fn pick(app: &App, lib: &Library) -> Result<Job, Duration> {
    if !app.sound().cfg.quality.analyze {
        return features(lib);
    }
    let (current, upcoming) = {
        let p = app.player.lock().unwrap_or_else(|e| e.into_inner());
        (p.now.as_ref().map(|n| (n.id, n.started.elapsed())), p.q.upcoming(QUALITY_LOOKAHEAD))
    };
    if let Some((id, since)) = current {
        if since < QUALITY_DELAY {
            return Err(QUALITY_DELAY - since);
        }
        if let Ok(Some(job)) = lib.quality_job(id) {
            return Ok(Job::Quality(job));
        }
    }
    for id in upcoming {
        if let Ok(Some(job)) = lib.quality_job(id) {
            return Ok(Job::Quality(job));
        }
    }
    let first = |jobs: poximp_library::Result<Vec<QualityJob>>| jobs.map(|mut j| (!j.is_empty()).then(|| j.remove(0)));
    match first(lib.tracks_needing_quality(1)) {
        Ok(Some(job)) => return Ok(Job::Quality(job)),
        Ok(None) => {}
        // Busy database (a scan batch committing): try again shortly.
        Err(_) => return Err(Duration::from_secs(1)),
    }
    features(lib)
}

/// Features first (radio, mixes), then the waveforms nobody has played yet.
fn features(lib: &Library) -> Result<Job, Duration> {
    let first = |jobs: poximp_library::Result<Vec<QualityJob>>| jobs.map(|mut j| (!j.is_empty()).then(|| j.remove(0)));
    match first(lib.tracks_needing_features(1)) {
        Ok(Some(job)) => return Ok(Job::Features(job)),
        Ok(None) => {}
        Err(_) => return Err(Duration::from_secs(1)),
    }
    match first(lib.tracks_needing_waveform(1)) {
        Ok(Some(job)) => Ok(Job::Waveform(job)),
        Ok(None) => Err(Duration::from_secs(60)),
        Err(_) => Err(Duration::from_secs(1)),
    }
}

/// Decodes, stores and announces one waveform. Like quality, a decoder panic or an unreadable
/// file is stored (empty) and not retried until the file changes.
fn store_waveform(app: &AppHandle, lib: &Library, job: &QualityJob) -> poximp_library::Result<()> {
    let peaks = std::panic::catch_unwind(|| poximp_quality::waveform::waveform(job.path.as_ref()))
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    lib.set_waveform(job.id, &job.key, &peaks)?;
    let _ = app.emit("waveform:ready", WaveReady { track_id: job.id });
    Ok(())
}

/// Waveform of the playing track, then of the next one, as soon as they change. Normal priority
/// on purpose: it decodes a whole file (~0.3 s for a 4-min MP3) while playback has its own
/// decoder and buffer; the UI never waits for it (it draws a flat line until "waveform:ready").
pub fn spawn_waveform(app: AppHandle, lib: Library, wake: Receiver<()>) {
    std::thread::Builder::new()
        .name("poximp-wave".into())
        .spawn(move || loop {
            let state = app.state::<App>();
            let ids: Vec<i64> = {
                let p = state.player.lock().unwrap_or_else(|e| e.into_inner());
                p.now.as_ref().map(|n| n.id).into_iter().chain(p.q.upcoming(1)).collect()
            };
            match ids.into_iter().find_map(|id| lib.waveform_job(id).ok().flatten()) {
                Some(job) => {
                    if store_waveform(&app, &lib, &job).is_err() {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
                None => {
                    if wake.recv().is_err() {
                        return;
                    }
                }
            }
        })
        .expect("spawn waveform thread");
}

pub fn spawn_quality(app: AppHandle, lib: Library, wake: Receiver<()>) {
    spawn("poximp-quality", move || {
        let opts = poximp_quality::Options::default();
        loop {
            let state = app.state::<App>();
            let job = match pick(&state, &lib) {
                Ok(Job::Quality(job)) => job,
                Ok(Job::Features(job)) => {
                    // Same rule as quality: a decoder panic means "unreadable", stored, not retried.
                    let vec = std::panic::catch_unwind(|| poximp_quality::features::features(job.path.as_ref())).ok().flatten();
                    if lib.set_features(job.id, &job.key, vec.as_deref()).is_err() {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    continue;
                }
                Ok(Job::Waveform(job)) => {
                    if store_waveform(&app, &lib, &job).is_err() {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    continue;
                }
                Err(wait) => match wake.recv_timeout(wait) {
                    Err(RecvTimeoutError::Disconnected) => return,
                    _ => continue,
                },
            };
            state.analyzing.store(job.id, Relaxed);
            let _ = app.emit("quality:update", QualityUpdate { track_id: job.id, verdict: "analyzing".into(), report: None });
            // Unreadable / unsupported files are stored as inconclusive so they are not retried.
            // A decoder panic on a malformed file counts as unreadable instead of killing the worker.
            let report = std::panic::catch_unwind(|| poximp_quality::analyze(job.path.as_ref(), &opts))
                .ok()
                .and_then(Result::ok)
                .map(|r| model::report(&r));
            let verdict = report.as_ref().map_or("inconclusive", |r| r.verdict.as_str()).to_string();
            let json = serde_json::to_string(&report).unwrap_or_else(|_| "null".into());
            let stored = lib.set_quality(job.id, &verdict, &json, &job.key);
            state.analyzing.store(0, Relaxed);
            let _ = app.emit("quality:update", QualityUpdate { track_id: job.id, verdict, report });
            if stored.is_err() {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
}
