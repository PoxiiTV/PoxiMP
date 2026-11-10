//! Listening statistics and the yearly Wrapped, from the `plays` table, in local time.
//! Generic over the track type so the app can hand its own wire type to the UI (`map`).

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{now, Error, Library, Result, Track};

const TOP: usize = 10;
/// Plays inside [?1, ?2).
const P: &str = "WITH p AS (SELECT * FROM plays WHERE played_at >= ?1 AND played_at < ?2) ";
const LOCAL: &str = "played_at, 'unixepoch', 'localtime'";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Range {
    /// Last 7 days.
    Week,
    /// Last 30 days.
    Month,
    /// Last 365 days.
    Year,
    All,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopTrack<T> {
    pub track: T,
    pub plays: u32,
    pub listened_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopItem {
    pub name: String,
    /// Artist id (top artists).
    pub id: Option<i64>,
    /// Album key (top albums).
    pub key: Option<String>,
    /// Album artist (top albums).
    pub by: Option<String>,
    /// A track of the item, for its cover.
    pub cover_id: Option<i64>,
    pub plays: u32,
    pub listened_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Streak {
    pub days: u32,
    /// Local dates, "YYYY-MM-DD".
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats<T> {
    /// Window [from, to), unix seconds.
    pub from: i64,
    pub to: i64,
    pub listened_ms: i64,
    pub plays: u32,
    /// Distinct tracks played.
    pub tracks: u32,
    pub top_tracks: Vec<TopTrack<T>>,
    /// Split artists: a "feat." play counts for every artist on the track.
    pub top_artists: Vec<TopItem>,
    pub top_albums: Vec<TopItem>,
    pub top_genres: Vec<TopItem>,
    /// Milliseconds listened per local hour 0..23.
    pub by_hour: Vec<i64>,
    /// Milliseconds listened per weekday, Monday first.
    pub by_weekday: Vec<i64>,
    /// Tracks first ever played inside the window.
    pub discoveries: u32,
    pub top_discoveries: Vec<TopTrack<T>>,
    pub streak: Streak,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthStat {
    /// 1..12
    pub month: u32,
    pub plays: u32,
    pub listened_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Wrapped<T> {
    pub year: i32,
    pub stats: Stats<T>,
    /// Milliseconds listened per month, January first.
    pub by_month: Vec<i64>,
    pub top_month: Option<MonthStat>,
    /// Most played between 00:00 and 05:00.
    pub late_night: Option<TopTrack<T>>,
    /// Most played tracks with the "real" (verified lossless) verdict.
    pub top_verified: Vec<TopTrack<T>>,
    /// Distinct tracks played this year / tracks in the library, 0..100.
    pub library_pct: f64,
}

fn map_tops<T, U>(v: Vec<TopTrack<T>>, f: &impl Fn(T) -> U) -> Vec<TopTrack<U>> {
    v.into_iter().map(|t| TopTrack { track: f(t.track), plays: t.plays, listened_ms: t.listened_ms }).collect()
}

impl<T> Stats<T> {
    pub fn map<U>(self, f: &impl Fn(T) -> U) -> Stats<U> {
        Stats {
            from: self.from,
            to: self.to,
            listened_ms: self.listened_ms,
            plays: self.plays,
            tracks: self.tracks,
            top_tracks: map_tops(self.top_tracks, f),
            top_artists: self.top_artists,
            top_albums: self.top_albums,
            top_genres: self.top_genres,
            by_hour: self.by_hour,
            by_weekday: self.by_weekday,
            discoveries: self.discoveries,
            top_discoveries: map_tops(self.top_discoveries, f),
            streak: self.streak,
        }
    }
}

impl<T> Wrapped<T> {
    pub fn map<U>(self, f: &impl Fn(T) -> U) -> Wrapped<U> {
        Wrapped {
            year: self.year,
            stats: self.stats.map(f),
            by_month: self.by_month,
            top_month: self.top_month,
            late_night: self.late_night.map(|t| TopTrack { track: f(t.track), plays: t.plays, listened_ms: t.listened_ms }),
            top_verified: map_tops(self.top_verified, f),
            library_pct: self.library_pct,
        }
    }
}

/// Longest run of consecutive days; `days` = (julian day, "YYYY-MM-DD") sorted ascending.
fn longest_streak(days: &[(i64, String)]) -> Streak {
    let mut best = Streak::default();
    let mut start = 0;
    for i in 0..days.len() {
        if i > 0 && days[i].0 != days[i - 1].0 + 1 {
            start = i;
        }
        let len = (i - start + 1) as u32;
        if len > best.days {
            best = Streak { days: len, from: Some(days[start].1.clone()), to: Some(days[i].1.clone()) };
        }
    }
    best
}

impl Library {
    pub fn stats(&self, range: Range) -> Result<Stats<Track>> {
        let to = now() + 1;
        let from = match range {
            Range::Week => to - 7 * 86_400,
            Range::Month => to - 30 * 86_400,
            Range::Year => to - 365 * 86_400,
            Range::All => 0,
        };
        self.stats_between(from, to)
    }

    /// Unix seconds of local midnight on 1 January of `year`.
    fn year_start(&self, year: i32) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT CAST(strftime('%s', ?1, 'utc') AS INTEGER)",
            [format!("{year:04}-01-01 00:00:00")],
            |r| r.get(0),
        )?)
    }

    /// (track, plays, ms) for `sql` = "SELECT track_id, COUNT(*), SUM(ms_listened) ..." over `p`.
    fn tops(&self, sql: &str, from: i64, to: i64, limit: usize) -> Result<Vec<TopTrack<Track>>> {
        let mut st = self.conn.prepare(&format!("{P}{sql} ORDER BY 2 DESC, 3 DESC, 1 LIMIT {limit}"))?;
        let rows = st
            .query_map(params![from, to], |r| Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<(i64, u32, i64)>>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, plays, listened_ms) in rows {
            if let Some(track) = self.track_by_id(id)? {
                out.push(TopTrack { track, plays, listened_ms });
            }
        }
        Ok(out)
    }

    fn items(&self, sql: &str, from: i64, to: i64) -> Result<Vec<TopItem>> {
        let mut st = self.conn.prepare(&format!("{P}{sql} ORDER BY plays DESC, ms DESC LIMIT {TOP}"))?;
        let rows = st
            .query_map(params![from, to], |r| {
                Ok(TopItem {
                    name: r.get("name")?,
                    id: r.get("id")?,
                    key: r.get("akey")?,
                    by: r.get("who")?,
                    cover_id: r.get("cover_id")?,
                    plays: r.get("plays")?,
                    listened_ms: r.get("ms")?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// ms listened grouped by `strftime(fmt)` (an integer), `n` buckets, `shift` added to the bucket.
    fn buckets(&self, fmt: &str, n: usize, shift: i64, from: i64, to: i64) -> Result<Vec<i64>> {
        let mut out = vec![0; n];
        let mut st = self.conn.prepare(&format!(
            "{P}SELECT CAST(strftime('{fmt}', {LOCAL}) AS INTEGER), SUM(ms_listened) FROM p GROUP BY 1"
        ))?;
        for row in st.query_map(params![from, to], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (b, ms) = row?;
            if let Some(slot) = out.get_mut((b + shift).rem_euclid(n as i64) as usize) {
                *slot += ms;
            }
        }
        Ok(out)
    }

    pub(crate) fn stats_between(&self, from: i64, to: i64) -> Result<Stats<Track>> {
        let (plays, listened_ms, tracks): (u32, i64, u32) = self.conn.query_row(
            &format!("{P}SELECT COUNT(*), COALESCE(SUM(ms_listened), 0), COUNT(DISTINCT track_id) FROM p"),
            params![from, to],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let top_tracks = self.tops("SELECT track_id, COUNT(*), SUM(ms_listened) FROM p GROUP BY track_id", from, to, TOP)?;
        let top_artists = self.items(
            "SELECT a.name AS name, a.id AS id, NULL AS akey, NULL AS who, MAX(p.track_id) AS cover_id, \
                COUNT(*) AS plays, SUM(p.ms_listened) AS ms \
             FROM p JOIN track_artists ta ON ta.track_id = p.track_id AND ta.role <> 'album' \
             JOIN artists a ON a.id = ta.artist_id GROUP BY a.id",
            from,
            to,
        )?;
        let top_albums = self.items(
            "SELECT MAX(t.album) AS name, NULL AS id, t.album_key AS akey, \
                CASE WHEN MAX(t.album_artist) <> '' THEN MAX(t.album_artist) \
                     WHEN COUNT(DISTINCT t.artist) = 1 THEN MAX(t.artist) END AS who, \
                MAX(t.id) AS cover_id, COUNT(*) AS plays, SUM(p.ms_listened) AS ms \
             FROM p JOIN tracks t ON t.id = p.track_id WHERE t.album <> '' GROUP BY t.album_key",
            from,
            to,
        )?;
        let top_genres = self.items(
            "SELECT MAX(t.genre) AS name, NULL AS id, NULL AS akey, NULL AS who, MAX(t.id) AS cover_id, \
                COUNT(*) AS plays, SUM(p.ms_listened) AS ms \
             FROM p JOIN tracks t ON t.id = p.track_id WHERE t.genre <> '' GROUP BY t.genre COLLATE NOCASE",
            from,
            to,
        )?;
        // %w: 0 = Sunday; shift 6 (mod 7) makes Monday 0.
        let by_hour = self.buckets("%H", 24, 0, from, to)?;
        let by_weekday = self.buckets("%w", 7, 6, from, to)?;
        // Imported stats (iTunes...) have no dated plays: those tracks are not new to the owner.
        let disc = "SELECT p.track_id, COUNT(*), SUM(p.ms_listened) FROM p WHERE p.track_id IN \
            (SELECT track_id FROM plays GROUP BY track_id HAVING MIN(played_at) >= ?1 AND MIN(played_at) < ?2) \
            AND p.track_id NOT IN (SELECT track_id FROM imported_stats WHERE plays > 0) GROUP BY p.track_id";
        let discoveries: u32 =
            self.conn.query_row(&format!("{P}SELECT COUNT(*) FROM ({disc})"), params![from, to], |r| r.get(0))?;
        let top_discoveries = self.tops(disc, from, to, TOP)?;
        let days = self
            .conn
            .prepare(&format!(
                "{P}SELECT DISTINCT CAST(julianday(d) AS INTEGER), d FROM \
                 (SELECT date({LOCAL}) AS d FROM p) ORDER BY 1"
            ))?
            .query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Stats {
            from,
            to,
            listened_ms,
            plays,
            tracks,
            top_tracks,
            top_artists,
            top_albums,
            top_genres,
            by_hour,
            by_weekday,
            discoveries,
            top_discoveries,
            streak: longest_streak(&days),
        })
    }

    pub fn wrapped(&self, year: i32) -> Result<Wrapped<Track>> {
        if !(1970..=9998).contains(&year) {
            return Err(Error::Invalid("year out of range"));
        }
        let (from, to) = (self.year_start(year)?, self.year_start(year + 1)?);
        let stats = self.stats_between(from, to)?;
        let by_month = self.buckets("%m", 12, -1, from, to)?;
        let top_month = self
            .conn
            .query_row(
                &format!(
                    "{P}SELECT CAST(strftime('%m', {LOCAL}) AS INTEGER), COUNT(*), SUM(ms_listened) FROM p \
                     GROUP BY 1 ORDER BY 3 DESC, 2 DESC LIMIT 1"
                ),
                params![from, to],
                |r| Ok(MonthStat { month: r.get(0)?, plays: r.get(1)?, listened_ms: r.get(2)? }),
            )
            .optional()?;
        let late_night = self
            .tops(
                &format!(
                    "SELECT track_id, COUNT(*), SUM(ms_listened) FROM p \
                     WHERE CAST(strftime('%H', {LOCAL}) AS INTEGER) < 5 GROUP BY track_id"
                ),
                from,
                to,
                1,
            )?
            .pop();
        let top_verified = self.tops(
            "SELECT p.track_id, COUNT(*), SUM(p.ms_listened) FROM p JOIN tracks t ON t.id = p.track_id \
             WHERE t.quality_verdict = 'real' GROUP BY p.track_id",
            from,
            to,
            5,
        )?;
        let library: u32 = self.conn.query_row("SELECT COUNT(*) FROM tracks WHERE available = 1 AND in_library = 1", [], |r| r.get(0))?;
        let library_pct = if library == 0 { 0.0 } else { (f64::from(stats.tracks) * 100.0 / f64::from(library)).min(100.0) };
        Ok(Wrapped { year, stats, by_month, top_month, late_night, top_verified, library_pct })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fake_track;

    /// Unix seconds of a local time "YYYY-MM-DD HH:MM:SS".
    fn ts(lib: &Library, local: &str) -> i64 {
        lib.conn.query_row("SELECT CAST(strftime('%s', ?1, 'utc') AS INTEGER)", [local], |r| r.get(0)).unwrap()
    }

    fn play(lib: &Library, id: i64, local: &str, ms: i64) {
        lib.conn
            .execute("INSERT INTO plays (track_id, played_at, ms_listened) VALUES (?1, ?2, ?3)", params![id, ts(lib, local), ms])
            .unwrap();
    }

    #[test]
    fn stats_and_wrapped_from_synthetic_plays() {
        let lib = Library::open_in_memory().unwrap();
        let a = fake_track(&lib, "/m/a.flac", "AURORA", "Mora", "album = 'Estrella', album_key = 'e', genre = 'Reggaeton', quality_verdict = 'real'");
        let b = fake_track(&lib, "/m/b.mp3", "DROGA", "Mora & C. Tangana", "album = 'Estrella', album_key = 'e', genre = 'reggaeton'");
        let c = fake_track(&lib, "/m/c.mp3", "CHUOS", "Quevedo", "genre = 'Trap'");
        let _never = fake_track(&lib, "/m/d.mp3", "Nunca", "Nadie", "");

        // 2024: one old play of c (so c is not a 2025 discovery).
        play(&lib, c, "2024-12-30 10:00:00", 60_000);
        // 2025: a streak of 3 days in March, a lone day in May, a late-night play.
        play(&lib, a, "2025-03-10 12:00:00", 200_000);
        play(&lib, a, "2025-03-11 12:00:00", 200_000);
        play(&lib, b, "2025-03-11 18:00:00", 100_000);
        play(&lib, a, "2025-03-12 09:00:00", 200_000);
        play(&lib, c, "2025-05-01 02:30:00", 50_000);
        play(&lib, c, "2025-05-01 03:30:00", 50_000);

        let w = lib.wrapped(2025).unwrap();
        let s = &w.stats;
        assert_eq!((s.plays, s.listened_ms, s.tracks), (6, 800_000, 3));
        assert_eq!(s.top_tracks[0].track.id, a);
        assert_eq!((s.top_tracks[0].plays, s.top_tracks[0].listened_ms), (3, 600_000));
        // Mora: 3 own plays + 1 shared with C. Tangana.
        let artists: Vec<_> = s.top_artists.iter().map(|x| (x.name.as_str(), x.plays)).collect();
        assert_eq!(artists, [("Mora", 4), ("Quevedo", 2), ("C. Tangana", 1)]);
        assert_eq!((s.top_albums[0].name.as_str(), s.top_albums[0].plays), ("Estrella", 4));
        // Genres group case-insensitively.
        assert_eq!((s.top_genres[0].plays, s.top_genres.len()), (4, 2));
        assert_eq!(s.by_hour.iter().sum::<i64>(), 800_000);
        assert_eq!((s.by_hour[12], s.by_hour[2], s.by_hour[3]), (400_000, 50_000, 50_000));
        // 2025-03-10 was a Monday, 2025-05-01 a Thursday.
        assert_eq!((s.by_weekday[0], s.by_weekday[3]), (200_000, 100_000));
        assert_eq!(s.by_weekday.iter().sum::<i64>(), 800_000);
        assert_eq!(s.discoveries, 2);
        assert!(s.top_discoveries.iter().all(|t| t.track.id != c));
        assert_eq!((s.streak.days, s.streak.from.as_deref(), s.streak.to.as_deref()), (3, Some("2025-03-10"), Some("2025-03-12")));

        assert_eq!(w.top_month.as_ref().map(|m| (m.month, m.plays)), Some((3, 4)));
        assert_eq!((w.by_month[2], w.by_month[4]), (700_000, 100_000));
        assert_eq!(w.late_night.as_ref().map(|t| (t.track.id, t.plays)), Some((c, 2)));
        assert_eq!(w.top_verified.iter().map(|t| t.track.id).collect::<Vec<_>>(), [a]);
        assert_eq!(w.library_pct, 75.0);

        // Everything: c's 2024 play counts too, and c becomes a discovery.
        let all = lib.stats(Range::All).unwrap();
        assert_eq!((all.plays, all.discoveries), (7, 3));
        // Nothing this week.
        assert_eq!(lib.stats(Range::Week).unwrap().plays, 0);
        assert!(lib.wrapped(1500).is_err());
        // Mapping keeps the numbers.
        let ids = w.map(&|t: Track| t.id);
        assert_eq!(ids.stats.top_tracks[0].track, a);
    }

    #[test]
    fn streak_picks_longest_run() {
        let d = |v: &[i64]| v.iter().map(|&j| (j, j.to_string())).collect::<Vec<_>>();
        assert_eq!(longest_streak(&d(&[])).days, 0);
        let s = longest_streak(&d(&[1, 2, 4, 5, 6, 9]));
        assert_eq!((s.days, s.from.as_deref(), s.to.as_deref()), (3, Some("4"), Some("6")));
    }
}
