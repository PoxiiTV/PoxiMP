//! PoxiMP app: Tauri commands, events, plugins and windows. Contract: src/core/types.ts.

mod autoeq;
mod lights;
mod lyrics;
mod media;
mod model;
mod open;
mod player;
mod remote;
mod sound;
mod work;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering::Relaxed};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use poximp_engine::{Command, Engine, EqSettings, FxSettings};
use poximp_library::radio::{Mix, Rng, RADIO_LEN};
use poximp_library::{ImportReport, Library, PlaylistKind, SearchGroup, TrackQuery};
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, DragDropEvent, Emitter, Listener, Manager, State, WindowEvent};

use model::{Error, Repeat, Result, SortKey};
use player::Player;
use sound::{Norm, Sound};
use work::ScanJob;

const START_VOLUME: f32 = 0.8;
const MAX_TEXT: usize = 200;
const MAX_PATH: usize = 4096;
const MAX_RULES_JSON: usize = 64 * 1024;
/// Hits per group in the global search.
const SEARCH_PER_GROUP: u32 = 6;

pub struct App {
    app: AppHandle,
    engine: Engine,
    /// Connection for UI queries; the workers open their own.
    lib: Mutex<Library>,
    player: Mutex<Player>,
    covers: PathBuf,
    scan: Mutex<Sender<ScanJob>>,
    wake_quality: Sender<()>,
    /// Waveform of the playing and the next track, as soon as they change.
    wake_wave: Sender<()>,
    /// Track the quality worker is analysing (0 = none).
    analyzing: AtomicI64,
    /// EQ, presets, normalization (sound.json). A leaf lock: never held while taking another.
    sound: Mutex<Sound>,
    shortcuts: Mutex<Vec<media::ShortcutInfo>>,
    data_dir: PathBuf,
    /// Phone remote server while it is on.
    remote: Mutex<Option<remote::Server>>,
    hub: Arc<remote::Hub>,
    /// Files being opened from Explorer / Finder / a drop, merged until launches stop arriving.
    opening: Mutex<open::Batch>,
    /// Refused files the UI has not shown yet ("open:rejected" pokes it; it may not be up yet).
    rejected: Mutex<Vec<open::Rejected>>,
}

impl App {
    fn analyzing(&self) -> i64 {
        self.analyzing.load(Relaxed)
    }

    fn queue_scan(&self, job: ScanJob) {
        let _ = self.scan.lock().unwrap_or_else(|e| e.into_inner()).send(job);
    }

    fn library(&self) -> std::sync::MutexGuard<'_, Library> {
        self.lib.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn sound(&self) -> std::sync::MutexGuard<'_, Sound> {
        self.sound.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Own connection for imports: long writes never hold the UI's library lock (WAL).
    fn import_lib(&self) -> Result<Library> {
        Ok(Library::open(self.data_dir.join("library.db"))?)
    }

    fn tracks(&self, ids: &[i64]) -> Result<Vec<model::Track>> {
        let a = self.analyzing();
        Ok(self.library().tracks_by_ids(ids)?.iter().map(|t| model::track(t, a)).collect())
    }

    /// New curve: to the engine, then to disk.
    fn apply_eq(&self, mut eq: EqSettings, preset: Option<String>) -> model::EqState {
        eq.sanitize();
        self.engine.send(Command::SetEq(eq.clone()));
        let mut s = self.sound();
        s.cfg.eq = eq;
        s.cfg.preset = preset;
        s.save();
        model::eq_state(&s)
    }

    /// Cover image of a track (bytes, MIME type), only ever from the covers folder.
    fn cover(&self, track_id: i64) -> Option<(Vec<u8>, &'static str)> {
        let name = self.library().track_by_id(track_id).ok().flatten()?.cover?;
        // Names come from our own scanner; still never let one leave the covers folder.
        if name.contains(['/', '\\']) || name.contains("..") {
            return None;
        }
        let bytes = std::fs::read(self.covers.join(&name)).ok()?;
        Some((bytes, if name.ends_with(".png") { "image/png" } else { "image/jpeg" }))
    }

