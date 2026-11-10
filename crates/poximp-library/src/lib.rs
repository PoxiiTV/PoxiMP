//! PoxiMP library: SQLite (rusqlite bundled, WAL, FTS5), folder scan, tags via lofty, covers.
//!
//! One `Library` = one SQLite connection (not `Sync`). The app keeps one behind a mutex for
//! queries and opens a second one on the scan thread: with WAL, readers never wait for the scan.

mod import;
pub mod lyrics;
pub mod radio;
mod scan;
mod search;
pub mod smart;
mod stats;

pub use import::{m3u_files, ImportReport};
pub use search::{SearchGroup, SearchHit};
pub use scan::{split_artists, supported, ScanProgress, ScanReport, DEFAULT_SEPARATORS};
pub use smart::SmartRules;
pub use stats::{MonthStat, Range, Stats, Streak, TopItem, TopTrack, Wrapped};

use rusqlite::types::Value as Sql;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    #[error("not found")]
    NotFound,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Index i = migration to `user_version` i + 1. Append only, never edit a shipped one.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE tracks (
    id             INTEGER PRIMARY KEY,
    path           TEXT    NOT NULL,          -- as found on disk (used to open the file)
    path_key       TEXT    NOT NULL UNIQUE,   -- normalised: NFC on macOS, lowercase on Windows/macOS
    folder         TEXT    NOT NULL,
    size           INTEGER NOT NULL,
    mtime          INTEGER NOT NULL,
    title          TEXT    NOT NULL,
    artist         TEXT    NOT NULL DEFAULT '',
    album_artist   TEXT    NOT NULL DEFAULT '',
    album          TEXT    NOT NULL DEFAULT '',
    album_key      TEXT    NOT NULL,
    track_no       INTEGER,
    disc_no        INTEGER,
    year           INTEGER,
    genre          TEXT    NOT NULL DEFAULT '',
    duration_ms    INTEGER NOT NULL DEFAULT 0,
    codec          TEXT    NOT NULL,
    container      TEXT    NOT NULL,
    bitrate_kbps   INTEGER,
    sample_rate    INTEGER,
    bit_depth      INTEGER,
    channels       INTEGER,
    has_cover      INTEGER NOT NULL DEFAULT 0, -- embedded picture present
    cover          TEXT,                       -- file name in the covers cache dir
    available      INTEGER NOT NULL DEFAULT 1,
    added_at       INTEGER NOT NULL,
    play_count     INTEGER NOT NULL DEFAULT 0,
    skip_count     INTEGER NOT NULL DEFAULT 0,
    last_played_at INTEGER,
    rating         INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
    quality_verdict TEXT,
    quality_json   TEXT,
    quality_key    TEXT                        -- "size:mtime" the verdict was computed for
);
CREATE INDEX tracks_folder ON tracks(folder);
CREATE INDEX tracks_album ON tracks(album_key, disc_no, track_no);

CREATE TABLE artists (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE track_artists (
    track_id  INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    artist_id INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
    role      TEXT    NOT NULL CHECK (role IN ('main', 'feat', 'album')),
    position  INTEGER NOT NULL,              -- order as tagged
    PRIMARY KEY (track_id, artist_id, role)
) WITHOUT ROWID;
CREATE INDEX track_artists_artist ON track_artists(artist_id);

CREATE TABLE playlists (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    kind       TEXT    NOT NULL CHECK (kind IN ('manual', 'smart')),
    rules_json TEXT,
    created_at INTEGER NOT NULL
);
CREATE TABLE playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    PRIMARY KEY (playlist_id, position)
) WITHOUT ROWID;

CREATE TABLE plays (
    id          INTEGER PRIMARY KEY,
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    played_at   INTEGER NOT NULL,
    ms_listened INTEGER NOT NULL
);
CREATE INDEX plays_time ON plays(played_at);

CREATE TABLE resume_positions (
    track_id    INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    position_ms INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title, artist, album, content = 'tracks', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album) VALUES (new.id, new.title, new.artist, new.album);
END;
CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album) VALUES ('delete', old.id, old.title, old.artist, old.album);
END;
CREATE TRIGGER tracks_au AFTER UPDATE OF title, artist, album ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album) VALUES ('delete', old.id, old.title, old.artist, old.album);
    INSERT INTO tracks_fts(rowid, title, artist, album) VALUES (new.id, new.title, new.artist, new.album);
