//! Global search: tracks, albums, artists and playlists, grouped. Same matching everywhere
//! (every word is the prefix of a word, accents and case ignored, like `fts_query`).

use rusqlite::params;
use serde::Serialize;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

use crate::{fts_query, Library, Result, Track};

/// One result. `id` is the track / artist / playlist id or the album key, as text: other sources
/// (a streaming service) can use their own ids in the same shape.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit<T> {
    pub id: String,
    pub title: String,
    /// Artist (tracks), album artist (albums; None = various artists).
    pub subtitle: Option<String>,
    /// Tracks in the album / by the artist / in the playlist.
    pub count: Option<u32>,
    /// A track whose cover stands for this hit.
    pub cover_id: Option<i64>,
    pub track: Option<T>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchGroup<T> {
    /// "local" (the library).
    pub source: &'static str,
    /// "tracks", "albums", "artists" or "playlists".
    pub kind: &'static str,
    pub hits: Vec<SearchHit<T>>,
}

impl<T> SearchGroup<T> {
    pub fn map<U>(self, f: &impl Fn(T) -> U) -> SearchGroup<U> {
        let hits = self
            .hits
            .into_iter()
            .map(|h| SearchHit { id: h.id, title: h.title, subtitle: h.subtitle, count: h.count, cover_id: h.cover_id, track: h.track.map(f) })
            .collect();
        SearchGroup { source: self.source, kind: self.kind, hits }
    }
}