    /// Dropped paths: folders become music folders (rescanned), files open like a double-click.
    fn open_paths(&self, paths: Vec<PathBuf>) {
        let (dirs, files): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| p.is_dir());
        if !dirs.is_empty() {
            let lib = self.library();
            for d in &dirs {
                let _ = lib.add_root(&d.to_string_lossy());
            }
            drop(lib);
            self.queue_scan(ScanJob::Rescan);
        }
        // Dropped paths are absolute: no "-flag" can be among them.
        let (ok, bad) = open::parse_args(files.into_iter().map(PathBuf::into_os_string), Path::new(""));
        self.open_files(ok, bad);
    }

    /// Checked files from a launch, Finder or a drop: the refused ones go to a toast, the rest
    /// join the batch, which plays once no launch came for `open::DEBOUNCE`.
    fn open_files(&self, ok: Vec<PathBuf>, bad: Vec<open::Rejected>) {
        self.reject(bad);
        if !self.opening.lock().unwrap_or_else(|e| e.into_inner()).push(ok, Instant::now()) {
            return;
        }
        let app = self.app.clone();
        std::thread::spawn(move || {
            let state = app.state::<App>();
            let batch = loop {
                let due = state.opening.lock().unwrap_or_else(|e| e.into_inner()).take(Instant::now());
                match due {
                    Ok(batch) => break batch,
                    Err(wait) => std::thread::sleep(wait),
                }
            };
            state.play_files(batch);
        });
    }

    fn reject(&self, bad: Vec<open::Rejected>) {
        if bad.is_empty() {
            return;
        }
        let mut r = self.rejected.lock().unwrap_or_else(|e| e.into_inner());
        // Only for a toast: a handful is plenty if the UI never asks.
        let room = 20usize.saturating_sub(r.len());
        r.extend(bad.into_iter().take(room));
        drop(r);
        let _ = self.app.emit("open:rejected", ());
    }

    /// Replaces the queue with these files (M3U lists expanded in place) and plays the first, like
    /// AIMP. Files not known yet are read as external tracks: they play but stay out of the library.
    fn play_files(&self, batch: Vec<PathBuf>) {
        let mut files = Vec::new();
        let mut bad = Vec::new();
        for p in batch {
            if !open::is_playlist(&p) {
                files.push(p);
                continue;
            }
            let listed: Vec<PathBuf> = poximp_library::m3u_files(&p)
                .unwrap_or_default()
                .into_iter()
                .filter(|f| poximp_library::supported(f) && f.is_file())
                .collect();
            if listed.is_empty() {
                bad.push(open::Rejected::new(&p, open::Reason::Empty));
            }
            files.extend(listed);
        }
        self.reject(bad);
        if files.is_empty() {
            return;
        }
        let Ok(mut lib) = self.import_lib() else { return };
        let id_of = |lib: &Library, f: &Path| lib.id_for_path(&f.to_string_lossy()).ok().flatten();
        if files.iter().any(|f| id_of(&lib, f).is_none()) {
            // Own connection, not the scanner's queue: a startup rescan would delay playback.
            match lib.scan_external(&files, &self.covers) {
                Ok(rep) if rep.added + rep.updated > 0 => {
                    let _ = self.app.emit("library:progress", poximp_library::ScanProgress { done: 1, total: 1 });
                    let _ = self.wake_quality.send(());
                }
                Ok(_) => {}
                // Busy (a long first scan holds the write lock): the scanner plays them when it can.
                Err(_) => return self.queue_scan(ScanJob::Import(files)),
            }
        }
        let (mut ids, mut unread) = (Vec::new(), Vec::new());
        for f in &files {
            match id_of(&lib, f) {
                Some(id) => ids.push(id),
                None => unread.push(open::Rejected::new(f, open::Reason::Unsupported)),
            }
        }
        drop(lib);
        self.reject(unread);
        if !ids.is_empty() {
            let _ = self.play_window(ids, 0);
        }
    }
}

fn trimmed(s: &str) -> &str {
    let s = s.trim();
    s.char_indices().nth(MAX_TEXT).map_or(s, |(i, _)| &s[..i])
}

/// Import source (an existing file) or export target (in an existing folder) with one of `exts`.
fn file_arg(path: &str, exts: &[&str], existing: bool) -> Result<PathBuf> {
    let p = Path::new(path);
    let ext_ok = p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)));
    if path.len() > MAX_PATH || !p.is_absolute() || !ext_ok {
        return Err(Error::Invalid);
    }
    let there = if existing { p.is_file() } else { p.parent().is_some_and(Path::is_dir) && !p.is_dir() };
    if !there {
        return Err(Error::NotFound);
    }
    Ok(p.to_path_buf())
}

/// Smart playlist rules from the UI, validated (fields, operators, values, depth, limit).
fn rules_arg(rules: &serde_json::Value) -> Result<String> {
    let json = serde_json::to_string(rules).map_err(|_| Error::Invalid)?;
    if json.len() > MAX_RULES_JSON {
        return Err(Error::Invalid);
    }
    poximp_library::smart::parse(&json)?;
    Ok(json)
}

/// Optional scope of a listing: a folder (and its subfolders), an album or an artist.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Scope {
    folder: Option<String>,
    recursive: Option<bool>,
    album_key: Option<String>,
    artist_id: Option<i64>,
}

/// The library listing the UI shows (also what play_view plays).
fn view_query(sort: SortKey, filter: &str, scope: Scope) -> Result<TrackQuery> {
    let bad = |s: &Option<String>| s.as_ref().is_some_and(|f| f.is_empty() || f.len() > MAX_PATH);
    if bad(&scope.folder) || bad(&scope.album_key) {
        return Err(Error::Invalid);
    }
    let (sort, desc) = model::lib_sort(sort);
    Ok(TrackQuery {
        sort,
        desc,
        text: trimmed(filter).to_string(),
        folder: scope.folder,
        folder_recursive: scope.recursive.unwrap_or(false),
        album_key: scope.album_key,
        artist_id: scope.artist_id,
        ..Default::default()
    })
}

/// New randomness for every radio.
fn fresh_rng() -> Rng {
    Rng::new(SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64))
}

