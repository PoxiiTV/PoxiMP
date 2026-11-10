//! Migration in and out: AIMP playlists (.aimppl4), iTunes/Music `Library.xml`, M3U/M3U8, and
//! our own JSON backup of stats and ratings. Files are matched to library tracks by path
//! (normalised: case, separators, NFC/NFD), falling back to title + artist + duration ±2 s.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::{now, path_key, Error, Library, Result};

const MAX_FILE: u64 = 512 << 20;
const MAX_UNMATCHED_LISTED: usize = 50;
const DURATION_SLACK_MS: i64 = 2_000;
const ITUNES: &str = "itunes";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Entries found in the library.
    pub matched: u32,
    pub unmatched: u32,
    /// Playlists created.
    pub playlists: Vec<i64>,
    /// The first unmatched entries (path, or "artist - title"), for the UI to list.
    pub unmatched_items: Vec<String>,
}

impl ImportReport {
    fn hit(&mut self, found: Option<i64>, label: impl FnOnce() -> String) -> Option<i64> {
        if found.is_some() {
            self.matched += 1;
        } else {
            self.unmatched += 1;
            if self.unmatched_items.len() < MAX_UNMATCHED_LISTED {
                self.unmatched_items.push(label());
            }
        }
        found
    }
}

fn norm(s: &str) -> String {
    s.trim().nfc().collect::<String>().to_lowercase()
}

/// Every library track by path key and by (title, artist).
struct Matcher {
    by_key: HashMap<String, i64>,
    by_meta: HashMap<(String, String), Vec<(i64, i64)>>,
}

impl Matcher {
    fn new(conn: &Connection) -> Result<Self> {
        let mut m = Matcher { by_key: HashMap::new(), by_meta: HashMap::new() };
        let mut st = conn.prepare("SELECT id, path_key, title, artist, duration_ms FROM tracks WHERE in_library = 1")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get(4)?))
        })?;
        for row in rows {
            let (id, key, title, artist, ms) = row?;
            m.by_key.insert(key, id);
            m.by_meta.entry((norm(&title), norm(&artist))).or_default().push((id, ms));
        }
        Ok(m)
    }

    fn find(&self, path: Option<&str>, title: &str, artist: &str, duration_ms: Option<i64>) -> Option<i64> {
        if let Some(p) = path.filter(|p| !p.is_empty()) {
            // A list written on macOS may carry NFD names, one from Windows NFC: try both.
            let nfc: String = p.nfc().collect();
            let nfd: String = p.nfd().collect();
            for k in [path_key(p), path_key(&nfc), path_key(&nfd)] {
                if let Some(&id) = self.by_key.get(&k) {
                    return Some(id);
                }
            }
        }
        let ms = duration_ms?;
        if title.trim().is_empty() {
            return None;
        }
        self.by_meta
            .get(&(norm(title), norm(artist)))?
            .iter()
            .map(|&(id, d)| (id, (d - ms).abs()))
            .filter(|&(_, diff)| diff <= DURATION_SLACK_MS)
            .min_by_key(|&(_, diff)| diff)
            .map(|(id, _)| id)
    }
}

fn read_capped(path: &Path) -> Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_FILE {
        return Err(Error::Invalid("file too large"));
    }
    Ok(fs::read(path)?)
}

