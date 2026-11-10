//! Folder scan: walk, stat, read tags with lofty, cache covers, upsert in batched transactions.

use crate::{now, path_key, Library, Result};
use lofty::file::FileType;
use lofty::picture::PictureType;
use lofty::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, UNIX_EPOCH};

const EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "aiff", "aif", "m4a", "alac", "aac", "ogg", "oga", "opus", "wv", "ape",
];
const BATCH: usize = 500;
const FOLDER_COVERS: &[&str] = &[
    "cover.jpg", "folder.jpg", "front.jpg", "cover.png", "folder.png", "front.png",
];

/// Default artist separators. Matched case-insensitively; a separator containing "feat"/"ft"
/// marks every artist after it as featured.
pub const DEFAULT_SEPARATORS: &[&str] = &[
    ", ", " & ", " feat. ", " ft. ", " featuring ", " (feat. ", " (ft. ", " x ", ";",
];

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub found: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    /// Known tracks under the scanned roots that are gone (marked unavailable, kept in the DB).
    pub missing: usize,
    /// Files that could not be stat'ed or parsed.
    pub errors: usize,
    pub cancelled: bool,
    pub elapsed_ms: u64,
}

/// Split an artist tag into (name, featured). "A, B & C feat. D" -> A, B, C, D(feat).
pub fn split_artists<S: AsRef<str>>(s: &str, seps: &[S]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut push = |raw: &str, feat: bool| {
        let mut name = raw.trim();
        // "X (feat. Y)" leaves "Y)" behind.
        if name.ends_with(')') && name.matches(')').count() > name.matches('(').count() {
            name = name[..name.len() - 1].trim_end();
        }
        if !name.is_empty() && !out.iter().any(|(n, _)| n.to_lowercase() == name.to_lowercase()) {
            out.push((name.to_string(), feat));
        }
    };
    let (mut start, mut i, mut feat) = (0, 0, false);
    while i < s.len() {
        // `get` returns None off char boundaries, so multibyte text is never split mid-char.
        let hit = seps.iter().map(AsRef::as_ref).find(|sep| {
            !sep.is_empty()
                && s.get(i..i + sep.len())
                    .is_some_and(|w| w.eq_ignore_ascii_case(sep))
        });
        match hit {
            Some(sep) => {
                push(&s[start..i], feat);
                let l = sep.to_ascii_lowercase();
                feat |= l.contains("feat") || l.contains("ft");
                i += sep.len();
                start = i;
            }
            None => i += 1,
        }
    }
    push(&s[start..], feat);
    out
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Stable album identity: (album artist, else first main artist) + album, like foobar's
/// %album artist%. No album tag = one album per folder.
/// ponytail: untagged compilations split per artist; also key by folder if users complain.
fn album_key(who: &str, album: &str, folder_key: &str) -> String {
    let basis = if album.is_empty() {
        format!("\u{1f}{folder_key}")
    } else if !who.is_empty() {
        format!("{who}\u{1f}{album}")
    } else {
        format!("{album}\u{1f}{folder_key}")
    };
    format!("{:016x}", fnv1a(&basis.to_lowercase()))
}

/// An audio file the scanner reads (by extension).
pub fn supported(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

#[cfg(windows)]
fn hidden(e: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
    e.metadata()
        .is_ok_and(|m| m.file_attributes() & HIDDEN_OR_SYSTEM != 0)
}

#[cfg(not(windows))]
fn hidden(_: &fs::DirEntry) -> bool {
    false
}

/// Symlinks are skipped (no loops). ponytail: follow them with a visited set if users ask.
/// `skip`: path keys of folders taken out of the library.
fn walk(dir: &Path, out: &mut Vec<PathBuf>, cancel: &AtomicBool, skip: &HashSet<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if e.file_name().to_string_lossy().starts_with('.') || hidden(&e) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            if skip.is_empty() || !skip.contains(&path_key(&p.to_string_lossy())) {
                walk(&p, out, cancel, skip);
            }
        } else if ft.is_file() && supported(&p) {
            out.push(p);
        }
    }
}

/// Covers cached once per album per scan, stored as-is (no re-encode) as `<album_key>.<ext>`.
/// ponytail: no resizing, big embedded art is served full size; add `image` (jpeg,png) thumbnails
/// if the album grid gets heavy.
struct Covers<'a> {
    dir: &'a Path,
    done: HashMap<String, Option<String>>,
}