// --- library ---

#[tauri::command]
async fn library_add_folder(app: State<'_, App>, path: String) -> Result<()> {
    let p = Path::new(&path);
    if path.len() > 4096 || !p.is_absolute() || !p.is_dir() {
        return Err(Error::Invalid);
    }
    app.library().add_root(&path)?;
    Ok(())
}

/// Takes a music folder (or one inside it) out of the library. The files are not touched.
#[tauri::command]
async fn library_remove_folder(app: State<'_, App>, path: String) -> Result<()> {
    if path.is_empty() || path.len() > 4096 || !Path::new(&path).is_absolute() {
        return Err(Error::Invalid);
    }
    app.library().remove_folder(&path)?;
    Ok(())
}

#[tauri::command]
async fn library_scan(app: State<'_, App>) -> Result<()> {
    app.queue_scan(ScanJob::Rescan);
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn library_page(
    app: State<'_, App>,
    offset: u32,
    limit: u32,
    sort: SortKey,
    filter: String,
    folder: Option<String>,
    recursive: Option<bool>,
    album_key: Option<String>,
    artist_id: Option<i64>,
) -> Result<model::LibraryPage> {
    let scope = Scope { folder, recursive, album_key, artist_id };
    let q = TrackQuery { offset, limit: limit.clamp(1, 1000), ..view_query(sort, &filter, scope)? };
    let page = app.library().list_tracks(&q)?;
    Ok(model::page(&page, app.analyzing()))
}

/// Plays a whole view server-side (huge libraries: the UI only has a page of it).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn play_view(
    app: State<'_, App>,
    sort: SortKey,
    filter: String,
    folder: Option<String>,
    recursive: Option<bool>,
    album_key: Option<String>,
    artist_id: Option<i64>,
    start_track_id: Option<i64>,
) -> Result<()> {
    let scope = Scope { folder, recursive, album_key, artist_id };
    let ids = app.library().view_ids(&view_query(sort, &filter, scope)?)?;
    if ids.is_empty() {
        return Err(Error::NotFound);
    }
    app.play_view(ids, start_track_id)
}

#[tauri::command]
async fn library_folder_tree(app: State<'_, App>) -> Result<Vec<poximp_library::FolderNode>> {
    Ok(app.library().folders()?)
}

#[tauri::command]
async fn library_search(app: State<'_, App>, q: String) -> Result<Vec<model::Track>> {
    let hits = app.library().search(trimmed(&q), 50)?;
    Ok(hits.iter().map(|t| model::track(t, app.analyzing())).collect())
}

/// Global search bar: tracks, albums, artists and playlists, grouped (only groups with hits).
#[tauri::command]
async fn search_all(app: State<'_, App>, q: String) -> Result<Vec<SearchGroup<model::Track>>> {
    if q.len() > MAX_TEXT * 4 {
        return Err(Error::Invalid);
    }
    let a = app.analyzing();
    let groups = app.library().search_all(trimmed(&q), SEARCH_PER_GROUP)?;
    Ok(groups.into_iter().map(|g| g.map(&|t| model::track(&t, a))).collect())
}

#[tauri::command]
async fn library_folders(app: State<'_, App>) -> Result<Vec<String>> {
    Ok(app.library().roots()?)
}

/// Every (split) artist with its track count, by name: the Biblioteca artist column.
#[tauri::command]
async fn library_artists(app: State<'_, App>) -> Result<Vec<poximp_library::Artist>> {
    Ok(app.library().artists()?)
}

/// Seekbar waveform: peaks then RMS (u8 each, see poximp_quality::waveform). Raw bytes, no JSON;
/// empty = not computed yet ("waveform:ready" follows) or unreadable file.
#[tauri::command]
async fn waveform_get(app: State<'_, App>, track_id: i64) -> Result<tauri::ipc::Response> {
    if track_id <= 0 {
        return Err(Error::Invalid);
    }
    Ok(tauri::ipc::Response::new(app.library().waveform(track_id)?.unwrap_or_default()))
}

#[tauri::command]
async fn quality_report(app: State<'_, App>, track_id: i64) -> Result<Option<model::QualityReport>> {
    let t = app.library().track_by_id(track_id)?.ok_or(Error::NotFound)?;
    Ok(t.quality_json.as_deref().and_then(|j| serde_json::from_str(j).ok()))
}

// --- player ---

#[tauri::command]
async fn play_track(app: State<'_, App>, track_id: i64) -> Result<()> {
    app.play_ids(vec![track_id], 0)
}

#[tauri::command]
async fn play_queue(app: State<'_, App>, track_ids: Vec<i64>, start_index: usize) -> Result<()> {
    app.play_ids(track_ids, start_index)
}

// --- queue ---

#[tauri::command]
async fn queue_get(app: State<'_, App>, offset: Option<usize>, limit: Option<usize>) -> Result<model::QueuePage> {
    app.queue_page(offset.unwrap_or(0), limit.unwrap_or(500).clamp(1, 1000))
}

#[tauri::command]
async fn queue_play_next(app: State<'_, App>, track_ids: Vec<i64>) -> Result<()> {
    app.queue_play_next(track_ids)
}