/// Text in UTF-8 or UTF-16 (BOM); invalid UTF-8 is read as Latin-1 (old .m3u files).
fn decode(b: &[u8]) -> String {
    let utf16 = |be: bool| {
        let units: Vec<u16> = b[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| if be { u16::from_be_bytes(c) } else { u16::from_le_bytes(c) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match b {
        [0xFF, 0xFE, ..] => utf16(false),
        [0xFE, 0xFF, ..] => utf16(true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => match std::str::from_utf8(b) {
            Ok(s) => s.to_string(),
            Err(_) => b.iter().map(|&c| c as char).collect(),
        },
    }
}

/// "file://localhost/C:/a%20b.mp3", "file:///Users/x/a.mp3" -> a local path.
fn file_url_to_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://").or_else(|| url.strip_prefix("FILE://"))?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let p = String::from_utf8(out).ok()?;
    let p = if !p.starts_with('/') {
        // file://server/share/x: a UNC path.
        format!("//{p}")
    } else if p.len() > 2 && p.as_bytes()[2] == b':' {
        // /C:/x -> C:/x
        p[1..].to_string()
    } else {
        p
    };
    Some(if cfg!(windows) { p.replace('/', "\\") } else { p })
}

/// #EXTINF of an M3U entry: duration (ms), artist, title.
type ExtInf = (Option<i64>, String, String);

/// Local files an M3U / M3U8 lists, in order, with their #EXTINF. Relative
/// paths resolve against the list's folder; stream URLs are skipped. Files may not exist.
fn m3u_entries(path: &Path) -> Result<Vec<(String, ExtInf)>> {
    let text = decode(&read_capped(path)?);
    let base = path.parent().unwrap_or(Path::new(""));
    let mut out = Vec::new();
    let mut info: Option<ExtInf> = None;
    for line in text.lines().map(str::trim) {
        if let Some(i) = line.strip_prefix("#EXTINF:") {
            let (secs, label) = i.split_once(',').unwrap_or((i, ""));
            // "123 tvg-id=..." -> 123
            let secs = secs.split_whitespace().next().and_then(|s| s.parse::<f64>().ok()).filter(|s| *s > 0.0);
            let (artist, title) = label.split_once(" - ").unwrap_or(("", label));
            info = Some((secs.map(|s| (s * 1000.0) as i64), artist.to_string(), title.to_string()));
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let meta = info.take().unwrap_or_default();
        let file = if line.contains("://") {
            file_url_to_path(line)
        } else {
            let p = Path::new(line);
            Some(lexical(&if p.is_absolute() { p.to_path_buf() } else { base.join(p) }).to_string_lossy().into_owned())
        };
        // None = a stream URL, not a file
        out.extend(file.map(|f| (f, meta)));
    }
    Ok(out)
}

/// The files an M3U / M3U8 lists, in order ("open with" a playlist plays them; nothing imported).
pub fn m3u_files(path: &Path) -> Result<Vec<PathBuf>> {
    Ok(m3u_entries(path)?.into_iter().map(|(f, _)| PathBuf::from(f)).collect())
}

/// Resolves "." and ".." without touching the disk (the file may live on an unplugged drive).
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// `target` relative to the directory `base`, or None when they share no root (other drive).
fn relative(base: &Path, target: &Path) -> Option<PathBuf> {
    let key = |c: &Component| path_key(&c.as_os_str().to_string_lossy());
    let b: Vec<_> = base.components().collect();
    let t: Vec<_> = target.components().collect();
    if b.is_empty() || t.is_empty() || key(&b[0]) != key(&t[0]) {
        return None;
    }
    let n = b.iter().zip(&t).take_while(|(x, y)| key(x) == key(y)).count();
    let mut out = PathBuf::new();
    for _ in n..b.len() {
        out.push("..");
    }
    for c in &t[n..] {
        out.push(c);
    }
    Some(out)
}

fn playlist_name(s: &str) -> String {
    let s: String = s.trim().chars().take(200).collect();
    if s.trim().is_empty() {
        "Imported".into()
    } else {
        s
    }
}

fn new_playlist(conn: &Connection, name: &str, ids: &[i64]) -> Result<i64> {
    conn.execute(
        "INSERT INTO playlists (name, kind, rules_json, created_at) VALUES (?1, 'manual', NULL, ?2)",
        params![playlist_name(name), now()],
    )?;
    let id = conn.last_insert_rowid();
    let mut ins = conn.prepare_cached("INSERT INTO playlist_items (playlist_id, position, track_id) VALUES (?1, ?2, ?3)")?;
    for (pos, tid) in ids.iter().enumerate() {
        ins.execute(params![id, pos as i64, tid])?;
    }
    Ok(id)
}

/// "YYYY-MM-DDTHH:MM:SSZ" (plist dates are UTC) -> unix seconds.
fn parse_utc(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

// --- a minimal XML plist reader (only what Library.xml uses) ---

#[derive(Debug, Clone, PartialEq)]
enum Pl {
    Dict(Vec<(String, Pl)>),
    Array(Vec<Pl>),
    Str(String),
    Int(i64),
    Bool(bool),
}

impl Pl {
    fn get(&self, key: &str) -> Option<&Pl> {
        match self {
            Pl::Dict(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Pl::Str(s) => Some(s),
            _ => None,
        }
    }
    fn int(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            Pl::Int(n) => Some(*n),
            _ => None,
        }
    }
    fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Pl::Bool(true)))
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| ent.strip_prefix('#').map(str::parse::<u32>))
                .and_then(|r| r.ok())
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

struct Xml<'a> {
    s: &'a str,
    i: usize,
}

const BAD_PLIST: Error = Error::Invalid("malformed plist");

impl<'a> Xml<'a> {
    /// Next element tag: (name, closing, self-closing). Skips text, comments, <?..?> and <!..>.
    fn tag(&mut self) -> Result<(&'a str, bool, bool)> {
        loop {
            let start = self.i + self.s[self.i..].find('<').ok_or(BAD_PLIST)?;
            let rest = &self.s[start..];
            let skip_to = |pat: &str| rest.find(pat).map(|e| start + e + pat.len()).ok_or(BAD_PLIST);
            if rest.starts_with("<!--") {
                self.i = skip_to("-->")?;
            } else if rest.starts_with("<?") {
                self.i = skip_to("?>")?;
            } else if rest.starts_with("<!") {
                self.i = skip_to(">")?;
            } else {
                let end = skip_to(">")?;
                let inner = &self.s[start + 1..end - 1];
                self.i = end;
                let closing = inner.starts_with('/');
                let selfc = inner.ends_with('/');
                let name = inner.trim_matches('/').split_whitespace().next().unwrap_or("");
                return Ok((name, closing, selfc));
            }
        }
    }

    /// Text up to `</name>` (entities decoded).
    fn text(&mut self, name: &str) -> Result<String> {
        let close = format!("</{name}>");
        let end = self.s[self.i..].find(&close).ok_or(BAD_PLIST)?;
        let t = unescape(&self.s[self.i..self.i + end]);
        self.i += end + close.len();
        Ok(t)
    }

    fn value(&mut self, name: &str, selfc: bool, depth: usize) -> Result<Pl> {
        if depth > 32 {
            return Err(BAD_PLIST);
        }
        Ok(match name {
            "dict" if selfc => Pl::Dict(Vec::new()),
            "dict" => {
                let mut kv = Vec::new();
                loop {
                    match self.tag()? {
                        ("dict", true, _) => break Pl::Dict(kv),
                        ("key", false, false) => {
                            let k = self.text("key")?;
                            let (n, closing, s) = self.tag()?;
                            if closing {
                                return Err(BAD_PLIST);
                            }
                            kv.push((k, self.value(n, s, depth + 1)?));
                        }
                        _ => return Err(BAD_PLIST),
                    }
                }
            }
            "array" if selfc => Pl::Array(Vec::new()),
            "array" => {
                let mut v = Vec::new();
                loop {
                    match self.tag()? {
                        ("array", true, _) => break Pl::Array(v),
                        (_, true, _) => return Err(BAD_PLIST),
                        (n, false, s) => v.push(self.value(n, s, depth + 1)?),
                    }
                }
            }
            "true" | "false" if selfc => Pl::Bool(name == "true"),
            "plist" => {
                let (n, closing, s) = self.tag()?;
                if closing {
                    return Err(BAD_PLIST);
                }
                self.value(n, s, depth + 1)?
            }
            // Self-closing scalars: `<string/>` is "", `<integer/>` is 0.
            "integer" if selfc => Pl::Int(0),
            _ if selfc => Pl::Str(String::new()),
            "integer" => Pl::Int(self.text("integer")?.trim().parse().map_err(|_| BAD_PLIST)?),
            "string" | "date" | "data" | "real" => Pl::Str(self.text(name)?),
            _ => return Err(BAD_PLIST),
        })
    }
}

fn parse_plist(s: &str) -> Result<Pl> {
    let mut x = Xml { s, i: 0 };
    let (n, closing, selfc) = x.tag()?;
    if closing {
        return Err(BAD_PLIST);
    }
    x.value(n, selfc, 0)
}

// --- backup format ---

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Backup {
    app: String,
    version: u32,
    exported_at: i64,
    tracks: Vec<BackupTrack>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupTrack {
    path: String,
    title: String,
    artist: String,
    album: String,
    duration_ms: i64,
    play_count: u32,
    skip_count: u32,
    rating: u8,
    last_played_at: Option<i64>,
    added_at: i64,
    /// (played_at, ms_listened)
    plays: Vec<(i64, i64)>,
}

impl Library {
    /// AIMP 4/5 playlist: UTF-16 text, `#-----CONTENT-----#` then group lines ("-<folder>") and
    /// track lines "path|title|artist|album|...|duration ms (field 14)|...".
    pub fn import_aimp_playlist(&mut self, path: &Path) -> Result<ImportReport> {
        let text = decode(&read_capped(path)?);
        let tx = self.conn.transaction()?;
        let m = Matcher::new(&tx)?;
        let mut rep = ImportReport::default();
        let mut name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut section = "";
        let mut ids = Vec::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.starts_with("#-----") {
                section = line.trim_matches(['#', '-']);
                continue;
            }
            match section {
                "SUMMARY" => {
                    if let Some(n) = line.strip_prefix("Name=").filter(|n| !n.trim().is_empty()) {
                        name = n.to_string();
                    }
                }
                "CONTENT" if !line.is_empty() && !line.starts_with('-') => {
                    let f: Vec<&str> = line.split('|').collect();
                    let file = f[0];
                    let (title, artist) = (f.get(1).copied().unwrap_or(""), f.get(2).copied().unwrap_or(""));
                    let dur = f.get(14).and_then(|d| d.parse().ok());
                    let found = m.find(Some(file), title, artist, dur);
                    ids.extend(rep.hit(found, || file.to_string()));
                }
                _ => {}
            }
        }
        rep.playlists.push(new_playlist(&tx, &name, &ids)?);
        tx.commit()?;
        Ok(rep)
    }

    /// M3U / M3U8: relative paths resolve against the list's folder; #EXTINF gives the fallback
    /// metadata ("#EXTINF:seconds,Artist - Title").
    pub fn import_m3u(&mut self, path: &Path) -> Result<ImportReport> {
        let entries = m3u_entries(path)?;
        let tx = self.conn.transaction()?;
        let m = Matcher::new(&tx)?;
        let mut rep = ImportReport::default();
        let mut ids = Vec::new();
        for (file, (dur, artist, title)) in entries {
            let found = m.find(Some(&file), &title, &artist, dur);
            ids.extend(rep.hit(found, || file.clone()));
        }
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        rep.playlists.push(new_playlist(&tx, &name, &ids)?);
        tx.commit()?;
        Ok(rep)
    }

    /// UTF-8 M3U8 with #EXTINF; paths relative to the list's folder when on the same drive.
    pub fn export_m3u(&self, playlist_id: i64, path: &Path) -> Result<u32> {
        let ids = self.playlist_ids(playlist_id)?;
        let base = path.parent().unwrap_or(Path::new(""));
        let mut out = String::from("#EXTM3U\n");
        for t in self.tracks_by_ids(&ids)? {
            let file = Path::new(&t.path);
            let shown = relative(base, file).unwrap_or_else(|| file.to_path_buf());
            let label = if t.artist.is_empty() { t.title.clone() } else { format!("{} - {}", t.artist, t.title) };
            out.push_str(&format!("#EXTINF:{},{}\n{}\n", t.duration_ms / 1000, label.replace('\n', " "), shown.display()));
        }
        fs::write(path, out)?;
        Ok(ids.len() as u32)
    }

    /// iTunes / Music "Library.xml": play and skip counts, ratings (0-100 -> 0-5 stars), date
    /// added and last played land on the matched tracks; playlists become manual playlists.
    /// Re-importing is safe: counts are tracked per source and only the difference is applied.
    pub fn import_itunes_xml(&mut self, path: &Path) -> Result<ImportReport> {
        let bytes = read_capped(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| BAD_PLIST)?;
        // ponytail: whole tree in memory (~3x the file); stream it if 500 MB libraries show up.
        let root = parse_plist(text)?;
        let tx = self.conn.transaction()?;
        let m = Matcher::new(&tx)?;
        let mut rep = ImportReport::default();
        let mut map: HashMap<i64, i64> = HashMap::new();
        if let Some(Pl::Dict(tracks)) = root.get("Tracks") {
            for (_, t) in tracks {
                // Streams and cloud-only tracks have no file to match.
                let Some(file) = t.str("Location").and_then(file_url_to_path) else { continue };
                let (title, artist) = (t.str("Name").unwrap_or(""), t.str("Artist").unwrap_or(""));
                let found = m.find(Some(&file), title, artist, t.int("Total Time"));
                let Some(id) = rep.hit(found, || file.clone()) else { continue };
                if let Some(tid) = t.int("Track ID") {
                    map.insert(tid, id);
                }
                let rating = if t.flag("Rating Computed") { 0 } else { t.int("Rating").unwrap_or(0).clamp(0, 100) };
                apply_imported(
                    &tx,
                    id,
                    ITUNES,
                    t.int("Play Count").unwrap_or(0).max(0),
                    t.int("Skip Count").unwrap_or(0).max(0),
                    (rating + 10) / 20,
                    t.str("Date Added").and_then(parse_utc),
                    t.str("Play Date UTC").and_then(parse_utc),
                )?;
            }
        }
        if let Some(Pl::Array(lists)) = root.get("Playlists") {
            for pl in lists {
                let skip = pl.flag("Master")
                    || pl.flag("Folder")
                    || pl.get("Distinguished Kind").is_some()
                    || matches!(pl.get("Visible"), Some(Pl::Bool(false)));
                let name = playlist_name(pl.str("Name").unwrap_or(""));
                if skip {
                    continue;
                }
                let ids: Vec<i64> = match pl.get("Playlist Items") {
                    Some(Pl::Array(items)) => items.iter().filter_map(|i| map.get(&i.int("Track ID")?).copied()).collect(),
                    _ => Vec::new(),
                };
                // Re-import: an existing playlist of that name is left alone.
                let exists = tx
                    .query_row("SELECT 1 FROM playlists WHERE name = ?1 COLLATE NOCASE", [&name], |_| Ok(()))
                    .optional()?
                    .is_some();
                if !ids.is_empty() && !exists {
                    rep.playlists.push(new_playlist(&tx, &name, &ids)?);
                }
            }
        }
        tx.commit()?;
        Ok(rep)
    }

    /// Stats and ratings of every track that has any, with its dated plays. Returns the track count.
    pub fn export_stats_backup(&self, path: &Path) -> Result<u32> {
        let mut st = self.conn.prepare(
            "SELECT id, path, title, artist, album, duration_ms, play_count, skip_count, rating, last_played_at, added_at \
             FROM tracks WHERE play_count > 0 OR skip_count > 0 OR rating > 0 OR id IN (SELECT track_id FROM plays) ORDER BY id",
        )?;
        let mut plays = self.conn.prepare("SELECT played_at, ms_listened FROM plays WHERE track_id = ?1 ORDER BY played_at")?;
        let mut tracks = Vec::new();
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    BackupTrack {
                        path: r.get(1)?,
                        title: r.get(2)?,
                        artist: r.get(3)?,
                        album: r.get(4)?,
                        duration_ms: r.get(5)?,
                        play_count: r.get(6)?,
                        skip_count: r.get(7)?,
                        rating: r.get(8)?,
                        last_played_at: r.get(9)?,
                        added_at: r.get(10)?,
                        plays: Vec::new(),
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, mut t) in rows {
            t.plays = plays.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            tracks.push(t);
        }
        let n = tracks.len() as u32;
        let b = Backup { app: "PoxiMP".into(), version: 1, exported_at: now(), tracks };
        fs::write(path, serde_json::to_vec_pretty(&b).map_err(|_| Error::Invalid("backup not serialisable"))?)?;
        Ok(n)
    }

    /// Restores a backup: counts take the higher value, plays are added once (same track and
    /// time = same play), ratings only fill unrated tracks. Safe to run twice.
    pub fn import_stats_backup(&mut self, path: &Path) -> Result<ImportReport> {
        let b: Backup = serde_json::from_slice(&read_capped(path)?).map_err(|_| Error::Invalid("not a PoxiMP backup"))?;
        if b.app != "PoxiMP" || b.version != 1 {
            return Err(Error::Invalid("not a PoxiMP backup"));
        }
        let tx = self.conn.transaction()?;
        let m = Matcher::new(&tx)?;
        let mut rep = ImportReport::default();
        for t in &b.tracks {
            let found = m.find(Some(&t.path), &t.title, &t.artist, Some(t.duration_ms));
            let Some(id) = rep.hit(found, || t.path.clone()) else { continue };
            tx.prepare_cached(
                "UPDATE tracks SET play_count = MAX(play_count, ?2), skip_count = MAX(skip_count, ?3), \
                    rating = CASE WHEN rating = 0 THEN ?4 ELSE rating END, added_at = MIN(added_at, ?5), \
                    last_played_at = CASE WHEN ?6 IS NULL THEN last_played_at \
                        WHEN last_played_at IS NULL OR last_played_at < ?6 THEN ?6 ELSE last_played_at END \
                 WHERE id = ?1",
            )?
            .execute(params![id, t.play_count, t.skip_count, t.rating.min(5), t.added_at, t.last_played_at])?;
            let mut ins = tx.prepare_cached(
                "INSERT INTO plays (track_id, played_at, ms_listened) SELECT ?1, ?2, ?3 \
                 WHERE NOT EXISTS (SELECT 1 FROM plays WHERE track_id = ?1 AND played_at = ?2)",
            )?;
            for &(at, ms) in &t.plays {
                ins.execute(params![id, at, ms.max(0)])?;
            }
        }
        tx.commit()?;
        Ok(rep)
    }
}

/// Applies stats imported from `source`; only the change since the last import of that source.
#[allow(clippy::too_many_arguments)]
fn apply_imported(
    conn: &Connection,
    id: i64,
    source: &str,
    plays: i64,
    skips: i64,
    rating: i64,
    added: Option<i64>,
    last_played: Option<i64>,
) -> Result<()> {
    let (old_p, old_s): (i64, i64) = conn
        .query_row(
            "SELECT plays, skips FROM imported_stats WHERE track_id = ?1 AND source = ?2",
            params![id, source],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, 0));
    conn.prepare_cached(
        "UPDATE tracks SET play_count = MAX(0, play_count + ?2), skip_count = MAX(0, skip_count + ?3), \
            rating = CASE WHEN rating = 0 THEN ?4 ELSE rating END, \
            added_at = MIN(added_at, COALESCE(?5, added_at)), \
            last_played_at = CASE WHEN ?6 IS NULL THEN last_played_at \
                WHEN last_played_at IS NULL OR last_played_at < ?6 THEN ?6 ELSE last_played_at END \
         WHERE id = ?1",
    )?
    .execute(params![id, plays - old_p, skips - old_s, rating.clamp(0, 5), added, last_played])?;
    conn.prepare_cached(
        "INSERT INTO imported_stats (track_id, source, plays, skips) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(track_id, source) DO UPDATE SET plays = excluded.plays, skips = excluded.skips",
    )?
    .execute(params![id, source, plays, skips])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fake_track;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("poximp-import-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn utf16(s: &str) -> Vec<u8> {
        let mut b = vec![0xFF, 0xFE];
        b.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        b
    }

    /// Native absolute path under a fake music folder.
    fn music(rel: &str) -> String {
        let root = if cfg!(windows) { "C:\\Música" } else { "/Música" };
        format!("{root}{}{}", std::path::MAIN_SEPARATOR, rel.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    #[test]
    fn aimp_playlist_real_format() {
        let d = dir("aimp");
        let mut lib = Library::open_in_memory().unwrap();
        let a = fake_track(&lib, &music("Quevedo/CAPRICHOSO.mp3"), "CAPRICHOSO", "Quevedo, DJ Hummer", "duration_ms = 195750");
        // Moved file: found by title + artist + duration (±2 s).
        let b = fake_track(&lib, &music("Moved/Sesion.mp3"), "Sesion", "DJ Hummer", "duration_ms = 1465000");
        // Same layout as AIMP 5's own Default.aimppl4 (UTF-16 LE with BOM, CRLF).
        let pl = format!(
            "#-----SUMMARY-----#\r\nID={{0B551064-010A-43A3-88D9-973FFC6F2C02}}\r\nName=Para el coche\r\nContentFiles=3\r\n\r\n\
             #-----SETTINGS-----#\r\nFlags=2047\r\nFormatMainLine=%IF(%Artist,%Artist - %Title,%Title)\r\n\r\n\
             #-----CONTENT-----#\r\n-{dir}\r\n\
             {pa}||Quevedo, DJ Hummer|||||||||320|4098|44100|195750|7872932|0|1|0||||0||||\r\n\
             {pb}|Sesion|DJ Hummer||||2026|||||320|4098|44100|1464830|58593754|0|1|0||||0||||\r\n\
             {pc}|Nada|Nadie||||||||||||200000|1|0|1|0||||0||||\r\n",
            dir = music("Quevedo"),
            pa = music("Quevedo/CAPRICHOSO.mp3"),
            pb = music("Old/Sesion.mp3"),
            pc = music("x.mp3"),
        );
        let f = d.join("Default.aimppl4");
        fs::write(&f, utf16(&pl)).unwrap();
        let r = lib.import_aimp_playlist(&f).unwrap();
        assert_eq!((r.matched, r.unmatched, r.unmatched_items.as_slice()), (2, 1, [music("x.mp3")].as_slice()));
        let p = &lib.playlists().unwrap()[0];
        assert_eq!(p.name, "Para el coche");
        assert_eq!(lib.playlist_ids(p.id).unwrap(), [a, b]);
    }

    #[test]
    fn m3u_round_trip_relative_and_nfd() {
        let d = dir("m3u");
        let mut lib = Library::open_in_memory().unwrap();
        let base = d.join("Musica");
        let p1 = base.join("Rosalía").join("Malamente.flac");
        let p2 = base.join("b.flac");
        let a = fake_track(&lib, &p1.to_string_lossy(), "Malamente", "Rosalía", "");
        let b = fake_track(&lib, &p2.to_string_lossy(), "B", "X", "");
        let pl = lib.create_playlist("Mix", crate::PlaylistKind::Manual, None).unwrap();
        lib.set_playlist_tracks(pl, &[a, b]).unwrap();
        let out = d.join("lists").join("mix.m3u8");
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        assert_eq!(lib.export_m3u(pl, &out).unwrap(), 2);
        let text = fs::read_to_string(&out).unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        assert!(text.starts_with("#EXTM3U\n#EXTINF:180,Rosalía - Malamente\n"), "{text}");
        assert!(text.contains(&format!("..{sep}Musica{sep}Rosalía{sep}Malamente.flac")), "{text}");

        let r = lib.import_m3u(&out).unwrap();
        assert_eq!((r.matched, r.unmatched), (2, 0));
        assert_eq!(lib.playlist_ids(r.playlists[0]).unwrap(), [a, b]);

        // A list from macOS: NFD names, file:// URL, plus a stream and a missing file.
        let nfd: String = p1.to_string_lossy().nfd().collect();
        let s = p2.to_string_lossy().replace('\\', "/").replace(' ', "%20");
        let url = format!("file://{}{s}", if s.starts_with('/') { "" } else { "/" });
        let mac = format!("#EXTM3U\n{nfd}\n{url}\nhttp://radio.example/stream\n#EXTINF:181,X - B\nnowhere/b.flac\nnowhere/c.flac\n");
        let f = d.join("mac.m3u");
        fs::write(&f, mac).unwrap();
        let r = lib.import_m3u(&f).unwrap();
        // nowhere/b.flac matches by #EXTINF metadata, c.flac does not.
        assert_eq!((r.matched, r.unmatched), (3, 1));
        assert_eq!(lib.playlist_ids(r.playlists[0]).unwrap(), [a, b, b]);
        // "Open with" a list: just its files, in order, the stream skipped, relative ones resolved.
        let files = m3u_files(&f).unwrap();
        assert_eq!(files.len(), 4);
        assert_eq!(files[1], p2);
        assert_eq!(files[3], d.join("nowhere").join("c.flac"));
    }

    #[test]
    fn itunes_library_stats_and_playlists() {
        let d = dir("itunes");
        let mut lib = Library::open_in_memory().unwrap();
        let a = fake_track(&lib, &music("A & B/One.m4a"), "One", "A & B", "duration_ms = 200000, added_at = 2000000000");
        let b = fake_track(&lib, &music("Two.mp3"), "Two", "C", "duration_ms = 100000, play_count = 1");
        let url = |rel: &str| {
            let p = music(rel).replace('\\', "/").replace(' ', "%20").replace('&', "&#38;");
            if cfg!(windows) { format!("file://localhost/{p}") } else { format!("file://{p}") }
        };
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Major Version</key><integer>1</integer>
	<key>Tracks</key>
	<dict>
		<key>100</key>
		<dict>
			<key>Track ID</key><integer>100</integer>
			<key>Name</key><string>One</string>
			<key>Artist</key><string>A &amp; B</string>
			<key>Total Time</key><integer>200100</integer>
			<key>Date Added</key><date>2019-03-04T21:15:33Z</date>
			<key>Play Count</key><integer>12</integer>
			<key>Play Date UTC</key><date>2024-01-02T03:04:05Z</date>
			<key>Skip Count</key><integer>2</integer>
			<key>Rating</key><integer>80</integer>
			<key>Loved</key><true/>
			<key>Location</key><string>{la}</string>
		</dict>
		<key>101</key>
		<dict>
			<key>Track ID</key><integer>101</integer>
			<key>Name</key><string>Two</string>
			<key>Artist</key><string>C</string>
			<key>Total Time</key><integer>101500</integer>
			<key>Play Count</key><integer>3</integer>
			<key>Rating</key><integer>100</integer>
			<key>Rating Computed</key><true/>
			<key>Location</key><string>file://localhost/Z:/elsewhere/Two.mp3</string>
		</dict>
		<key>102</key>
		<dict>
			<key>Track ID</key><integer>102</integer>
			<key>Name</key><string>Lost</string>
			<key>Location</key><string>file://localhost/Z:/lost.mp3</string>
		</dict>
		<key>103</key>
		<dict><key>Track ID</key><integer>103</integer><key>Name</key><string>Radio</string><key>Track Type</key><string>URL</string></dict>
	</dict>
	<key>Playlists</key>
	<array>
		<dict><key>Name</key><string>Library</string><key>Master</key><true/><key>Playlist Items</key><array><dict><key>Track ID</key><integer>100</integer></dict></array></dict>
		<dict><key>Name</key><string>Music</string><key>Distinguished Kind</key><integer>4</integer></dict>
		<dict><key>Name</key><string>Coche</string><key>Playlist Items</key><array>
			<dict><key>Track ID</key><integer>101</integer></dict>
			<dict><key>Track ID</key><integer>102</integer></dict>
			<dict><key>Track ID</key><integer>100</integer></dict>
		</array></dict>
		<dict><key>Name</key><string>Vacía</string><key>Playlist Items</key><array/></dict>
	</array>
</dict>
</plist>
"#,
            la = url("A & B/One.m4a")
        );
        let f = d.join("Library.xml");
        fs::write(&f, xml).unwrap();
        let r = lib.import_itunes_xml(&f).unwrap();
        assert_eq!((r.matched, r.unmatched, r.playlists.len()), (2, 1, 1), "{r:?}");
        let ta = lib.track_by_id(a).unwrap().unwrap();
        assert_eq!((ta.play_count, ta.skip_count, ta.rating), (12, 2, 4));
        assert_eq!((ta.added_at, ta.last_played_at), (1_551_734_133, Some(1_704_164_645)));
        // Computed (album) rating ignored; imported plays add to local ones.
        let tb = lib.track_by_id(b).unwrap().unwrap();
        assert_eq!((tb.play_count, tb.rating), (4, 0));
        assert_eq!(lib.playlist_ids(r.playlists[0]).unwrap(), [b, a]);

        // Importing again changes nothing (and makes no duplicate playlist).
        let r = lib.import_itunes_xml(&f).unwrap();
        assert!(r.playlists.is_empty());
        assert_eq!(lib.track_by_id(a).unwrap().unwrap().play_count, 12);
        assert_eq!(lib.track_by_id(b).unwrap().unwrap().play_count, 4);

        fs::write(&f, "<plist><dict><key>x</key>").unwrap();
        assert!(lib.import_itunes_xml(&f).is_err());
    }

    #[test]
    fn backup_round_trip_is_idempotent() {
        let d = dir("backup");
        let mut src = Library::open_in_memory().unwrap();
        let a = fake_track(&src, &music("a.flac"), "A", "X", "rating = 5, skip_count = 1");
        src.record_play(a, 120_000).unwrap();
        src.record_play(a, 90_000).unwrap();
        let f = d.join("backup.json");
        assert_eq!(src.export_stats_backup(&f).unwrap(), 1);

        let mut dst = Library::open_in_memory().unwrap();
        let a2 = fake_track(&dst, &music("a.flac"), "A", "X", "");
        for _ in 0..2 {
            let r = dst.import_stats_backup(&f).unwrap();
            assert_eq!((r.matched, r.unmatched), (1, 0));
        }
        let t = dst.track_by_id(a2).unwrap().unwrap();
        assert_eq!((t.play_count, t.skip_count, t.rating), (2, 1, 5));
        let n: i64 = dst.conn.query_row("SELECT COUNT(*) FROM plays", [], |r| r.get(0)).unwrap();
        // Both plays share the same second here: the same play once.
        assert!((1..=2).contains(&n));
        fs::write(&f, "{\"app\":\"other\"}").unwrap();
        assert!(dst.import_stats_backup(&f).is_err());
    }

    #[test]
    fn helpers() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_utc("garbage"), None);
        assert_eq!(unescape("a &amp; b &#233;&#xE9; &bogus; &"), "a & b éé &bogus; &");
        let p = file_url_to_path("file://localhost/C:/M%C3%BAsica/a%20b.mp3").unwrap();
        assert_eq!(p, if cfg!(windows) { "C:\\Música\\a b.mp3" } else { "C:/Música/a b.mp3" });
        assert_eq!(file_url_to_path("file:///Users/x/a.mp3").unwrap().replace('\\', "/"), "/Users/x/a.mp3");
        assert_eq!(file_url_to_path("file://nas/share/a.mp3").unwrap().replace('\\', "/"), "//nas/share/a.mp3");
        assert!(file_url_to_path("http://x").is_none());
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'h', b'i']), "hi");
        assert_eq!(decode(&[b'c', 0xE9]), "cé");
        assert_eq!(lexical(Path::new("a/b/../c/./d")), Path::new("a/c/d"));
    }
}
