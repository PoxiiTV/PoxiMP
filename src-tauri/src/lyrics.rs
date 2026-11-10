//! Lyrics for the lyrics view: on disk first (tags, sidecar .lrc), then LRCLIB (lrclib.net: free,
//! no account) when "online lyrics" is on. Online answers, "not found" included, are cached in
//! the library so each track is asked once (a miss again after `RETRY_DAYS`).

use std::path::Path;

use poximp_library::lyrics::{self as lib_lyrics, Lyrics};
use serde::Deserialize;

use crate::model::{Error, Result};
use crate::App;

const API: &str = "https://lrclib.net/api";
const MAX_BYTES: usize = 2 << 20;
/// A search hit has to last this close to the track (seconds), as LRCLIB itself matches.
const DURATION_SLACK: f64 = 5.0;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Record {
    duration: Option<f64>,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

/// LRC text and plain text, as LRCLIB has them.
#[derive(Debug, Default, PartialEq)]
pub struct Found {
    pub synced: Option<String>,
    pub plain: Option<String>,
}

fn found(r: Record) -> Option<Found> {
    let keep = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
    let f = Found { synced: keep(r.synced_lyrics), plain: keep(r.plain_lyrics) };
    (!r.instrumental && (f.synced.is_some() || f.plain.is_some())).then_some(f)
}

/// Best search hit: close in duration, synced before plain, then the closest.
fn best(list: Vec<Record>, duration: f64) -> Option<Found> {
    let off = |r: &Record| r.duration.map_or(f64::MAX, |d| (d - duration).abs());
    list.into_iter()
        .filter(|r| duration <= 0.0 || off(r) <= DURATION_SLACK)
        .min_by(|a, b| (a.synced_lyrics.is_none(), off(a)).partial_cmp(&(b.synced_lyrics.is_none(), off(b))).unwrap_or(std::cmp::Ordering::Equal))
        .and_then(found)
}

/// `Ok(None)` = LRCLIB has nothing for it; `Err` = offline or the service failed.
async fn get_json<T: serde::de::DeserializeOwned>(url: String, query: &[(&str, String)]) -> std::result::Result<Option<T>, ()> {
    let res = crate::autoeq::client().map_err(drop)?.get(url).query(query).send().await.map_err(drop)?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let bytes = res.error_for_status().map_err(drop)?.bytes().await.map_err(drop)?;
    if bytes.len() > MAX_BYTES {
        return Err(());
    }
    serde_json::from_slice(&bytes).map(Some).map_err(drop)
}

/// Exact match first (`/api/get`: artist, title, album, duration), then `/api/search`.
pub async fn fetch(artist: &str, title: &str, album: &str, duration: f64) -> std::result::Result<Option<Found>, ()> {
    let base = [("artist_name", artist.to_string()), ("track_name", title.to_string())];
    if !album.is_empty() && duration > 0.0 {
        let mut q = base.to_vec();
        q.extend([("album_name", album.to_string()), ("duration", format!("{}", duration.round()))]);
        // /get also looks the track up elsewhere and that can fail (503) where /search works:
        // only /search failing means offline.
        if let Ok(Some(f)) = get_json::<Record>(format!("{API}/get"), &q).await.map(|r| r.and_then(found)) {
            return Ok(Some(f));
        }
    }
    let list: Vec<Record> = get_json(format!("{API}/search"), &base).await?.unwrap_or_default();
    Ok(best(list, duration))
}

/// Synced beats plain; between two of a kind, what is on disk wins.
fn pick(local: Option<Lyrics>, online: Option<Lyrics>) -> Lyrics {
    match (local, online) {
        (Some(l), Some(o)) if !l.is_synced() && o.is_synced() => o,
        (Some(l), _) => l,
        (None, o) => o.unwrap_or_default(),
    }
}

pub async fn get(app: &App, track_id: i64) -> Result<Lyrics> {
    let t = app.library().track_by_id(track_id)?.ok_or(Error::NotFound)?;
    let local = lib_lyrics::local(Path::new(&t.path));
    if local.as_ref().is_some_and(Lyrics::is_synced) {
        return Ok(pick(local, None));
    }
    let cached = app.library().cached_lyrics(track_id)?;
    if let Some(c) = cached.filter(|c| !c.stale()) {
        return Ok(pick(local, c.lyrics));
    }
    let online = app.sound().cfg.online_lyrics;
    if !online || t.title.trim().is_empty() {
        return Ok(pick(local, None));
    }
    match fetch(t.artist.trim(), t.title.trim(), t.album.trim(), t.duration_ms as f64 / 1000.0).await {
        Ok(f) => {
            let f = f.unwrap_or_default();
            app.library().cache_lyrics(track_id, "lrclib", f.synced.as_deref(), f.plain.as_deref())?;
            Ok(pick(local, Lyrics::from_parts(f.synced.as_deref(), f.plain.as_deref(), "lrclib")))
        }
        // Offline: what is on disk, or tell the UI it could not ask.
        Err(()) => local.ok_or(Error::Network),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(duration: f64, synced: Option<&str>, plain: Option<&str>) -> Record {
        Record { duration: Some(duration), instrumental: false, synced_lyrics: synced.map(Into::into), plain_lyrics: plain.map(Into::into) }
    }

    #[test]
    fn picks_the_best_search_hit() {
        let list = vec![rec(200.0, None, Some("plain")), rec(203.0, Some("[00:01.00]a"), None), rec(260.0, Some("[00:01.00]far"), None)];
        assert_eq!(best(list, 201.0).unwrap().synced.as_deref(), Some("[00:01.00]a"), "synced and close beats plain and closer");
        assert!(best(vec![rec(300.0, Some("x"), None)], 200.0).is_none(), "too far in duration");
        assert!(best(vec![Record { instrumental: true, ..rec(200.0, None, Some("x")) }], 200.0).is_none());
        assert!(best(vec![rec(200.0, Some("  "), Some(""))], 200.0).is_none(), "empty texts are nothing");
    }

    #[test]
    fn local_and_online_merge() {
        let plain = Lyrics::from_parts(None, Some("p"), "embedded");
        let synced = Lyrics::from_parts(Some("[00:01.00]s"), None, "lrclib");
        assert_eq!(pick(plain.clone(), synced.clone()).source.as_deref(), Some("lrclib"), "synced online beats plain local");
        assert_eq!(pick(plain.clone(), Lyrics::from_parts(None, Some("q"), "lrclib")).source.as_deref(), Some("embedded"));
        assert_eq!(pick(None, None), Lyrics::default());
    }
}

#[cfg(test)]
mod online {
    /// Needs internet: `cargo test -p poximp -- --ignored lrclib_online --nocapture`
    #[test]
    #[ignore]
    fn lrclib_online() {
        tauri::async_runtime::block_on(async {
            let f = super::fetch("Queen", "Bohemian Rhapsody", "A Night at the Opera", 354.0).await.expect("reachable");
            let f = f.expect("found");
            println!("{:?}", f.synced.as_deref().map(|s| &s[..s.len().min(120)]));
            assert!(f.synced.is_some() || f.plain.is_some());
        });
    }
}