#[tauri::command]
async fn queue_add(app: State<'_, App>, track_ids: Vec<i64>) -> Result<()> {
    app.queue_add(track_ids)
}

#[tauri::command]
async fn queue_remove(app: State<'_, App>, positions: Vec<usize>) -> Result<()> {
    app.queue_remove(positions)
}

#[tauri::command]
async fn queue_move(app: State<'_, App>, from: usize, to: usize) -> Result<()> {
    app.queue_move(from, to)
}

#[tauri::command]
async fn queue_jump(app: State<'_, App>, position: usize) -> Result<()> {
    app.queue_jump(position)
}

#[tauri::command]
async fn queue_clear(app: State<'_, App>) -> Result<()> {
    app.queue_clear()
}

#[tauri::command]
async fn resume_get(app: State<'_, App>, track_id: i64) -> Result<Option<f64>> {
    app.resume_get(track_id)
}

#[tauri::command]
async fn resume_clear(app: State<'_, App>, track_id: i64) -> Result<()> {
    app.resume_clear(track_id)
}

// --- playlists ---

#[tauri::command]
async fn playlist_list(app: State<'_, App>) -> Result<Vec<model::Playlist>> {
    Ok(app.library().playlists()?.into_iter().map(model::playlist).collect())
}

/// `rules` = null for a manual playlist, a rule tree for a smart one.
#[tauri::command]
async fn playlist_create(app: State<'_, App>, name: String, rules: Option<serde_json::Value>) -> Result<model::Playlist> {
    let rules = rules.as_ref().map(rules_arg).transpose()?;
    let kind = if rules.is_some() { PlaylistKind::Smart } else { PlaylistKind::Manual };
    let lib = app.library();
    let id = lib.create_playlist(&name, kind, rules.as_deref())?;
    Ok(model::playlist(lib.playlist(id)?))
}

#[tauri::command]
async fn playlist_rename(app: State<'_, App>, id: i64, name: String) -> Result<()> {
    Ok(app.library().rename_playlist(id, &name)?)
}

#[tauri::command]
async fn playlist_set_rules(app: State<'_, App>, id: i64, rules: serde_json::Value) -> Result<()> {
    let rules = poximp_library::smart::parse(&rules_arg(&rules)?)?;
    Ok(app.library().set_smart_rules(id, &rules)?)
}

#[tauri::command]
async fn playlist_delete(app: State<'_, App>, id: i64) -> Result<()> {
    Ok(app.library().delete_playlist(id)?)
}

#[tauri::command]
async fn playlist_page(app: State<'_, App>, id: i64, offset: usize, limit: usize) -> Result<model::PlaylistPage> {
    let (ids, duration_ms) = {
        let lib = app.library();
        let ids = lib.playlist_ids(id)?;
        let d = lib.duration_of(&ids)?;
        (ids, d)
    };
    let page: Vec<i64> = ids.iter().skip(offset).take(limit.clamp(1, 1000)).copied().collect();
    Ok(model::PlaylistPage { total: ids.len() as u32, duration: duration_ms as f64 / 1000.0, tracks: app.tracks(&page)? })
}

/// Live result of rules being edited: total and the first `limit` (max 200) tracks.
#[tauri::command]
async fn smart_preview(app: State<'_, App>, rules: serde_json::Value, limit: Option<usize>) -> Result<model::PlaylistPage> {
    let rules = poximp_library::smart::parse(&rules_arg(&rules)?)?;
    let (ids, duration_ms) = {
        let lib = app.library();
        let ids = lib.smart_ids(&rules)?;
        let d = lib.duration_of(&ids)?;
        (ids, d)
    };
    let first: Vec<i64> = ids.iter().take(limit.unwrap_or(50).clamp(1, 200)).copied().collect();
    Ok(model::PlaylistPage { total: ids.len() as u32, duration: duration_ms as f64 / 1000.0, tracks: app.tracks(&first)? })
}

fn known_ids(lib: &Library, ids: &[i64]) -> Result<()> {
    if ids.is_empty() || ids.len() > player::MAX_EDIT {
        return Err(Error::Invalid);
    }
    let mut unique = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    if lib.count_existing(&unique)? as usize != unique.len() {
        return Err(Error::NotFound);
    }
    Ok(())
}

#[tauri::command]
async fn playlist_add(app: State<'_, App>, id: i64, track_ids: Vec<i64>) -> Result<()> {
    let mut lib = app.library();
    known_ids(&lib, &track_ids)?;
    Ok(lib.add_to_playlist(id, &track_ids)?)
}

#[tauri::command]
async fn playlist_remove(app: State<'_, App>, id: i64, positions: Vec<usize>) -> Result<()> {
    if positions.is_empty() || positions.len() > player::MAX_QUEUE {
        return Err(Error::Invalid);
    }
    Ok(app.library().remove_from_playlist(id, &positions)?)
}

#[tauri::command]
async fn playlist_move(app: State<'_, App>, id: i64, from: usize, to: usize) -> Result<()> {
    Ok(app.library().move_in_playlist(id, from, to)?)
}