impl Covers<'_> {
    fn get(&mut self, key: &str, folder: &Path, embedded: Option<&[u8]>) -> Option<String> {
        match (self.done.get(key), embedded) {
            (Some(Some(name)), _) => return Some(name.clone()),
            (Some(None), None) => return None,
            _ => {}
        }
        let name = match embedded {
            Some(data) => {
                let ext = if data.starts_with(b"\x89PNG") { "png" } else { "jpg" };
                let name = format!("{key}.{ext}");
                fs::write(self.dir.join(&name), data).ok().map(|_| name)
            }
            None => FOLDER_COVERS.iter().find_map(|f| {
                let src = folder.join(f);
                let ext = if f.ends_with("png") { "png" } else { "jpg" };
                let name = format!("{key}.{ext}");
                src.is_file()
                    .then(|| fs::copy(&src, self.dir.join(&name)).ok().map(|_| name))
                    .flatten()
            }),
        };
        self.done.insert(key.to_string(), name.clone());
        name
    }
}

struct Meta {
    title: String,
    artist: String,
    artists: Vec<(String, bool)>,
    album_artist: String,
    album_artists: Vec<(String, bool)>,
    album: String,
    album_key: String,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    year: Option<i32>,
    genre: String,
    duration_ms: i64,
    codec: &'static str,
    container: &'static str,
    bitrate_kbps: Option<u32>,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    channels: Option<u8>,
    has_cover: bool,
    cover: Option<String>,
    /// track gain, track peak, album gain, album peak
    rg: [Option<f64>; 4],
}

/// Bump when the scan starts reading new tags: older rows are re-read once.
pub(crate) const TAGS_REV: i64 = 1;

/// "-6.54 dB", "+1.2 dB", "0.988" -> number.
pub(crate) fn leading_number(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(s.len(), |(i, _)| i);
    s[..end].parse().ok().filter(|v: &f64| v.is_finite())
}

fn read_meta(
    path: &Path,
    folder: &Path,
    seps: &[String],
    covers: &mut Covers,
) -> std::result::Result<Meta, lofty::error::FileParseError> {
    let f = lofty::read_from_path(path)?;
    let p = f.properties();
    let (container, codec) = match f.file_type() {
        FileType::Mpeg => ("MP3", "MP3"),
        FileType::Flac => ("FLAC", "FLAC"),
        FileType::Wav => ("WAV", "PCM"),
        FileType::Aiff => ("AIFF", "PCM"),
        // Only lossless MP4 codecs (ALAC, FLAC-in-MP4) carry a bit depth in lofty.
        FileType::Mp4 if p.bit_depth().is_some() => ("M4A", "ALAC"),
        FileType::Mp4 => ("M4A", "AAC"),
        FileType::Aac => ("AAC", "AAC"),
        FileType::Vorbis => ("OGG", "Vorbis"),
        FileType::Opus => ("OGG", "Opus"),
        FileType::Speex => ("OGG", "Speex"),
        FileType::WavPack => ("WV", "WavPack"),
        FileType::Ape => ("APE", "APE"),
        FileType::Mpc => ("MPC", "Musepack"),
        _ => ("?", "?"),
    };
    let tag = f.primary_tag().or_else(|| f.first_tag());
    let text = |k: ItemKey| -> Vec<String> {
        tag.map(|t| {
            t.get_strings(k)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
    };
    let artist_vals = text(ItemKey::TrackArtist);
    // A multi-value ARTISTS tag is more precise than splitting the display string.
    let artists_tag = text(ItemKey::TrackArtists);
    let split_all = |vals: &[String]| {
        let mut out: Vec<(String, bool)> = Vec::new();
        for v in vals {
            for a in split_artists(v, seps) {
                if !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&a.0)) {
                    out.push(a);
                }
            }
        }
        out
    };
    let artists = split_all(if artists_tag.is_empty() {
        &artist_vals
    } else {
        &artists_tag
    });
    let album_artist_vals = text(ItemKey::AlbumArtist);
    let album_artists = split_all(&album_artist_vals);
    let album_artist = album_artist_vals.join(", ");
    let album = tag
        .and_then(|t| t.album())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let folder_str = folder.to_string_lossy();
    let who = album_artists
        .first()
        .or(artists.first())
        .map_or("", |(n, _)| n.as_str());
    let album_key = album_key(who, &album, &path_key(&folder_str));
    let year = tag.and_then(|t| t.date()).map(|d| d.year as i32).or_else(|| {
        text(ItemKey::Year)
            .first()
            .and_then(|y| y.get(..4)?.parse().ok())
    });
    let picture = f
        .tags()
        .iter()
        .flat_map(|t| t.pictures())
        .min_by_key(|pic| pic.pic_type() != PictureType::CoverFront);
    let cover = covers.get(&album_key, folder, picture.map(|pic| pic.data()));
    // Any tag may carry ReplayGain (an MP3 can have both ID3v2 and APE).
    let rg = [
        ItemKey::ReplayGainTrackGain,
        ItemKey::ReplayGainTrackPeak,
        ItemKey::ReplayGainAlbumGain,
        ItemKey::ReplayGainAlbumPeak,
    ]
    .map(|k| f.tags().iter().find_map(|t| t.get_string(k).and_then(leading_number)));
    Ok(Meta {
        title: tag
            .and_then(|t| t.title())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
            }),
        artist: artist_vals.join(", "),
        artists,
        album_artist,
        album_artists,
        album,
        album_key,
        track_no: tag.and_then(|t| t.track()),
        disc_no: tag.and_then(|t| t.disk()),
        year,
        genre: tag
            .and_then(|t| t.genre())
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        duration_ms: p.duration().as_millis() as i64,
        codec,
        container,
        bitrate_kbps: p.audio_bitrate().or(p.overall_bitrate()),
        sample_rate: p.sample_rate(),
        bit_depth: p.bit_depth(),
        channels: p.channels(),
        has_cover: picture.is_some(),
        cover,
        rg,
    })
}

