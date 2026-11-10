//! Lyrics: LRC parsing, local sources (embedded tags, a sidecar `.lrc`) and the cache of online
//! results. The download itself lives in the app (it owns the HTTP client and the setting).

use std::path::Path;

use lofty::prelude::*;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::{now, Library, Result};

/// A cached "not found" is asked again after this many days.
pub const RETRY_DAYS: i64 = 14;
/// Bigger sidecar files are not lyrics.
const MAX_LRC_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub t_ms: u32,
    pub text: String,
}

/// What the lyrics view shows. `source`: "embedded", "lrc" (sidecar file), "lrclib", or None.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub synced: Option<Vec<Line>>,
    pub plain: Option<String>,
    pub source: Option<String>,
}

impl Lyrics {
    /// From LRC (or plain) text: synced when it has timestamps, plain otherwise. None if empty.
    pub fn from_text(text: &str, source: &str) -> Option<Self> {
        let synced = parse_lrc(text);
        let plain = if synced.is_empty() { text.trim().to_string() } else { lrc_plain(&synced) };
        if plain.is_empty() && synced.is_empty() {
            return None;
        }
        Some(Self { synced: (!synced.is_empty()).then_some(synced), plain: (!plain.is_empty()).then_some(plain), source: Some(source.into()) })
    }

    /// From stored / downloaded parts: `synced` as LRC text. None when both are empty.
    pub fn from_parts(synced: Option<&str>, plain: Option<&str>, source: &str) -> Option<Self> {
        let synced = synced.map(parse_lrc).filter(|l| !l.is_empty());
        let plain = plain.map(str::trim).filter(|p| !p.is_empty()).map(String::from);
        (synced.is_some() || plain.is_some()).then(|| Self { synced, plain, source: Some(source.into()) })
    }

    pub fn is_synced(&self) -> bool {
        self.synced.is_some()
    }
}