#[tauri::command]
async fn playlist_play(app: State<'_, App>, id: i64, start_index: Option<usize>) -> Result<()> {
    let ids = app.library().playlist_ids(id)?;
    let start = start_index.unwrap_or(0);
    if ids.is_empty() || start >= ids.len() {
        return Err(Error::Invalid);
    }
    app.play_window(ids, start)
}

// --- lyrics ---

#[tauri::command]
async fn lyrics_get(app: State<'_, App>, track_id: i64) -> Result<poximp_library::lyrics::Lyrics> {
    lyrics::get(&app, track_id).await
}

// --- effects ---

#[tauri::command]
async fn fx_get(app: State<'_, App>) -> Result<FxSettings> {
    Ok(app.sound().cfg.fx)
}

/// New effects: to the engine (speed reopens the decks there), then to disk.
#[tauri::command]
async fn fx_set(app: State<'_, App>, mut fx: FxSettings) -> Result<FxSettings> {
    fx.sanitize();
    app.engine.send(Command::SetFx(fx));
    let mut s = app.sound();
    s.cfg.fx = fx;
    s.save();
    Ok(fx)
}

#[tauri::command]
async fn lyrics_online_get(app: State<'_, App>) -> Result<bool> {
    Ok(app.sound().cfg.online_lyrics)
}

#[tauri::command]
async fn lyrics_online_set(app: State<'_, App>, on: bool) -> Result<bool> {
    let mut s = app.sound();
    s.cfg.online_lyrics = on;
    s.save();
    Ok(on)
}

// --- radio and mixes ---

/// Replaces the queue with the seed and ~50 tracks like it, and plays.
#[tauri::command]
async fn radio_start(app: State<'_, App>, seed_track_id: i64) -> Result<()> {
    let ids = app.library().radio(seed_track_id, RADIO_LEN, &mut fresh_rng())?;
    app.play_ids(ids, 0)
}

#[tauri::command]
async fn mixes_list(app: State<'_, App>) -> Result<Vec<Mix>> {
    // Same mixes all day: they do not reshuffle every time the view opens.
    let day = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
    Ok(app.library().mixes(&mut Rng::new(day))?)
}

// --- stats ---

#[tauri::command]
async fn stats(app: State<'_, App>, range: poximp_library::Range) -> Result<poximp_library::Stats<model::Track>> {
    let a = app.analyzing();
    Ok(app.library().stats(range)?.map(&|t| model::track(&t, a)))
}

#[tauri::command]
async fn wrapped(app: State<'_, App>, year: i32) -> Result<poximp_library::Wrapped<model::Track>> {
    let a = app.analyzing();
    Ok(app.library().wrapped(year)?.map(&|t| model::track(&t, a)))
}

// --- migration: imports and exports ---

#[tauri::command]
async fn import_aimp_playlist(app: State<'_, App>, path: String) -> Result<ImportReport> {
    let p = file_arg(&path, &["aimppl4", "aimppl"], true)?;
    Ok(app.import_lib()?.import_aimp_playlist(&p)?)
}

#[tauri::command]
async fn import_itunes_xml(app: State<'_, App>, path: String) -> Result<ImportReport> {
    let p = file_arg(&path, &["xml"], true)?;
    Ok(app.import_lib()?.import_itunes_xml(&p)?)
}

#[tauri::command]
async fn import_m3u(app: State<'_, App>, path: String) -> Result<ImportReport> {
    let p = file_arg(&path, &["m3u", "m3u8"], true)?;
    Ok(app.import_lib()?.import_m3u(&p)?)
}

/// Returns how many tracks were written.
#[tauri::command]
async fn export_m3u(app: State<'_, App>, playlist_id: i64, path: String) -> Result<u32> {
    let p = file_arg(&path, &["m3u8", "m3u"], false)?;
    Ok(app.library().export_m3u(playlist_id, &p)?)
}

/// Returns how many tracks were written.
#[tauri::command]
async fn export_stats_backup(app: State<'_, App>, path: String) -> Result<u32> {
    let p = file_arg(&path, &["json"], false)?;
    Ok(app.import_lib()?.export_stats_backup(&p)?)
}

#[tauri::command]
async fn import_stats_backup(app: State<'_, App>, path: String) -> Result<ImportReport> {
    let p = file_arg(&path, &["json"], true)?;
    Ok(app.import_lib()?.import_stats_backup(&p)?)
}

#[tauri::command]
async fn toggle(app: State<'_, App>) -> Result<()> {
    app.toggle()
}

#[tauri::command]
async fn next(app: State<'_, App>) -> Result<()> {
    app.next()
}

#[tauri::command]
async fn previous(app: State<'_, App>) -> Result<()> {
    app.previous()
}

#[tauri::command]
async fn stop(app: State<'_, App>) -> Result<()> {
    app.stop()
}

/// A-B repeat of the current track: `a` alone marks A, both loop, neither = off (seconds).
#[tauri::command]
async fn ab_set(app: State<'_, App>, a: Option<f64>, b: Option<f64>) -> Result<()> {
    app.ab_set(a, b)
}