END;
"#, r#"
CREATE TABLE roots (
    id       INTEGER PRIMARY KEY,
    path_key TEXT NOT NULL UNIQUE,
    path     TEXT NOT NULL
);
"#, r#"
-- ReplayGain tags (dB / linear peak). tags_rev < scan::TAGS_REV makes the next scan re-read
-- the file's tags even if it did not change, so existing libraries pick these up.
ALTER TABLE tracks ADD COLUMN rg_track_gain REAL;
ALTER TABLE tracks ADD COLUMN rg_track_peak REAL;
ALTER TABLE tracks ADD COLUMN rg_album_gain REAL;
ALTER TABLE tracks ADD COLUMN rg_album_peak REAL;
ALTER TABLE tracks ADD COLUMN tags_rev INTEGER NOT NULL DEFAULT 0;
"#, r#"
-- Stats (per track, per day, top lists) read plays by track.
CREATE INDEX plays_track ON plays(track_id, played_at);
-- Counts brought in from other players, per source: a re-import applies only the difference.
CREATE TABLE imported_stats (
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    source   TEXT    NOT NULL,
    plays    INTEGER NOT NULL,
    skips    INTEGER NOT NULL,
    PRIMARY KEY (track_id, source)
) WITHOUT ROWID;
"#, r#"
-- Online lyrics (LRCLIB) per track, "not found" included (source 'none') so it is not asked
-- again until `lyrics::RETRY_DAYS` have passed. Local lyrics (tags, .lrc) are read fresh.
CREATE TABLE lyrics (
    track_id   INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    source     TEXT    NOT NULL,
    synced     TEXT,                       -- LRC text
    plain      TEXT,
    fetched_at INTEGER NOT NULL
);
-- Sound features for radio / mixes (analysis pass 2). vec NULL = unreadable file.
CREATE TABLE track_features (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    feat_key TEXT    NOT NULL,             -- "rev:size:mtime" the vector was computed for
    vec      BLOB                          -- f32 little-endian
);
"#, r#"
-- Seekbar waveform: u8 peaks then u8 RMS (poximp_quality::waveform). Empty = unreadable file.
CREATE TABLE track_waveforms (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    wave_key TEXT    NOT NULL,             -- "rev:size:mtime" it was computed for
    peaks    BLOB    NOT NULL
);
"#, r#"
-- Files opened from outside the music folders (double-click, "Open with", a drop) play from
-- here but stay out of every library view. Rows of older versions outside every root are such files.
ALTER TABLE tracks ADD COLUMN in_library INTEGER NOT NULL DEFAULT 1;
UPDATE tracks SET in_library = 0 WHERE NOT EXISTS (SELECT 1 FROM roots r WHERE tracks.path_key = r.path_key
    OR substr(tracks.path_key, 1, length(r.path_key) + 1) IN (r.path_key || '/', r.path_key || '\'));
"#, r#"
-- Folders inside a music folder taken out of the library (right-click > Quitar): the scan skips them.
CREATE TABLE excluded (
    path_key TEXT PRIMARY KEY
) WITHOUT ROWID;
"#];

/// Bump when the waveform format changes: every track gets a new one.
pub const WAVEFORM_REV: u32 = 1;

/// One track as the UI sees it.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub folder: String,
    pub title: String,
    /// Display string as tagged ("A feat. B"); the split artists live in `artists_of`.
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub album_key: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<i32>,
    pub genre: String,
    pub duration_ms: i64,
    pub codec: String,
    pub container: String,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub has_cover: bool,
    pub cover: Option<String>,
    pub available: bool,
    pub added_at: i64,
    pub play_count: u32,
    pub skip_count: u32,
    pub last_played_at: Option<i64>,
    pub rating: u8,
    pub quality_verdict: Option<String>,
    pub quality_json: Option<String>,
    /// ReplayGain (dB) and peaks (linear), when tagged.
    pub rg_track_gain: Option<f64>,
    pub rg_track_peak: Option<f64>,
    pub rg_album_gain: Option<f64>,
    pub rg_album_peak: Option<f64>,
    /// False = a file opened from outside the music folders: playable, hidden from the library.
    pub in_library: bool,
}

const TRACK_COLS: &str = "t.id, t.path, t.folder, t.title, t.artist, t.album_artist, t.album, \
    t.album_key, t.track_no, t.disc_no, t.year, t.genre, t.duration_ms, t.codec, t.container, \
    t.bitrate_kbps, t.sample_rate, t.bit_depth, t.channels, t.has_cover, t.cover, t.available, \
    t.added_at, t.play_count, t.skip_count, t.last_played_at, t.rating, t.quality_verdict, t.quality_json, \
    t.rg_track_gain, t.rg_track_peak, t.rg_album_gain, t.rg_album_peak, t.in_library";

fn track_from_row(r: &Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: r.get(0)?,
        path: r.get(1)?,
        folder: r.get(2)?,
        title: r.get(3)?,
        artist: r.get(4)?,
        album_artist: r.get(5)?,
        album: r.get(6)?,
        album_key: r.get(7)?,
        track_no: r.get(8)?,
        disc_no: r.get(9)?,
        year: r.get(10)?,
        genre: r.get(11)?,
        duration_ms: r.get(12)?,
        codec: r.get(13)?,
        container: r.get(14)?,
        bitrate_kbps: r.get(15)?,
        sample_rate: r.get(16)?,
        bit_depth: r.get(17)?,
        channels: r.get(18)?,
        has_cover: r.get(19)?,
        cover: r.get(20)?,
        available: r.get(21)?,
        added_at: r.get(22)?,
        play_count: r.get(23)?,
        skip_count: r.get(24)?,
        last_played_at: r.get(25)?,
        rating: r.get(26)?,
        quality_verdict: r.get(27)?,
        quality_json: r.get(28)?,
        rg_track_gain: r.get(29)?,
        rg_track_peak: r.get(30)?,
        rg_album_gain: r.get(31)?,
        rg_album_peak: r.get(32)?,
        in_library: r.get(33)?,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    #[default]
    Artist,
    Title,
    Album,
    Year,
    AddedAt,
    Duration,
    PlayCount,
    LastPlayed,
    Rating,
    Path,
}

/// Paged track listing. Every filter is optional and they combine with AND.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackQuery {
    pub offset: u32,
    /// 0 = default page (200). Capped at 1000.
    pub limit: u32,
    pub sort: SortKey,
    pub desc: bool,
    pub artist_id: Option<i64>,
    pub album_key: Option<String>,
    /// Folder view: this folder, and its subfolders too when `folder_recursive`.
    pub folder: Option<String>,
    pub folder_recursive: bool,
    pub genre: Option<String>,
    /// Free text, same matching as `search` ("" = no filter).
    pub text: String,
    pub include_unavailable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub total: u32,
    /// Sum over all matching rows, not just this page.
    pub duration_ms: i64,
    /// (quality_verdict, count) over all matching rows; `None` = never analysed.
    pub verdicts: Vec<(Option<String>, u32)>,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub key: String,
    pub title: String,
    /// None = several artists (UI shows the i18n "various artists" text).
    pub artist: Option<String>,
    pub year: Option<i32>,
    pub track_count: u32,
    pub duration_ms: i64,
    pub cover: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub track_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackArtist {
    pub id: i64,
    pub name: String,
    /// "main", "feat" or "album".
    pub role: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderNode {
    pub name: String,
    /// Value for `TrackQuery::folder`.
    pub path: String,
    /// Tracks directly in this folder.
    pub track_count: u32,
    /// Tracks in this folder and every subfolder.
    pub total_count: u32,
    pub children: Vec<FolderNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistKind {
    Manual,
    Smart,
}

impl PlaylistKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Smart => "smart",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub kind: PlaylistKind,
    pub rules_json: Option<String>,
    pub created_at: i64,
    pub track_count: u32,
}

/// A track whose quality verdict is missing or stale. Pass `key` back to `set_quality`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityJob {
    pub id: i64,
    pub path: String,
    pub key: String,
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Key used for path identity: NFC on macOS, case-insensitive on Windows/macOS.
pub(crate) fn path_key(p: &str) -> String {
    #[cfg(target_os = "macos")]
    let p: String = {
        use unicode_normalization::UnicodeNormalization;
        p.nfc().collect()
    };
    #[cfg(windows)]
    let p = p.replace('/', "\\");
    if cfg!(any(windows, target_os = "macos")) {
        p.to_lowercase()
    } else {
        p.to_string()
    }
}

fn rules_to_json(r: &SmartRules) -> String {
    serde_json::to_string(r).unwrap_or_else(|_| "{}".into())
}

fn check_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(Error::Invalid("playlist name must be 1-200 characters"));
    }
    Ok(name)
}