fn lrc_plain(lines: &[Line]) -> String {
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// `mm:ss`, `mm:ss.x`, `mm:ss.xx`, `mm:ss.xxx` (also `:` before the fraction) -> ms.
fn stamp(s: &str) -> Option<i64> {
    let (m, rest) = s.split_once(':')?;
    let (sec, frac) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let ok = |p: &str, max: usize| !p.is_empty() && p.len() <= max && p.bytes().all(|b| b.is_ascii_digit());
    if !ok(m, 4) || !ok(sec, 2) || !(frac.is_empty() || ok(frac, 3)) {
        return None;
    }
    let sec: i64 = sec.parse().ok()?;
    if sec >= 60 {
        return None;
    }
    let frac_ms = match frac.len() {
        0 => 0,
        n => frac.parse::<i64>().ok()? * 10i64.pow(3 - n as u32),
    };
    Some(m.parse::<i64>().ok()? * 60_000 + sec * 1000 + frac_ms)
}

/// Removes enhanced-LRC word stamps (`<00:12.34>`).
fn strip_word_stamps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        match rest[i + 1..].find('>') {
            Some(j) if stamp(&rest[i + 1..i + 1 + j]).is_some() => {
                out.push_str(&rest[..i]);
                rest = &rest[i + j + 2..];
            }
            _ => {
                out.push_str(&rest[..=i]);
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// LRC -> lines sorted by time. Several stamps on one line repeat it; `[offset:±ms]` shifts every
/// line (positive = earlier, as the format says); other tags (`[ar:]`, `[ti:]`...) are ignored.
/// Empty when the text has no timed line.
pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut offset = 0i64;
    let mut timed: Vec<(i64, String)> = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim_start_matches('\u{feff}').trim();
        let mut stamps = Vec::new();
        while let Some(body) = rest.strip_prefix('[') {
            let Some(end) = body.find(']') else { break };
            let tag = &body[..end];
            match stamp(tag) {
                Some(ms) => stamps.push(ms),
                None => {
                    if let Some(v) = tag.strip_prefix("offset:") {
                        offset = v.trim().parse().unwrap_or(0);
                    }
                    if stamps.is_empty() {
                        // A metadata tag: nothing else on this line.
                        rest = "";
                    }
                    break;
                }
            }
            rest = body[end + 1..].trim_start();
        }
        let line = strip_word_stamps(rest);
        timed.extend(stamps.into_iter().map(|ms| (ms, line.clone())));
    }
    let mut out: Vec<Line> = timed.into_iter().map(|(ms, text)| Line { t_ms: (ms - offset).clamp(0, u32::MAX as i64) as u32, text }).collect();
    out.sort_by_key(|l| l.t_ms);
    out
}

/// ID3v2 SYLT (synced lyrics, millisecond stamps) of an MP3. Not in the generic tag model.
fn sylt(path: &Path) -> Option<Vec<Line>> {
    use lofty::id3::v2::{Frame, SyncTextContentType, SynchronizedTextFrame, TimestampFormat};
    let mut file = std::fs::File::open(path).ok()?;
    let mp3 = lofty::mpeg::MpegFile::read_from(&mut file, lofty::config::ParseOptions::new()).ok()?;
    for frame in mp3.id3v2()? {
        let Frame::Binary(b) = frame else { continue };
        if frame.id_str() != "SYLT" {
            continue;
        }
        let Ok(s) = SynchronizedTextFrame::parse(&b.data, frame.flags()) else { continue };
        if !matches!(s.timestamp_format, TimestampFormat::MS) || !matches!(s.content_type, SyncTextContentType::Lyrics | SyncTextContentType::Other) {
            continue;
        }
        let mut lines: Vec<Line> = s.content.into_iter().map(|(t_ms, text)| Line { t_ms, text: text.trim().to_string() }).collect();
        lines.sort_by_key(|l| l.t_ms);
        if !lines.is_empty() {
            return Some(lines);
        }
    }
    None
}

/// Lyrics stored in the file's tags: SYLT, else USLT / LYRICS / ©lyr (LRC text there counts as synced).
fn embedded(path: &Path) -> Option<Lyrics> {
    let f = lofty::read_from_path(path).ok()?;
    let text = f
        .tags()
        .iter()
        .flat_map(|t| t.get_strings(ItemKey::Lyrics).chain(t.get_strings(ItemKey::UnsyncLyrics)))
        .find(|s| !s.trim().is_empty())
        .map(str::to_string);
    let tagged = text.as_deref().and_then(|t| Lyrics::from_text(t, "embedded"));
    if tagged.as_ref().is_some_and(Lyrics::is_synced) {
        return tagged;
    }
    if f.file_type() == lofty::file::FileType::Mpeg {
        if let Some(lines) = sylt(path) {
            let plain = tagged.as_ref().and_then(|l| l.plain.clone()).or_else(|| Some(lrc_plain(&lines)).filter(|p| !p.is_empty()));
            return Some(Lyrics { synced: Some(lines), plain, source: Some("embedded".into()) });
        }
    }
    tagged
}

/// `song.lrc` next to `song.flac`.
fn sidecar(path: &Path) -> Option<Lyrics> {
    let lrc = path.with_extension("lrc");
    if std::fs::metadata(&lrc).ok()?.len() > MAX_LRC_BYTES {
        return None;
    }
    let bytes = std::fs::read(&lrc).ok()?;
    Lyrics::from_text(&String::from_utf8_lossy(&bytes), "lrc")
}

/// Best lyrics on disk: synced beats plain, the tags beat the sidecar file. Never fails (None).
pub fn local(path: &Path) -> Option<Lyrics> {
    let tags = embedded(path);
    if tags.as_ref().is_some_and(Lyrics::is_synced) {
        return tags;
    }
    match sidecar(path) {
        Some(s) if s.is_synced() || tags.is_none() => Some(s),
        _ => tags,
    }
}

/// A cache row: `None` lyrics = looked up and not found.
pub struct Cached {
    pub lyrics: Option<Lyrics>,
    pub fetched_at: i64,
}

impl Cached {
    /// A "not found" old enough to ask again.
    pub fn stale(&self) -> bool {
        self.lyrics.is_none() && now() - self.fetched_at > RETRY_DAYS * 86_400
    }
}

impl Library {
    pub fn cached_lyrics(&self, track_id: i64) -> Result<Option<Cached>> {
        let row: Option<(String, Option<String>, Option<String>, i64)> = self
            .conn
            .query_row("SELECT source, synced, plain, fetched_at FROM lyrics WHERE track_id = ?1", [track_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()?;
        Ok(row.map(|(source, synced, plain, fetched_at)| Cached {
            lyrics: (source != "none").then(|| Lyrics::from_parts(synced.as_deref(), plain.as_deref(), &source)).flatten(),
            fetched_at,
        }))
    }

    /// `synced` as LRC text. Both None = not found.
    pub fn cache_lyrics(&self, track_id: i64, source: &str, synced: Option<&str>, plain: Option<&str>) -> Result<()> {
        let source = if synced.is_none() && plain.is_none() { "none" } else { source };
        self.conn.execute(
            "INSERT INTO lyrics (track_id, source, synced, plain, fetched_at) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(track_id) DO UPDATE SET source = excluded.source, synced = excluded.synced, \
             plain = excluded.plain, fetched_at = excluded.fetched_at",
            params![track_id, source, synced, plain, now()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(t_ms: u32, text: &str) -> Line {
        Line { t_ms, text: text.into() }
    }

    #[test]
    fn parses_lrc() {
        let text = "\u{feff}[ar:Someone]\n[ti:Song]\n[length: 03:20]\n[00:12.00]First\n[00:15.5]Second <00:16.10>word\n\
[01:02.345][00:20.10]Twice\n[00:25:50]Colon\nno stamp here\n[00:30.00]\n[100:00.00]Long\n[00:61.00]Bad seconds\n";
        let lines = parse_lrc(text);
        assert_eq!(
            lines,
            vec![
                l(12_000, "First"),
                l(15_500, "Second word"),
                l(20_100, "Twice"),
                l(25_500, "Colon"),
                l(30_000, ""),
                l(62_345, "Twice"),
                l(6_000_000, "Long"),
            ]
        );
        assert!(parse_lrc("Just plain\nlyrics").is_empty());
        assert!(parse_lrc("[ar:Only tags]\n[ti:x]").is_empty());
    }

    #[test]
    fn lrc_offset_and_plain_fallback() {
        let lines = parse_lrc("[offset:+500]\n[00:01.00]a\n[00:00.20]b");
        assert_eq!(lines, vec![l(0, "b"), l(500, "a")], "positive offset = earlier, clamped at 0");
        assert_eq!(parse_lrc("[offset:-250]\n[00:01.00]a")[0].t_ms, 1250);
        let ly = Lyrics::from_text("[00:01.00]one\n[00:02.00]two", "lrc").unwrap();
        assert_eq!((ly.plain.as_deref(), ly.source.as_deref()), (Some("one\ntwo"), Some("lrc")));
        let plain = Lyrics::from_text("  one\ntwo \n", "embedded").unwrap();
        assert!(plain.synced.is_none() && plain.plain.as_deref() == Some("one\ntwo"));
        assert!(Lyrics::from_text(" \n ", "lrc").is_none());
    }

    #[test]
    fn sidecar_and_cache() {
        let dir = std::env::temp_dir().join(format!("poximp-lyrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.flac");
        assert!(local(&song).is_none(), "no file, no sidecar: nothing, no error");
        std::fs::write(dir.join("song.lrc"), "[00:01.00]hola").unwrap();
        let ly = local(&song).unwrap();
        assert_eq!((ly.synced.unwrap(), ly.source.as_deref()), (vec![l(1000, "hola")], Some("lrc")));
        let _ = std::fs::remove_dir_all(&dir);

        let lib = Library::open_in_memory().unwrap();
        let id = crate::tests::fake_track(&lib, "/m/a.flac", "A", "B", "");
        assert!(lib.cached_lyrics(id).unwrap().is_none());
        lib.cache_lyrics(id, "lrclib", None, None).unwrap();
        let c = lib.cached_lyrics(id).unwrap().unwrap();
        assert!(c.lyrics.is_none() && !c.stale(), "fresh not-found");
        lib.conn.execute("UPDATE lyrics SET fetched_at = 0", []).unwrap();
        assert!(lib.cached_lyrics(id).unwrap().unwrap().stale());
        lib.cache_lyrics(id, "lrclib", Some("[00:02.00]x"), Some("x")).unwrap();
        let c = lib.cached_lyrics(id).unwrap().unwrap();
        assert_eq!(c.lyrics.unwrap().synced.unwrap(), vec![l(2000, "x")]);
    }
}