/// Sleep timer: pause in `minutes` (1..=1440), or stop when the current track ends; neither = off.
#[tauri::command]
async fn sleep_set(app: State<'_, App>, minutes: Option<u32>, end_of_track: bool) -> Result<()> {
    app.sleep_set(minutes, end_of_track)
}

#[tauri::command]
async fn seek(app: State<'_, App>, secs: f64) -> Result<()> {
    app.seek(secs)
}

#[tauri::command]
async fn set_volume(app: State<'_, App>, v: f32) -> Result<()> {
    app.set_volume(v)
}

#[tauri::command]
async fn set_shuffle(app: State<'_, App>, b: bool) -> Result<()> {
    app.set_shuffle(b);
    Ok(())
}

#[tauri::command]
async fn set_repeat(app: State<'_, App>, mode: Repeat) -> Result<()> {
    app.set_repeat(mode);
    Ok(())
}

/// `bands` spectrum bytes, then L and R peak (0..255 = -60..0 dBFS). Raw bytes, no JSON.
#[tauri::command]
async fn viz_frame(app: State<'_, App>, bands: usize) -> Result<tauri::ipc::Response> {
    if !(1..=512).contains(&bands) {
        return Err(Error::Invalid);
    }
    let mut out = app.engine.spectrum(bands);
    out.extend(app.engine.peaks().map(|p| {
        let db = 20.0 * p.max(1e-9).log10();
        ((db + 60.0) / 60.0).clamp(0.0, 1.0).mul_add(255.0, 0.5) as u8
    }));
    Ok(tauri::ipc::Response::new(out))
}

#[tauri::command]
async fn output_info(app: State<'_, App>) -> Result<model::OutputInfo> {
    // The device opens on the engine thread right after start: give it a moment on first call.
    let t0 = Instant::now();
    let mut info = app.engine.output_info();
    while info.is_none() && t0.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(50));
        info = app.engine.output_info();
    }
    Ok(model::output_info(info))
}

/// Devices the system offers now, plus the chosen output settings.
#[tauri::command]
async fn output_devices(app: State<'_, App>) -> Result<model::OutputDevices> {
    let devices = poximp_engine::output_devices();
    Ok(model::OutputDevices { devices, settings: app.sound().cfg.output.clone() })
}

/// `deviceId` must be one of `output_devices` (or null = follow the system default).
#[tauri::command]
async fn output_set(app: State<'_, App>, settings: sound::Output) -> Result<sound::Output> {
    let mut o = settings;
    if !o.crossfade.is_finite() || !(0.0..=sound::MAX_CROSSFADE).contains(&o.crossfade) {
        return Err(Error::Invalid);
    }
    if let Some(id) = &o.device_id {
        // The saved one may be unplugged right now; anything else must exist.
        let saved = app.sound().cfg.output.device_id.as_ref() == Some(id);
        let known = id.len() <= sound::MAX_DEVICE_ID && (saved || poximp_engine::output_devices().iter().any(|d| &d.id == id));
        if !known {
            return Err(Error::Invalid);
        }
    }
    o.sanitize();
    app.set_output(o.clone());
    let mut s = app.sound();
    s.cfg.output = o.clone();
    s.save();
    Ok(o)
}

#[tauri::command]
async fn player_sync(app: State<'_, App>) -> Result<model::PlayerSync> {
    Ok(app.sync())
}

// --- sound: EQ, presets, AutoEq, normalization ---

#[tauri::command]
async fn eq_get(app: State<'_, App>) -> Result<model::EqState> {
    Ok(model::eq_state(&app.sound()))
}

#[tauri::command]
async fn eq_presets(app: State<'_, App>) -> Result<Vec<sound::Preset>> {
    Ok(app.sound().presets())
}

/// `preset` = id of the preset the curve still matches (None once edited by hand).
#[tauri::command]
async fn eq_set(app: State<'_, App>, eq: EqSettings, preset: Option<String>) -> Result<model::EqState> {
    let known = |id: &str| {
        app.sound().presets().iter().any(|p| p.id == id)
            || id.strip_prefix("autoeq:").is_some_and(|n| sound::clean_name(n).is_some_and(|c| c == n))
    };
    if eq.bands.len() > poximp_engine::MAX_BANDS || preset.as_deref().is_some_and(|p| !known(p)) {
        return Err(Error::Invalid);
    }
    Ok(app.apply_eq(eq, preset))
}

#[tauri::command]
async fn eq_save_preset(app: State<'_, App>, name: String) -> Result<model::EqState> {
    let name = sound::clean_name(&name).ok_or(Error::Invalid)?;
    let mut s = app.sound();
    let id = s.save_preset(name).ok_or(Error::Invalid)?;
    s.cfg.preset = Some(id);
    s.save();
    Ok(model::eq_state(&s))
}

#[tauri::command]
async fn eq_delete_preset(app: State<'_, App>, id: String) -> Result<model::EqState> {
    let mut s = app.sound();
    if !s.delete_preset(&id) {
        return Err(Error::NotFound);
    }
    s.save();
    Ok(model::eq_state(&s))
}