const UPSERT: &str = "INSERT INTO tracks (path, path_key, folder, size, mtime, title, artist, \
    album_artist, album, album_key, track_no, disc_no, year, genre, duration_ms, codec, container, \
    bitrate_kbps, sample_rate, bit_depth, channels, has_cover, cover, added_at, \
    rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, tags_rev, in_library) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, \
    ?25, ?26, ?27, ?28, ?29, ?30) \
    ON CONFLICT(path_key) DO UPDATE SET path = excluded.path, folder = excluded.folder, \
    size = excluded.size, mtime = excluded.mtime, title = excluded.title, artist = excluded.artist, \
    album_artist = excluded.album_artist, album = excluded.album, album_key = excluded.album_key, \
    track_no = excluded.track_no, disc_no = excluded.disc_no, year = excluded.year, \
    genre = excluded.genre, duration_ms = excluded.duration_ms, codec = excluded.codec, \
    container = excluded.container, bitrate_kbps = excluded.bitrate_kbps, \
    sample_rate = excluded.sample_rate, bit_depth = excluded.bit_depth, \
    channels = excluded.channels, has_cover = excluded.has_cover, cover = excluded.cover, \
    rg_track_gain = excluded.rg_track_gain, rg_track_peak = excluded.rg_track_peak, \
    rg_album_gain = excluded.rg_album_gain, rg_album_peak = excluded.rg_album_peak, \
    tags_rev = excluded.tags_rev, available = 1, in_library = MAX(in_library, excluded.in_library) \
    RETURNING id";

impl Library {
    /// Incremental scan of `roots`: unchanged files (same size and mtime) are skipped, missing
    /// ones are marked unavailable. Covers go to `covers_dir`. Commits every `BATCH` files, so a
    /// cancelled scan keeps what it already did (but marks nothing missing). Every file found
    /// becomes a library track, opened files under these roots included.
    pub fn scan(
        &mut self,
        roots: &[impl AsRef<Path>],
        covers_dir: &Path,
        cancel: &AtomicBool,
        progress: impl FnMut(ScanProgress),
    ) -> Result<ScanReport> {
        self.scan_as(roots, covers_dir, cancel, progress, true)
    }

    /// Reads files opened from outside (double-click, "Open with", a drop) so they can play.
    /// New ones stay out of the library (`in_library = 0`); library tracks stay library tracks.
    pub fn scan_external(&mut self, files: &[impl AsRef<Path>], covers_dir: &Path) -> Result<ScanReport> {
        self.scan_as(files, covers_dir, &AtomicBool::new(false), |_| {}, false)
    }