/// Lowercase, accents removed ("Canción" -> "cancion").
fn fold(s: &str) -> String {
    s.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

fn words(s: &str) -> Vec<String> {
    fold(s).split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Every query word starts some word of `name`.
fn matches(query: &[String], name: &str) -> bool {
    let n = words(name);
    !query.is_empty() && query.iter().all(|q| n.iter().any(|w| w.starts_with(q.as_str())))
}

impl Library {
    /// Groups with at least one hit, in display order: tracks, albums, artists, playlists.
    pub fn search_all(&self, q: &str, per_group: u32) -> Result<Vec<SearchGroup<Track>>> {
        let query = words(q);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let n = per_group.clamp(1, 50);
        let tracks = self.search(q, n)?.into_iter().map(|t| SearchHit {
            id: t.id.to_string(),
            title: t.title.clone(),
            subtitle: Some(t.artist.clone()),
            count: None,
            cover_id: Some(t.id),
            track: Some(t),
        });
        let groups = [
            ("tracks", tracks.collect()),
            ("albums", self.search_albums(q, n)?),
            ("artists", self.search_artists(&query, n)?),
            ("playlists", self.search_playlists(&query, n)?),
        ];
        Ok(groups.into_iter().filter(|(_, h)| !h.is_empty()).map(|(kind, hits)| SearchGroup { source: "local", kind, hits }).collect())
    }

    /// Albums whose title matches (FTS on the album column), biggest first.
    fn search_albums(&self, q: &str, n: u32) -> Result<Vec<SearchHit<Track>>> {
        let Some(fts) = fts_query(q) else { return Ok(Vec::new()) };
        // Column filter on every word: `album : "w"*`.
        let fts = fts.split(' ').map(|w| format!("album : {w}")).collect::<Vec<_>>().join(" ");
        let mut st = self.conn.prepare_cached(
            "SELECT album_key, MAX(album), \
                CASE WHEN MAX(album_artist) <> '' THEN MAX(album_artist) \
                     WHEN COUNT(DISTINCT artist) = 1 THEN MAX(artist) END, \
                COUNT(*), COALESCE(MAX(CASE WHEN cover IS NOT NULL THEN id END), MIN(id)) \
             FROM tracks WHERE available = 1 AND in_library = 1 AND album <> '' \
               AND id IN (SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?1) \
             GROUP BY album_key ORDER BY COUNT(*) DESC, 2 COLLATE NOCASE LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![fts, n], |r| {
                Ok(SearchHit { id: r.get(0)?, title: r.get(1)?, subtitle: r.get(2)?, count: Some(r.get(3)?), cover_id: r.get(4)?, track: None })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// ponytail: scans every artist name in Rust (accent folding); an artists FTS table if libraries reach ~100k artists.
    fn search_artists(&self, query: &[String], n: u32) -> Result<Vec<SearchHit<Track>>> {
        let mut st = self.conn.prepare_cached(
            "SELECT a.id, a.name, COUNT(DISTINCT t.id), MAX(CASE WHEN t.cover IS NOT NULL THEN t.id END), MIN(t.id) \
             FROM artists a JOIN track_artists ta ON ta.artist_id = a.id \
             JOIN tracks t ON t.id = ta.track_id AND t.available = 1 AND t.in_library = 1 GROUP BY a.id",
        )?;
        let mut hits: Vec<SearchHit<Track>> = st
            .query_map([], |r| {
                let cover: Option<i64> = r.get(3)?;
                Ok(SearchHit {
                    id: r.get::<_, i64>(0)?.to_string(),
                    title: r.get(1)?,
                    subtitle: None,
                    count: Some(r.get(2)?),
                    cover_id: cover.or(r.get(4)?),
                    track: None,
                })
            })?
            .filter_map(|h| h.ok().filter(|h| matches(query, &h.title)))
            .collect();
        hits.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())));
        hits.truncate(n as usize);
        Ok(hits)
    }

    fn search_playlists(&self, query: &[String], n: u32) -> Result<Vec<SearchHit<Track>>> {
        // Only the matching smart playlists get evaluated (for their count).
        let mut hits = Vec::new();
        for p in self.playlist_rows(None)?.into_iter().filter(|p| matches(query, &p.name)).take(n as usize) {
            let p = if p.kind == crate::PlaylistKind::Smart { self.playlist(p.id)? } else { p };
            hits.push(SearchHit { id: p.id.to_string(), title: p.name, subtitle: None, count: Some(p.track_count), cover_id: None, track: None });
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fake_track;
    use crate::PlaylistKind;

    #[test]
    fn groups_and_matching() {
        assert_eq!(fold("Canción ÑANDÚ"), "cancion nandu");
        assert!(matches(&words("can nan"), "Canción, Ñandú") && !matches(&words("xyz"), "Canción"));
        let lib = Library::open_in_memory().unwrap();
        let a = fake_track(&lib, "/m/1.flac", "Canción del mar", "Lúa feat. Bo", "album = 'Mareas', album_key = 'k1'");
        fake_track(&lib, "/m/2.flac", "Otra", "Lúa", "album = 'Mareas', album_key = 'k1'");
        fake_track(&lib, "/m/3.flac", "Lejos", "Zeta", "album = 'Mar adentro', album_key = 'k2'");
        lib.create_playlist("Mar y sol", PlaylistKind::Manual, None).unwrap();
        assert!(lib.search_all("  ¿? ", 5).unwrap().is_empty());

        let g = lib.search_all("mar", 5).unwrap();
        let kinds: Vec<_> = g.iter().map(|g| (g.source, g.kind, g.hits.len())).collect();
        assert_eq!(kinds, vec![("local", "tracks", 3), ("local", "albums", 2), ("local", "playlists", 1)]);
        assert_eq!(g[1].hits[0].title, "Mareas", "bigger album first");
        assert_eq!((g[1].hits[0].count, g[1].hits[0].subtitle.as_deref()), (Some(2), None), "two artists = various");

        let g = lib.search_all("lua", 5).unwrap();
        let artists = g.iter().find(|g| g.kind == "artists").unwrap();
        assert_eq!((artists.hits[0].title.as_str(), artists.hits[0].count), ("Lúa", Some(2)));
        let tracks = g.iter().find(|g| g.kind == "tracks").unwrap();
        assert!(tracks.hits.iter().any(|h| h.track.as_ref().map(|t| t.id) == Some(a)));
    }
}