#[tauri::command]
async fn set_normalization(app: State<'_, App>, mode: Norm) -> Result<model::EqState> {
    app.set_normalization(mode);
    let mut s = app.sound();
    s.cfg.normalization = mode;
    s.save();
    Ok(model::eq_state(&s))
}

#[tauri::command]
async fn autoeq_search(app: State<'_, App>, q: String) -> Result<Vec<autoeq::Entry>> {
    let q = trimmed(&q);
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let list = autoeq::index(&app.data_dir.join("autoeq-index.md")).await.ok_or(Error::Network)?;
    Ok(autoeq::search(&list, q))
}

/// `path` = an entry from `autoeq_search` (only index entries are ever downloaded).
#[tauri::command]
async fn autoeq_apply(app: State<'_, App>, path: String) -> Result<model::EqState> {
    if !autoeq::safe_path(&path) {
        return Err(Error::Invalid);
    }
    let list = autoeq::index(&app.data_dir.join("autoeq-index.md")).await.ok_or(Error::Network)?;
    let entry = list.into_iter().find(|e| e.path == path).ok_or(Error::NotFound)?;
    let text = autoeq::fetch(&entry.path).await.ok_or(Error::Network)?;
    let (preamp, bands) = autoeq::parse_parametric(&text).ok_or(Error::NotFound)?;
    let auto_preamp = app.sound().cfg.eq.auto_preamp;
    let name = sound::clean_name(&entry.name).ok_or(Error::NotFound)?;
    Ok(app.apply_eq(EqSettings { enabled: true, preamp, auto_preamp, bands }, Some(format!("autoeq:{name}"))))
}

// --- fake-quality detector switch ---

#[tauri::command]
async fn quality_settings_get(app: State<'_, App>) -> Result<sound::QualitySettings> {
    Ok(app.sound().cfg.quality)
}

/// Turning the analysis back on wakes the worker at once.
#[tauri::command]
async fn quality_settings_set(app: State<'_, App>, settings: sound::QualitySettings) -> Result<sound::QualitySettings> {
    {
        let mut s = app.sound();
        s.cfg.quality = settings;
        s.save();
    }
    let _ = app.wake_quality.send(());
    Ok(settings)
}