pub struct Library {
    conn: Connection,
    /// Artist separators (matched case-insensitively). Default: `DEFAULT_SEPARATORS`.
    pub separators: Vec<String>,
}

impl Library {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;
        let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version as usize > MIGRATIONS.len() {
            return Err(Error::Invalid("library database is newer than this app"));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as u32 + 1)?;
            tx.commit()?;
        }
        Ok(Self {
            conn,
            separators: DEFAULT_SEPARATORS.iter().map(|s| s.to_string()).collect(),
        })
    }

    /// "WHERE ..." (over `tracks t`) and its parameters for a listing (library tracks only).
    fn track_filter(q: &TrackQuery) -> (String, Vec<Sql>) {
        let mut conds = vec!["t.in_library = 1"];
        let mut args = Vec::new();
        if !q.include_unavailable {
            conds.push("t.available = 1");
        }
        if let Some(a) = q.artist_id {
            conds.push("t.id IN (SELECT track_id FROM track_artists WHERE artist_id = ?)");
            args.push(Sql::Integer(a));
        }
        if let Some(k) = &q.album_key {
            conds.push("t.album_key = ?");
            args.push(Sql::Text(k.clone()));
        }
        if let Some(f) = &q.folder {
            args.push(Sql::Text(f.clone()));
            if q.folder_recursive {
                // Prefix compare, not LIKE: folder names may contain % or _.
                let prefix = if f.ends_with(['/', '\\']) {
                    f.clone()
                } else {
                    format!("{f}{}", std::path::MAIN_SEPARATOR)
                };
                conds.push("(t.folder = ? OR substr(t.folder, 1, ?) = ?)");
                args.push(Sql::Integer(prefix.chars().count() as i64));
                args.push(Sql::Text(prefix));
            } else {
                conds.push("t.folder = ?");
            }
        }
        if let Some(g) = &q.genre {
            conds.push("t.genre = ? COLLATE NOCASE");
            args.push(Sql::Text(g.clone()));
        }
        if let Some(f) = fts_query(&q.text) {
            conds.push("t.id IN (SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?)");
            args.push(Sql::Text(f));
        }
        (format!("WHERE {}", conds.join(" AND ")), args)
    }

    /// Whitelisted fragments only: nothing user-provided reaches the SQL text.
    fn track_order(q: &TrackQuery) -> String {
        let dir = if q.desc { "DESC" } else { "ASC" };
        let album_order = "t.album COLLATE NOCASE, t.album_key, t.disc_no, t.track_no, t.path";
        match q.sort {
            SortKey::Artist => format!("t.artist COLLATE NOCASE {dir}, {album_order}"),
            SortKey::Title => format!("t.title COLLATE NOCASE {dir}, t.id"),
            SortKey::Album => format!(
                "t.album COLLATE NOCASE {dir}, t.album_key, t.disc_no, t.track_no, t.path"
            ),
            SortKey::Year => format!("t.year {dir}, {album_order}"),
            SortKey::AddedAt => format!("t.added_at {dir}, t.id"),
            SortKey::Duration => format!("t.duration_ms {dir}, t.id"),
            SortKey::PlayCount => format!("t.play_count {dir}, t.id"),
            SortKey::LastPlayed => format!("t.last_played_at {dir}, t.id"),
            SortKey::Rating => format!("t.rating {dir}, t.id"),
            SortKey::Path => format!("t.path_key {dir}"),
        }
    }

    pub fn list_tracks(&self, q: &TrackQuery) -> Result<Page> {
        let (wh, mut args) = Self::track_filter(q);
        let (total, duration_ms): (u32, i64) = self.conn.query_row(
            &format!("SELECT COUNT(*), COALESCE(SUM(t.duration_ms), 0) FROM tracks t {wh}"),
            params_from_iter(&args),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let verdicts = self
            .conn
            .prepare(&format!(
                "SELECT t.quality_verdict, COUNT(*) FROM tracks t {wh} GROUP BY 1"
            ))?
            .query_map(params_from_iter(&args), |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let limit = match q.limit {
            0 => 200,
            n => n.min(1000),
        };
        args.push(Sql::Integer(limit.into()));
        args.push(Sql::Integer(q.offset.into()));
        let mut st = self.conn.prepare(&format!(
            "SELECT {TRACK_COLS} FROM tracks t {wh} ORDER BY {} LIMIT ? OFFSET ?",
            Self::track_order(q)
        ))?;
        let tracks = st
            .query_map(params_from_iter(&args), track_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Page {
            total,
            duration_ms,
            verdicts,
            tracks,
        })
    }

    /// Every id of a listing in its order (offset / limit ignored), e.g. to play a whole view.
    pub fn view_ids(&self, q: &TrackQuery) -> Result<Vec<i64>> {
        let (wh, args) = Self::track_filter(q);
        let mut st = self.conn.prepare(&format!(
            "SELECT t.id FROM tracks t {wh} ORDER BY {}",
            Self::track_order(q)
        ))?;
        let ids = st
            .query_map(params_from_iter(&args), |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Tracks in the order of `ids` (duplicates kept, unknown ids skipped).
    pub fn tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        let json = serde_json::to_string(ids).unwrap_or_else(|_| "[]".into());
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {TRACK_COLS} FROM json_each(?1) j JOIN tracks t ON t.id = j.value ORDER BY j.key"
        ))?;
        let rows = st
            .query_map([json], track_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Total duration of `ids` (duplicates counted each time).
    pub fn duration_of(&self, ids: &[i64]) -> Result<i64> {
        let json = serde_json::to_string(ids).unwrap_or_else(|_| "[]".into());
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(t.duration_ms), 0) FROM json_each(?1) j JOIN tracks t ON t.id = j.value",
            [json],
            |r| r.get(0),
        )?)
    }

    /// Ids matching smart playlist rules (available tracks only), in the rules' order.
    pub fn smart_ids(&self, rules: &SmartRules) -> Result<Vec<i64>> {
        let c = rules.compile(now())?;
        let mut args = c.args;
        args.push(Sql::Integer(c.limit));
        let mut st = self.conn.prepare(&format!(
            "SELECT t.id FROM tracks t WHERE t.available = 1 AND t.in_library = 1 AND {} ORDER BY {} LIMIT ?",
            c.wh, c.order
        ))?;
        let ids = st
            .query_map(params_from_iter(&args), |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Prefix, accent- and case-insensitive search over title/artist/album ("cancion" finds "Canción").
    pub fn search(&self, q: &str, limit: u32) -> Result<Vec<Track>> {
        let Some(fts) = fts_query(q) else {
            return Ok(Vec::new());
        };
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {TRACK_COLS} FROM tracks_fts JOIN tracks t ON t.id = tracks_fts.rowid \
             WHERE tracks_fts MATCH ?1 AND t.available = 1 AND t.in_library = 1 ORDER BY tracks_fts.rank LIMIT ?2"
        ))?;
        let rows = st
            .query_map(params![fts, limit.clamp(1, 500)], track_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn track_by_id(&self, id: i64) -> Result<Option<Track>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TRACK_COLS} FROM tracks t WHERE t.id = ?1"),
                [id],
                track_from_row,
            )
            .optional()?)
    }

    /// Id of the track at `path` (any spelling that maps to the same path key).
    pub fn id_for_path(&self, path: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM tracks WHERE path_key = ?1",
                [path_key(path)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// How many of `ids` are known tracks (duplicates count once).
    pub fn count_existing(&self, ids: &[i64]) -> Result<u32> {
        let json = format!(
            "[{}]",
            ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
        );
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM tracks WHERE id IN (SELECT value FROM json_each(?1))",
            [json],
            |r| r.get(0),
        )?)
    }

    /// Music folders to scan, in the order they were added.
    pub fn roots(&self) -> Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT path FROM roots ORDER BY id")?;
        let rows = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Adds a music folder. False if it (or a folder containing it) is already there.
    pub fn add_root(&self, path: &str) -> Result<bool> {
        let trim = |p: &str| path_key(p.trim_end_matches(['/', '\\']));
        let key = trim(path);
        // A folder taken out (or one holding such) comes back on the next scan.
        let back = self.conn.execute(
            "DELETE FROM excluded WHERE path_key = ?1 OR substr(path_key, 1, length(?1) + 1) = ?1 || ?2",
            params![key, std::path::MAIN_SEPARATOR.to_string()],
        )?;
        // ponytail: a new folder that contains old ones keeps both (walked twice, deduped by path)
        let inside = self.roots()?.iter().any(|r| {
            let r = trim(r);
            key == r || key.starts_with(&format!("{r}{}", std::path::MAIN_SEPARATOR))
        });
        if inside {
            return Ok(back > 0);
        }
        self.conn.execute(
            "INSERT INTO roots (path_key, path) VALUES (?1, ?2)",
            params![key, path],
        )?;
        Ok(true)
    }

    /// Takes a folder out of the library: a music folder is forgotten, a folder inside one is
    /// skipped from now on. Its tracks become like opened files (hidden; ones with plays, ratings
    /// or in a playlist stay for the stats, the rest go at the next start). The files stay on disk.
    pub fn remove_folder(&self, path: &str) -> Result<()> {
        let key = path_key(path.trim_end_matches(['/', '\\']));
        let sep = std::path::MAIN_SEPARATOR.to_string();
        let roots: Vec<String> = self.roots()?.iter().map(|r| path_key(r.trim_end_matches(['/', '\\']))).collect();
        let tx = self.conn.unchecked_transaction()?;
        let dropped = tx.execute(
            "DELETE FROM roots WHERE path_key = ?1 OR substr(path_key, 1, length(?1) + 1) = ?1 || ?2",
            params![key, sep],
        )?;
        // Still inside another music folder: skipped there from now on.
        if roots.iter().any(|r| key.starts_with(&format!("{r}{sep}"))) {
            tx.execute("INSERT OR IGNORE INTO excluded (path_key) VALUES (?1)", [&key])?;
        } else if dropped == 0 {
            return Err(Error::NotFound);
        }
        tx.execute(
            "UPDATE tracks SET in_library = 0 WHERE substr(path_key, 1, length(?1) + 1) = ?1 || ?2",
            params![key, sep],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Folders skipped inside the music folders (path keys).
    pub(crate) fn excluded(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached("SELECT path_key FROM excluded")?;
        let rows = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Forgets opened files (outside the music folders) nothing refers to: never played, unrated,
    /// in no playlist. Ones with plays stay (hidden) so Estadísticas / Wrapped keep them. Call it
    /// before anything is queued (startup): the queue is not saved, so no queued track is lost.
    pub fn prune_external(&self) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM tracks WHERE in_library = 0 AND play_count = 0 AND rating = 0 \
             AND id NOT IN (SELECT track_id FROM plays) AND id NOT IN (SELECT track_id FROM playlist_items)",
            [],
        )?;
        if n > 0 {
            self.conn.execute("DELETE FROM artists WHERE id NOT IN (SELECT artist_id FROM track_artists)", [])?;
        }
        Ok(n)
    }

    pub fn albums(&self) -> Result<Vec<Album>> {
        let mut st = self.conn.prepare_cached(
            "SELECT album_key, MAX(album), \
                CASE WHEN MAX(album_artist) <> '' THEN MAX(album_artist) \
                     WHEN COUNT(DISTINCT artist) = 1 THEN MAX(artist) END AS who, \
                MAX(year), COUNT(*), SUM(duration_ms), MAX(cover) \
             FROM tracks WHERE available = 1 AND in_library = 1 GROUP BY album_key \
             ORDER BY who COLLATE NOCASE, 2 COLLATE NOCASE",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(Album {
                    key: r.get(0)?,
                    title: r.get(1)?,
                    artist: r.get(2)?,
                    year: r.get(3)?,
                    track_count: r.get(4)?,
                    duration_ms: r.get(5)?,
                    cover: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn artists(&self) -> Result<Vec<Artist>> {
        let mut st = self.conn.prepare_cached(
            "SELECT a.id, a.name, COUNT(DISTINCT ta.track_id) FROM artists a \
             JOIN track_artists ta ON ta.artist_id = a.id \
             JOIN tracks t ON t.id = ta.track_id AND t.available = 1 AND t.in_library = 1 \
             GROUP BY a.id ORDER BY a.name COLLATE NOCASE",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(Artist {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    track_count: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Split artists of one track in tag order, album artists last.
    pub fn artists_of(&self, track_id: i64) -> Result<Vec<TrackArtist>> {
        let mut st = self.conn.prepare_cached(
            "SELECT a.id, a.name, ta.role FROM track_artists ta JOIN artists a ON a.id = ta.artist_id \
             WHERE ta.track_id = ?1 ORDER BY ta.role = 'album', ta.position",
        )?;
        let rows = st
            .query_map([track_id], |r| {
                Ok(TrackArtist {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    role: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Folder tree for the folder view. Chains of empty single-child folders are merged
    /// ("F:\Music\Rock" as one node), like compact folders in an editor.
    pub fn folders(&self) -> Result<Vec<FolderNode>> {
        let mut st = self.conn.prepare_cached(
            "SELECT folder, COUNT(*) FROM tracks WHERE available = 1 AND in_library = 1 GROUP BY folder",
        )?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(folder_tree(rows, &self.roots()?))
    }

    pub fn record_play(&mut self, track_id: i64, ms_listened: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let t = now();
        tx.execute(
            "INSERT INTO plays (track_id, played_at, ms_listened) VALUES (?1, ?2, ?3)",
            params![track_id, t, ms_listened.max(0)],
        )?;
        tx.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played_at = ?2 WHERE id = ?1",
            params![track_id, t],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn record_skip(&self, track_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET skip_count = skip_count + 1 WHERE id = ?1",
            [track_id],
        )?;
        Ok(())
    }

    /// 0 = no rating, 1-5 stars.
    pub fn set_rating(&self, track_id: i64, rating: u8) -> Result<()> {
        if rating > 5 {
            return Err(Error::Invalid("rating must be 0-5"));
        }
        self.conn.execute(
            "UPDATE tracks SET rating = ?2 WHERE id = ?1",
            params![track_id, rating],
        )?;
        Ok(())
    }

    pub fn set_quality(&self, track_id: i64, verdict: &str, json: &str, key: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET quality_verdict = ?2, quality_json = ?3, quality_key = ?4 WHERE id = ?1",
            params![track_id, verdict, json, key],
        )?;
        Ok(())
    }

    /// Tracks never analysed or changed on disk since their verdict.
    pub fn tracks_needing_quality(&self, limit: u32) -> Result<Vec<QualityJob>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, path, size || ':' || mtime AS k FROM tracks \
             WHERE available = 1 AND in_library = 1 AND quality_key IS NOT (size || ':' || mtime) ORDER BY id LIMIT ?1",
        )?;
        let rows = st
            .query_map([limit], |r| {
                Ok(QualityJob {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    key: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// `Some` if this track needs (re)analysis, like `tracks_needing_quality` for one id.
    pub fn quality_job(&self, id: i64) -> Result<Option<QualityJob>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, path, size || ':' || mtime FROM tracks                  WHERE id = ?1 AND available = 1 AND quality_key IS NOT (size || ':' || mtime)",
                [id],
                |r| {
                    Ok(QualityJob {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        key: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Waveform of the track's current file: `None` = missing or stale, empty = unreadable.
    pub fn waveform(&self, track_id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT w.peaks FROM tracks t JOIN track_waveforms w ON w.track_id = t.id                  WHERE t.id = ?1 AND w.wave_key = (?2 || ':' || t.size || ':' || t.mtime)",
                params![track_id, WAVEFORM_REV],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// `Some` if this track needs a (new) waveform. Pass `key` back to `set_waveform`.
    pub fn waveform_job(&self, id: i64) -> Result<Option<QualityJob>> {
        Ok(self.waveform_jobs("t.id = ?2", id)?.pop())
    }

    /// Tracks without a waveform for their current file, most played first.
    pub fn tracks_needing_waveform(&self, limit: u32) -> Result<Vec<QualityJob>> {
        self.waveform_jobs("t.in_library = 1 ORDER BY t.play_count DESC, t.id LIMIT ?2", limit)
    }

    fn waveform_jobs(&self, tail: &str, arg: impl rusqlite::ToSql) -> Result<Vec<QualityJob>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT t.id, t.path, ?1 || ':' || t.size || ':' || t.mtime FROM tracks t              LEFT JOIN track_waveforms w ON w.track_id = t.id              WHERE t.available = 1 AND w.wave_key IS NOT (?1 || ':' || t.size || ':' || t.mtime) AND {tail}"
        ))?;
        let rows = st
            .query_map(params![WAVEFORM_REV, arg], |r| Ok(QualityJob { id: r.get(0)?, path: r.get(1)?, key: r.get(2)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn set_waveform(&self, track_id: i64, key: &str, peaks: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO track_waveforms (track_id, wave_key, peaks) VALUES (?1, ?2, ?3)              ON CONFLICT(track_id) DO UPDATE SET wave_key = excluded.wave_key, peaks = excluded.peaks",
            params![track_id, key, peaks],
        )?;
        Ok(())
    }

    /// Smart playlists need valid rules; manual ones take none.
    pub fn create_playlist(
        &self,
        name: &str,
        kind: PlaylistKind,
        rules_json: Option<&str>,
    ) -> Result<i64> {
        let rules = match (kind, rules_json) {
            (PlaylistKind::Smart, Some(j)) => Some(rules_to_json(&smart::parse(j)?)),
            (PlaylistKind::Manual, None) => None,
            _ => return Err(Error::Invalid("smart playlists need rules, manual ones none")),
        };
        self.conn.execute(
            "INSERT INTO playlists (name, kind, rules_json, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![check_name(name)?, kind.as_str(), rules, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_playlist(&self, id: i64, name: &str, rules_json: Option<&str>) -> Result<()> {
        let rules = rules_json.map(smart::parse).transpose()?.map(|r| rules_to_json(&r));
        self.conn.execute(
            "UPDATE playlists SET name = ?2, rules_json = ?3 WHERE id = ?1",
            params![id, check_name(name)?, rules],
        )?;
        Ok(())
    }

    pub fn rename_playlist(&self, id: i64, name: &str) -> Result<()> {
        self.playlist_row(id)?;
        self.conn.execute(
            "UPDATE playlists SET name = ?2 WHERE id = ?1",
            params![id, check_name(name)?],
        )?;
        Ok(())
    }

    pub fn set_smart_rules(&self, id: i64, rules: &SmartRules) -> Result<()> {
        if self.playlist_row(id)?.kind != PlaylistKind::Smart {
            return Err(Error::Invalid("not a smart playlist"));
        }
        rules.compile(now())?;
        self.conn.execute(
            "UPDATE playlists SET rules_json = ?2 WHERE id = ?1",
            params![id, rules_to_json(rules)],
        )?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Rows with the stored item count (0 for smart playlists).
    fn playlist_rows(&self, id: Option<i64>) -> Result<Vec<Playlist>> {
        let mut st = self.conn.prepare_cached(
            "SELECT p.id, p.name, p.kind, p.rules_json, p.created_at, \
                (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id) \
             FROM playlists p WHERE ?1 IS NULL OR p.id = ?1 ORDER BY p.name COLLATE NOCASE, p.id",
        )?;
        let rows = st
            .query_map([id], |r| {
                let kind: String = r.get(2)?;
                Ok(Playlist {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: if kind == "smart" {
                        PlaylistKind::Smart
                    } else {
                        PlaylistKind::Manual
                    },
                    rules_json: r.get(3)?,
                    created_at: r.get(4)?,
                    track_count: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    fn playlist_row(&self, id: i64) -> Result<Playlist> {
        self.playlist_rows(Some(id))?.pop().ok_or(Error::NotFound)
    }

    fn manual(&self, id: i64) -> Result<()> {
        match self.playlist_row(id)?.kind {
            PlaylistKind::Manual => Ok(()),
            PlaylistKind::Smart => Err(Error::Invalid("smart playlists have no editable contents")),
        }
    }

    /// Every playlist by name; smart ones count what their rules match right now.
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut rows = self.playlist_rows(None)?;
        for p in rows.iter_mut().filter(|p| p.kind == PlaylistKind::Smart) {
            p.track_count = self.playlist_ids(p.id).map_or(0, |ids| ids.len() as u32);
        }
        Ok(rows)
    }

    pub fn playlist(&self, id: i64) -> Result<Playlist> {
        let mut p = self.playlist_row(id)?;
        if p.kind == PlaylistKind::Smart {
            p.track_count = self.playlist_ids(id)?.len() as u32;
        }
        Ok(p)
    }

    /// Track ids in order: the items of a manual playlist, the matches of a smart one.
    pub fn playlist_ids(&self, id: i64) -> Result<Vec<i64>> {
        let p = self.playlist_row(id)?;
        if p.kind == PlaylistKind::Smart {
            return self.smart_ids(&smart::parse(p.rules_json.as_deref().unwrap_or(""))?);
        }
        let mut st = self.conn.prepare_cached(
            "SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
        )?;
        let ids = st
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Playlist contents in order (unavailable tracks of manual lists included, the UI greys them out).
    pub fn playlist_tracks(&self, id: i64) -> Result<Vec<Track>> {
        self.tracks_by_ids(&self.playlist_ids(id)?)
    }

    /// Replace the whole contents of a manual playlist.
    pub fn set_playlist_tracks(&mut self, id: i64, track_ids: &[i64]) -> Result<()> {
        self.manual(id)?;
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO playlist_items (playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
            )?;
            for (pos, tid) in track_ids.iter().enumerate() {
                ins.execute(params![id, pos as i64, tid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn add_to_playlist(&mut self, id: i64, track_ids: &[i64]) -> Result<()> {
        self.manual(id)?;
        let tx = self.conn.transaction()?;
        let next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_items WHERE playlist_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO playlist_items (playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
            )?;
            for (i, tid) in track_ids.iter().enumerate() {
                ins.execute(params![id, next + i as i64, tid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes the items at `positions` (0-based, any order) of a manual playlist.
    pub fn remove_from_playlist(&mut self, id: i64, positions: &[usize]) -> Result<()> {
        self.manual(id)?;
        let ids = self.playlist_ids(id)?;
        if positions.iter().any(|&p| p >= ids.len()) {
            return Err(Error::Invalid("position out of range"));
        }
        let gone: std::collections::HashSet<usize> = positions.iter().copied().collect();
        let keep: Vec<i64> = ids
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !gone.contains(i))
            .map(|(_, t)| t)
            .collect();
        self.set_playlist_tracks(id, &keep)
    }

    /// Moves the item at `from` so it ends up at `to` (positions of the list as it is now).
    pub fn move_in_playlist(&mut self, id: i64, from: usize, to: usize) -> Result<()> {
        self.manual(id)?;
        let mut ids = self.playlist_ids(id)?;
        if from >= ids.len() || to >= ids.len() {
            return Err(Error::Invalid("position out of range"));
        }
        let t = ids.remove(from);
        ids.insert(to, t);
        self.set_playlist_tracks(id, &ids)
    }

    pub fn resume_position(&self, track_id: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT position_ms FROM resume_positions WHERE track_id = ?1",
                [track_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// `position_ms <= 0` forgets the position (track finished).
    pub fn set_resume_position(&self, track_id: i64, position_ms: i64) -> Result<()> {
        if position_ms <= 0 {
            self.conn
                .execute("DELETE FROM resume_positions WHERE track_id = ?1", [track_id])?;
        } else {
            self.conn.execute(
                "INSERT INTO resume_positions (track_id, position_ms, updated_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(track_id) DO UPDATE SET position_ms = excluded.position_ms, updated_at = excluded.updated_at",
                params![track_id, position_ms, now()],
            )?;
        }
        Ok(())
    }
}

/// User text -> FTS5 prefix query. Only alphanumeric tokens reach FTS, quoted: user text can
/// never form FTS syntax. `None` when there is nothing to search.
fn fts_query(q: &str) -> Option<String> {
    let toks: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!toks.is_empty()).then(|| toks.join(" "))
}

/// Each music folder (root) is one top node with its subfolders below; folders outside every
/// root (a scan of a folder that is not a root) hang from their drive.
fn folder_tree(rows: Vec<(String, u32)>, roots: &[String]) -> Vec<FolderNode> {
    #[derive(Default)]
    struct N {
        path: String,
        count: u32,
        kids: BTreeMap<String, N>,
        root: bool,
    }
    let roots: Vec<&str> = roots.iter().map(|r| r.trim_end_matches(['/', '\\'])).collect();
    let mut top = N::default();
    for (folder, count) in rows {
        let under = |r: &str| {
            folder.get(..r.len()).is_some_and(|p| path_key(p) == path_key(r))
                && folder[r.len()..].chars().next().is_none_or(|c| c == '/' || c == '\\')
        };
        let (mut node, mut end) = match roots.iter().copied().filter(|r| under(r)).max_by_key(|r| r.len()) {
            Some(r) => {
                let n = top.kids.entry(r.to_string()).or_default();
                n.path = r.to_string();
                n.root = true;
                (n, r.len())
            }
            None => (&mut top, 0),
        };
        for part in folder[end..].split(['/', '\\']) {
            end += part.len();
            if !part.is_empty() {
                node = node.kids.entry(part.to_string()).or_default();
                node.path = folder[..end].to_string();
            }
            end += 1;
        }
        node.path = folder;
        node.count += count;
    }
    fn convert(name: String, n: N) -> FolderNode {
        let mut children: Vec<FolderNode> =
            n.kids.into_iter().map(|(k, v)| convert(k, v)).collect();
        // A music folder stays its own node even when it only holds one subfolder.
        if n.count == 0 && children.len() == 1 && !n.root {
            let mut only = children.remove(0);
            only.name = format!("{name}{}{}", std::path::MAIN_SEPARATOR, only.name);
            return only;
        }
        FolderNode {
            name,
            path: n.path,
            track_count: n.count,
            total_count: n.count + children.iter().map(|c| c.total_count).sum::<u32>(),
            children,
        }
    }
    top.kids.into_iter().map(|(k, v)| convert(k, v)).collect()
}

#[cfg(test)]
mod tests;