    fn scan_as(
        &mut self,
        roots: &[impl AsRef<Path>],
        covers_dir: &Path,
        cancel: &AtomicBool,
        mut progress: impl FnMut(ScanProgress),
        library: bool,
    ) -> Result<ScanReport> {
        let t0 = Instant::now();
        let mut files = Vec::new();
        let skip: HashSet<String> = if library { self.excluded()?.into_iter().collect() } else { HashSet::new() };
        for r in roots {
            let r = r.as_ref();
            // A root may also be a single file (drag-drop, "open with").
            if r.is_file() && supported(r) {
                files.push(r.to_path_buf());
            } else {
                walk(r, &mut files, cancel, &skip);
            }
        }
        fs::create_dir_all(covers_dir)?;
        let known: HashMap<String, (i64, i64, i64, bool, bool)> = {
            let mut st = self
                .conn
                .prepare("SELECT path_key, id, size, mtime, available, tags_rev, in_library FROM tracks")?;
            let rows = st.query_map([], |r| {
                // Rows read by an older scan look changed, so their tags are read again.
                let mtime: i64 = if r.get::<_, i64>(5)? < TAGS_REV { i64::MIN } else { r.get(3)? };
                Ok((r.get(0)?, (r.get(1)?, r.get(2)?, mtime, r.get(4)?, r.get(6)?)))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut seen = HashSet::new();
        let mut covers = Covers {
            dir: covers_dir,
            done: HashMap::new(),
        };
        let mut rep = ScanReport {
            found: files.len(),
            ..Default::default()
        };
        let total = files.len();
        let added_at = now();

        'outer: for (ci, chunk) in files.chunks(BATCH).enumerate() {
            let tx = self.conn.transaction()?;
            for (j, path) in chunk.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    tx.commit()?;
                    break 'outer;
                }
                progress(ScanProgress {
                    done: ci * BATCH + j,
                    total,
                });
                let (Some(path_str), Some(folder), Ok(md)) =
                    (path.to_str(), path.parent(), fs::metadata(path))
                else {
                    rep.errors += 1;
                    continue;
                };
                let size = md.len() as i64;
                let mtime = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                let key = path_key(path_str);
                let old = known.get(&key);
                if let Some(&(id, s, m, available, in_library)) = old {
                    // Seen even if parsing fails below: an unreadable file is not a missing one.
                    seen.insert(id);
                    if s == size && m == mtime {
                        // A library scan takes in an opened file it finds under a root.
                        if !available || (library && !in_library) {
                            tx.execute(
                                "UPDATE tracks SET available = 1, in_library = MAX(in_library, ?2) WHERE id = ?1",
                                rusqlite::params![id, library],
                            )?;
                        }
                        rep.unchanged += 1;
                        continue;
                    }
                }
                let Ok(m) = read_meta(path, folder, &self.separators, &mut covers) else {
                    rep.errors += 1;
                    continue;
                };
                let id: i64 = tx.prepare_cached(UPSERT)?.query_row(
                    rusqlite::params![
                        path_str,
                        key,
                        folder.to_string_lossy(),
                        size,
                        mtime,
                        m.title,
                        m.artist,
                        m.album_artist,
                        m.album,
                        m.album_key,
                        m.track_no,
                        m.disc_no,
                        m.year,
                        m.genre,
                        m.duration_ms,
                        m.codec,
                        m.container,
                        m.bitrate_kbps,
                        m.sample_rate,
                        m.bit_depth,
                        m.channels,
                        m.has_cover,
                        m.cover,
                        added_at,
                        m.rg[0],
                        m.rg[1],
                        m.rg[2],
                        m.rg[3],
                        TAGS_REV,
                        library,
                    ],
                    |r| r.get(0),
                )?;
                tx.execute("DELETE FROM track_artists WHERE track_id = ?1", [id])?;
                let mut artist_id = tx.prepare_cached(
                    "INSERT INTO artists (name) VALUES (?1) \
                     ON CONFLICT(name) DO UPDATE SET name = name RETURNING id",
                )?;
                let mut link = tx.prepare_cached(
                    "INSERT OR IGNORE INTO track_artists (track_id, artist_id, role, position) \
                     VALUES (?1, ?2, ?3, ?4)",
                )?;
                let roles = m
                    .artists
                    .iter()
                    .map(|(n, feat)| (n, if *feat { "feat" } else { "main" }))
                    .chain(m.album_artists.iter().map(|(n, _)| (n, "album")));
                for (pos, (name, role)) in roles.enumerate() {
                    let aid: i64 = artist_id.query_row([name], |r| r.get(0))?;
                    link.execute(rusqlite::params![id, aid, role, pos as i64])?;
                }
                if old.is_some() {
                    rep.updated += 1;
                } else {
                    rep.added += 1;
                }
            }
            tx.commit()?;
        }

        // Also covers a cancel during the walk: a partial file list must not mark anything missing.
        rep.cancelled = cancel.load(Ordering::Relaxed);
        let tx = self.conn.transaction()?;
        if !rep.cancelled {
            let roots: Vec<String> = roots
                .iter()
                .map(|r| {
                    let mut k = path_key(&r.as_ref().to_string_lossy());
                    if !k.ends_with(['/', '\\']) {
                        k.push(std::path::MAIN_SEPARATOR);
                    }
                    k
                })
                .collect();
            let mut gone = tx.prepare("UPDATE tracks SET available = 0 WHERE id = ?1")?;
            for (key, &(id, _, _, available, _)) in &known {
                if available && !seen.contains(&id) && roots.iter().any(|r| key.starts_with(r)) {
                    gone.execute([id])?;
                    rep.missing += 1;
                }
            }
        }
        // Tracks of an album scanned before the album's cover was found get it now.
        tx.execute_batch(
            "UPDATE tracks SET cover = (SELECT MAX(c.cover) FROM tracks c WHERE c.album_key = tracks.album_key) \
                 WHERE cover IS NULL; \
             DELETE FROM artists WHERE id NOT IN (SELECT artist_id FROM track_artists);",
        )?;
        tx.commit()?;
        progress(ScanProgress { done: total, total });
        rep.elapsed_ms = t0.elapsed().as_millis() as u64;
        Ok(rep)
    }
}