#[tauri::command]
async fn shortcuts_list(app: State<'_, App>) -> Result<Vec<media::ShortcutInfo>> {
    Ok(app.shortcuts.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

// --- phone remote ---

#[tauri::command]
async fn remote_get(app: State<'_, App>) -> Result<remote::Info> {
    Ok(remote::info(&app))
}

/// `ip` = one of `remote_get().ips` (None = the saved one, else the default route's).
#[tauri::command]
async fn remote_set(app: State<'_, App>, on: bool, ip: Option<String>) -> Result<remote::Info> {
    remote::set(&app, on, ip)
}

#[tauri::command]
async fn remote_rotate(app: State<'_, App>) -> Result<remote::Info> {
    remote::rotate(&app)
}

// --- opening files from Explorer / Finder ---

/// Files refused since the last call ("open:rejected" says when; the UI also asks once at start).
#[tauri::command]
async fn open_rejected(app: State<'_, App>) -> Result<Vec<open::Rejected>> {
    Ok(std::mem::take(&mut *app.rejected.lock().unwrap_or_else(|e| e.into_inner())))
}

/// Windows Settings, Default apps, on PoxiMP's page. "PoxiMP" is the RegisteredApplications name
/// the installer writes per user (installMode currentUser; per machine it is registeredAppMachine).
/// Windows 10 ignores the parameter and shows the general page.
#[tauri::command]
async fn open_default_apps(app: AppHandle) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let o = app.opener();
    o.open_url("ms-settings:defaultapps?registeredAppUser=PoxiMP", None::<&str>)
        .or_else(|_| o.open_url("ms-settings:defaultapps", None::<&str>))
        .map_err(|_| Error::NotFound)
}

/// macOS: the mini player's traffic lights at (x, vertical centre), or None for AppKit's place.
#[tauri::command]
fn window_lights(window: tauri::Window, at: Option<(f64, f64)>) {
    lights::place(&window, at)
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

// --- covers: cover://localhost/<track id> (http://cover.localhost/<id> on Windows) ---

fn cover_response(app: &AppHandle, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let found = req.uri().path().trim_start_matches('/').parse::<i64>().ok().and_then(|id| app.state::<App>().cover(id));
    let b = Response::builder().header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    match found {
        Some((bytes, mime)) => b
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "max-age=3600")
            .body(bytes),
        None => b.status(StatusCode::NOT_FOUND).body(Vec::new()),
    }
    .unwrap_or_default()
}

/// Brings the window back to the front.
fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn setup(app: &mut tauri::App) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let db = dir.join("library.db");
    let lib = Library::open(&db)?;
    // Opened files of earlier sessions nothing refers to any more (the queue starts empty).
    let _ = lib.prune_external();
    let has_roots = !lib.roots()?.is_empty();

    let sound = Sound::load(&dir.join("sound.json"));
    let (engine, events) = Engine::new();
    engine.send(Command::SetVolume(START_VOLUME));
    engine.send(Command::SetEq(sound.cfg.eq.clone()));
    engine.send(Command::SetFx(sound.cfg.fx));
    let out = &sound.cfg.output;
    if out.device_id.is_some() || out.bit_perfect {
        engine.send(Command::SetOutput { device: out.device_id.clone(), bit_perfect: out.bit_perfect });
    }
    let mut player = Player::new(START_VOLUME);
    player.norm = sound.cfg.normalization;
    player.output = out.clone();
    let (scan_tx, scan_rx) = std::sync::mpsc::channel();
    let (wake_tx, wake_rx) = std::sync::mpsc::channel();
    let (wave_tx, wave_rx) = std::sync::mpsc::channel();
    let hub = Arc::new(remote::Hub::default());
    app.manage(App {
        app: handle.clone(),
        engine,
        lib: Mutex::new(lib),
        player: Mutex::new(player),
        covers: dir.join("covers"),
        scan: Mutex::new(scan_tx),
        wake_quality: wake_tx,
        wake_wave: wave_tx,
        analyzing: AtomicI64::new(0),
        sound: Mutex::new(sound),
        shortcuts: Mutex::new(Vec::new()),
        data_dir: dir.clone(),
        remote: Mutex::new(None),
        hub: hub.clone(),
        opening: Mutex::new(open::Batch::default()),
        rejected: Mutex::new(Vec::new()),
    });
    // The phones' live streams wake on the same events the UI gets.
    for (event, queue) in [("player:state", false), ("player:track", false), ("player:queue", true)] {
        let hub = hub.clone();
        handle.listen_any(event, move |_| hub.poke(queue));
    }
    player::spawn_events(handle.clone(), events);
    work::spawn_scanner(handle.clone(), Library::open(&db)?, scan_rx);
    work::spawn_quality(handle.clone(), Library::open(&db)?, wake_rx);
    work::spawn_waveform(handle.clone(), Library::open(&db)?, wave_rx);

    // Windows are built here, once managed state exists: if the UI loaded first its first
    // invokes would fail. The UI shows the window itself after the first paint.
    for w in &app.config().app.windows {
        tauri::WebviewWindowBuilder::from_config(app.handle(), w)?.build()?;
    }

    let state = app.state::<App>();
    media::init(app.handle());
    media::register_shortcuts(app.handle(), &state.shortcuts);
    if has_roots {
        state.queue_scan(ScanJob::Rescan);
    }
    // Cold start from a double-click: the files are on the command line.
    let (ok, bad) = open::parse_args(std::env::args_os().skip(1), &std::env::current_dir().unwrap_or_default());
    state.open_files(ok, bad);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // A double-click while PoxiMP runs starts a second process: it hands its files over here.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            show_window(app);
            if let Some(state) = app.try_state::<App>() {
                let (ok, bad) = open::parse_args(argv.into_iter().skip(1).map(OsString::from), Path::new(&cwd));
                state.open_files(ok, bad);
            }
        }))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol("cover", |ctx, req, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || responder.respond(cover_response(&app, &req)));
        })
        .setup(setup)
        .on_window_event(|window, event| {
            match event {
                WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => window.state::<App>().open_paths(paths.clone()),
                WindowEvent::Resized(_) => lights::reapply(window),
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            library_add_folder,
            library_remove_folder,
            library_scan,
            library_page,
            library_search,
            search_all,
            library_folders,
            library_artists,
            library_folder_tree,
            quality_report,
            waveform_get,
            play_track,
            play_queue,
            play_view,
            queue_get,
            queue_play_next,
            queue_add,
            queue_remove,
            queue_move,
            queue_clear,
            queue_jump,
            resume_get,
            resume_clear,
            playlist_list,
            playlist_create,
            playlist_rename,
            playlist_set_rules,
            playlist_delete,
            playlist_page,
            smart_preview,
            playlist_add,
            playlist_remove,
            playlist_move,
            playlist_play,
            lyrics_get,
            lyrics_online_get,
            lyrics_online_set,
            radio_start,
            mixes_list,
            stats,
            wrapped,
            import_aimp_playlist,
            import_itunes_xml,
            import_m3u,
            export_m3u,
            export_stats_backup,
            import_stats_backup,

            toggle,
            next,
            previous,
            stop,
            ab_set,
            sleep_set,
            seek,
            set_volume,
            set_shuffle,
            set_repeat,
            viz_frame,
            output_info,
            output_devices,
            output_set,
            player_sync,
            eq_get,
            eq_presets,
            eq_set,
            eq_save_preset,
            eq_delete_preset,
            set_normalization,
            fx_get,
            fx_set,
            autoeq_search,
            autoeq_apply,
            quality_settings_get,
            quality_settings_set,
            shortcuts_list,
            remote_get,
            remote_set,
            remote_rotate,
            open_rejected,
            open_default_apps,
            window_lights
        ])
        .build(tauri::generate_context!())
        .expect("error while building PoxiMP");
    app.run(|_app, _event| {
        // macOS "open with" / double-click in Finder.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = _event {
            let paths = urls.into_iter().filter_map(|u| u.to_file_path().ok()).collect();
            if let Some(state) = _app.try_state::<App>() {
                state.open_paths(paths);
            }
        }
    });
}
