//! Offline radio and mixes: no server, no account. Similarity = sound features (analysis pass 2,
//! z-scored over the library) + shared artists, genre, year and what gets played together.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::{Library, QualityJob, Result};

/// Bump when the feature extractor changes: every track gets analysed again.
pub const FEATURES_REV: u32 = 1;
pub const RADIO_LEN: usize = 50;
const MIX_LEN: usize = 40;
/// Most played tracks the mixes are clustered from.
const MIX_SOURCE: usize = 200;
const MIX_MIN_TRACKS: usize = 12;
const MIX_MIN_CLUSTER: usize = 3;
const MAX_MIXES: usize = 6;
const MIN_MIXES: usize = 3;

const W_SOUND: f32 = 1.0;
const W_ARTIST: f32 = 0.35;
const W_GENRE: f32 = 0.3;
const W_YEAR: f32 = 0.15;
const W_TOGETHER: f32 = 0.4;
/// Mixes: the cluster's own tracks come first, without crowding out discoveries.
const W_MEMBER: f32 = 0.5;
/// Plays this close in time count as "listened together".
const TOGETHER_SECS: i64 = 3600;
/// Candidates kept before diversifying, per track wanted.
const POOL_PER_TRACK: usize = 6;
/// No artist again within this many tracks.
const ARTIST_GAP: usize = 2;
/// Max tracks per artist in one radio / mix.
const MAX_PER_ARTIST: usize = 4;
/// Score noise: ±10 % (mild randomness, the best matches stay near the top).
const JITTER: f32 = 0.2;

/// xorshift64*: deterministic, no crate needed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A library track as similarity sees it.
#[derive(Debug, Clone, Default)]
pub struct Cand {
    pub id: i64,
    pub album: String,
    /// Lowercase ("" = none).
    pub genre: String,
    pub genre_shown: String,
    pub year: Option<i32>,
    pub artists: Vec<i64>,
    pub plays: u32,
    pub duration_ms: i64,
    /// z-scored features.
    pub vec: Option<Vec<f32>>,
}

/// What a radio / mix is looking for.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub vec: Option<Vec<f32>>,
    pub artists: HashSet<i64>,
    pub genres: HashSet<String>,
    pub year: Option<i32>,
    /// Track id -> times played close to the seed.
    pub together: HashMap<i64, u32>,
}

fn dist2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Higher = more alike. Sound counts most; metadata and history add to it.
pub fn score(p: &Profile, c: &Cand) -> f32 {
    let mut s = 0.0;
    if let (Some(a), Some(b)) = (&p.vec, &c.vec) {
        // Mean squared z-distance per dimension: ~2 for unrelated tracks, 0 for twins.
        s += W_SOUND * (-dist2(a, b) / a.len().max(1) as f32 / 2.0).exp();
    }
    if c.artists.iter().any(|a| p.artists.contains(a)) {
        s += W_ARTIST;
    }
    if !c.genre.is_empty() && p.genres.contains(&c.genre) {
        s += W_GENRE;
    }
    if let (Some(a), Some(b)) = (p.year, c.year) {
        s += W_YEAR * (1.0 - (a - b).abs() as f32 / 10.0).max(0.0);
    }
    if let Some(&n) = p.together.get(&c.id) {
        s += W_TOGETHER * (n as f32 / 3.0).min(1.0);
    }
    s
}

/// Picks `len` of the scored candidates: best first with a little noise, never the same track
/// twice, no artist again within `ARTIST_GAP` tracks (`recent` = artists already playing, e.g.
/// the seed's), at most `MAX_PER_ARTIST` per artist and no two in a row from one album.
/// Rules relax only when nothing else is left.
pub fn diversify(mut scored: Vec<(f32, &Cand)>, len: usize, recent: &[i64], rng: &mut Rng) -> Vec<i64> {
    for s in &mut scored {
        s.0 *= 1.0 + JITTER * (rng.unit() - 0.5);
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    let mut seen = HashSet::new();
    scored.retain(|s| seen.insert(s.1.id));
    scored.truncate(len * POOL_PER_TRACK);
    let mut last: Vec<Vec<i64>> = vec![recent.to_vec()];
    let mut per_artist: HashMap<i64, usize> = HashMap::new();
    let mut album = String::new();
    let mut out = Vec::with_capacity(len);
    while out.len() < len && !scored.is_empty() {
        let fresh = |c: &Cand| !c.artists.iter().any(|a| last.iter().rev().take(ARTIST_GAP).any(|l| l.contains(a)));
        let room = |c: &Cand| c.artists.iter().all(|a| per_artist.get(a).copied().unwrap_or(0) < MAX_PER_ARTIST);
        let other_album = |c: &Cand| c.album.is_empty() || c.album != album;
        let i = scored
            .iter()
            .position(|s| fresh(s.1) && room(s.1) && other_album(s.1))
            .or_else(|| scored.iter().position(|s| fresh(s.1)))
            .unwrap_or(0);
        let c = scored.remove(i).1;
        for a in &c.artists {
            *per_artist.entry(*a).or_default() += 1;
        }
        last.push(c.artists.clone());
        album.clone_from(&c.album);
        out.push(c.id);
    }
    out
}

/// An automatic mix for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mix {
    /// Most common genre of its tracks (as tagged), if any.
    pub genre: Option<String>,
    /// Its most played artists (up to 3).
    pub artists: Vec<String>,
    pub track_ids: Vec<i64>,
    /// Covers for the tile (up to 4 track ids).
    pub cover_ids: Vec<i64>,
    pub duration_ms: i64,
}

/// Farthest-first start + a few Lloyd rounds. Deterministic. Returns the cluster of each point.
pub fn kmeans(points: &[&[f32]], k: usize) -> Vec<usize> {
    if points.is_empty() || k == 0 {
        return Vec::new();
    }
    let mut cents: Vec<Vec<f32>> = vec![points[0].to_vec()];
    while cents.len() < k.min(points.len()) {
        let far = points
            .iter()
            .max_by(|a, b| {
                let da = cents.iter().map(|c| dist2(a, c)).fold(f32::MAX, f32::min);
                let db = cents.iter().map(|c| dist2(b, c)).fold(f32::MAX, f32::min);
                da.total_cmp(&db)
            })
            .map(|p| p.to_vec());
        cents.extend(far);
    }
    let nearest = |p: &[f32], cents: &[Vec<f32>]| (0..cents.len()).min_by(|&a, &b| dist2(p, &cents[a]).total_cmp(&dist2(p, &cents[b]))).unwrap_or(0);
    let mut of: Vec<usize> = points.iter().map(|p| nearest(p, &cents)).collect();
    for _ in 0..12 {
        for (c, cent) in cents.iter_mut().enumerate() {
            let members: Vec<&&[f32]> = points.iter().zip(&of).filter(|(_, &o)| o == c).map(|(p, _)| p).collect();
            if members.is_empty() {
                continue;
            }
            for (d, v) in cent.iter_mut().enumerate() {
                *v = members.iter().map(|p| p[d]).sum::<f32>() / members.len() as f32;
            }
        }
        let next: Vec<usize> = points.iter().map(|p| nearest(p, &cents)).collect();
        if next == of {
            break;
        }
        of = next;
    }
    of
}

pub fn encode(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn decode(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

/// z-scores every vector of the most common length in place (the others are dropped).
fn normalize(cands: &mut [Cand]) {
    let mut lens: HashMap<usize, usize> = HashMap::new();
    for v in cands.iter().filter_map(|c| c.vec.as_ref()) {
        *lens.entry(v.len()).or_default() += 1;
    }
    let Some((dims, n)) = lens.into_iter().max_by_key(|&(l, n)| (n, l)) else { return };
    let (mut mean, mut var) = (vec![0f64; dims], vec![0f64; dims]);
    for v in cands.iter().filter_map(|c| c.vec.as_ref()).filter(|v| v.len() == dims) {
        for (m, x) in mean.iter_mut().zip(v) {
            *m += *x as f64 / n as f64;
        }
    }
    for v in cands.iter().filter_map(|c| c.vec.as_ref()).filter(|v| v.len() == dims) {
        for ((s, x), m) in var.iter_mut().zip(v).zip(&mean) {
            *s += (*x as f64 - m).powi(2) / n as f64;
        }
    }
    for c in cands.iter_mut() {
        c.vec = c.vec.take().filter(|v| v.len() == dims && v.iter().all(|x| x.is_finite())).map(|v| {
            v.iter().zip(&mean).zip(&var).map(|((x, m), s)| if *s > 1e-12 { ((*x as f64 - m) / s.sqrt()) as f32 } else { 0.0 }).collect()
        });
    }
}

impl Library {
    /// Tracks without features for their current file (most played first: the mixes need them).
    pub fn tracks_needing_features(&self, limit: u32) -> Result<Vec<QualityJob>> {
        let mut st = self.conn.prepare_cached(
            "SELECT t.id, t.path, ?1 || ':' || t.size || ':' || t.mtime AS k FROM tracks t \
             LEFT JOIN track_features f ON f.track_id = t.id \
             WHERE t.available = 1 AND t.in_library = 1 AND f.feat_key IS NOT (?1 || ':' || t.size || ':' || t.mtime) \
             ORDER BY t.play_count DESC, t.id LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![FEATURES_REV, limit], |r| Ok(QualityJob { id: r.get(0)?, path: r.get(1)?, key: r.get(2)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// `vec` None = the file could not be analysed (not retried until it changes).
    pub fn set_features(&self, track_id: i64, key: &str, vec: Option<&[f32]>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO track_features (track_id, feat_key, vec) VALUES (?1, ?2, ?3) \
             ON CONFLICT(track_id) DO UPDATE SET feat_key = excluded.feat_key, vec = excluded.vec",
            params![track_id, key, vec.map(encode)],
        )?;
        Ok(())
    }

    /// ponytail: loads the whole library per radio / mixes call (~100 ms at 100k tracks); cache it if it shows.
    fn candidates(&self) -> Result<Vec<Cand>> {
        let mut artists: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut st = self.conn.prepare_cached(
            "SELECT ta.track_id, ta.artist_id FROM track_artists ta JOIN tracks t ON t.id = ta.track_id \
             WHERE t.available = 1 AND t.in_library = 1 AND ta.role <> 'album' ORDER BY ta.track_id, ta.position",
        )?;
        for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (t, a) = r?;
            artists.entry(t).or_default().push(a);
        }
        let mut st = self.conn.prepare_cached(
            "SELECT t.id, t.album_key, t.genre, t.year, t.play_count, t.duration_ms, f.vec FROM tracks t \
             LEFT JOIN track_features f ON f.track_id = t.id WHERE t.available = 1 AND t.in_library = 1",
        )?;
        let mut cands = st
            .query_map([], |r| {
                let genre: String = r.get(2)?;
                let id = r.get(0)?;
                Ok(Cand {
                    id,
                    album: r.get(1)?,
                    genre: genre.trim().to_lowercase(),
                    genre_shown: genre.trim().to_string(),
                    year: r.get(3)?,
                    plays: r.get(4)?,
                    duration_ms: r.get(5)?,
                    vec: r.get::<_, Option<Vec<u8>>>(6)?.map(|b| decode(&b)),
                    artists: artists.remove(&id).unwrap_or_default(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        normalize(&mut cands);
        Ok(cands)
    }

    /// The seed, then up to `len - 1` tracks like it.
    pub fn radio(&self, seed: i64, len: usize, rng: &mut Rng) -> Result<Vec<i64>> {
        let cands = self.candidates()?;
        let s = cands.iter().find(|c| c.id == seed).ok_or(crate::Error::NotFound)?;
        let mut together = HashMap::new();
        let mut st = self.conn.prepare_cached(
            "SELECT p2.track_id, COUNT(*) FROM plays p1 JOIN plays p2 \
               ON p2.played_at BETWEEN p1.played_at - ?2 AND p1.played_at + ?2 AND p2.track_id <> p1.track_id \
             WHERE p1.track_id = ?1 GROUP BY p2.track_id",
        )?;
        for r in st.query_map(params![seed, TOGETHER_SECS], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u32>(1)?)))? {
            let (t, n) = r?;
            together.insert(t, n);
        }
        let p = Profile {
            vec: s.vec.clone(),
            artists: s.artists.iter().copied().collect(),
            genres: (!s.genre.is_empty()).then(|| s.genre.clone()).into_iter().collect(),
            year: s.year,
            together,
        };
        let scored = cands.iter().filter(|c| c.id != seed).map(|c| (score(&p, c), c)).collect();
        let mut out = vec![seed];
        out.extend(diversify(scored, len.saturating_sub(1), &s.artists, rng));
        Ok(out)
    }

    /// 3-6 mixes from clusters of the most played tracks (needs their features). Empty until
    /// there is enough history. Same `rng` = same mixes.
    pub fn mixes(&self, rng: &mut Rng) -> Result<Vec<Mix>> {
        let cands = self.candidates()?;
        let mut top: Vec<&Cand> = cands.iter().filter(|c| c.plays > 0 && c.vec.is_some()).collect();
        top.sort_by(|a, b| b.plays.cmp(&a.plays).then(a.id.cmp(&b.id)));
        top.truncate(MIX_SOURCE);
        if top.len() < MIX_MIN_TRACKS {
            return Ok(Vec::new());
        }
        let k = (top.len() / 20).clamp(MIN_MIXES, MAX_MIXES);
        let points: Vec<&[f32]> = top.iter().filter_map(|c| c.vec.as_deref()).collect();
        let of = kmeans(&points, k);
        let mut mixes = Vec::new();
        for cluster in 0..k {
            let members: Vec<&Cand> = top.iter().zip(&of).filter(|(_, &o)| o == cluster).map(|(c, _)| *c).collect();
            if members.len() < MIX_MIN_CLUSTER {
                continue;
            }
            let dims = points[0].len();
            let centroid: Vec<f32> = (0..dims).map(|d| members.iter().filter_map(|m| m.vec.as_ref()).map(|v| v[d]).sum::<f32>() / members.len() as f32).collect();
            let mut years: Vec<i32> = members.iter().filter_map(|m| m.year).collect();
            years.sort_unstable();
            let p = Profile {
                vec: Some(centroid),
                artists: members.iter().flat_map(|m| m.artists.iter().copied()).collect(),
                genres: members.iter().filter(|m| !m.genre.is_empty()).map(|m| m.genre.clone()).collect(),
                year: years.get(years.len() / 2).copied(),
                together: HashMap::new(),
            };
            let ids: HashSet<i64> = members.iter().map(|m| m.id).collect();
            let scored = cands.iter().map(|c| (score(&p, c) + if ids.contains(&c.id) { W_MEMBER } else { 0.0 }, c)).collect();
            let track_ids = diversify(scored, MIX_LEN, &[], rng);
            let by_id: HashMap<i64, &Cand> = cands.iter().map(|c| (c.id, c)).collect();
            mixes.push(Mix {
                genre: most_common(members.iter().filter(|m| !m.genre.is_empty()).map(|m| (m.genre.as_str(), m.genre_shown.as_str(), m.plays))),
                artists: self.artist_names(&top_artists(&members))?,
                cover_ids: members.iter().take(4).map(|m| m.id).collect(),
                duration_ms: track_ids.iter().filter_map(|i| by_id.get(i)).map(|c| c.duration_ms).sum(),
                track_ids,
            });
        }
        Ok(mixes)
    }

    fn artist_names(&self, ids: &[i64]) -> Result<Vec<String>> {
        let mut st = self.conn.prepare_cached("SELECT name FROM artists WHERE id = ?1")?;
        let mut out = Vec::new();
        for id in ids {
            out.extend(st.query_row([id], |r| r.get::<_, String>(0)).optional()?);
        }
        Ok(out)
    }
}

/// Up to 3 artists by summed plays of the cluster's tracks.
fn top_artists(members: &[&Cand]) -> Vec<i64> {
    let mut plays: HashMap<i64, u32> = HashMap::new();
    for m in members {
        for a in &m.artists {
            *plays.entry(*a).or_default() += m.plays;
        }
    }
    let mut v: Vec<(i64, u32)> = plays.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().take(3).map(|(a, _)| a).collect()
}

/// (key, shown, weight) -> the shown text of the heaviest key.
fn most_common<'a>(items: impl Iterator<Item = (&'a str, &'a str, u32)>) -> Option<String> {
    let mut w: HashMap<&str, (u32, &str)> = HashMap::new();
    for (k, shown, n) in items {
        let e = w.entry(k).or_insert((0, shown));
        e.0 += n.max(1);
    }
    w.into_iter().max_by(|a, b| a.1 .0.cmp(&b.1 .0).then(b.0.cmp(a.0))).map(|(_, (_, s))| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fake_track;

    fn cand(id: i64, artist: i64, album: &str) -> Cand {
        Cand { id, artists: vec![artist], album: album.into(), ..Default::default() }
    }

    #[test]
    fn score_weighs_sound_then_metadata() {
        let p = Profile {
            vec: Some(vec![0.0; 4]),
            artists: [1].into(),
            genres: ["rock".to_string()].into(),
            year: Some(2000),
            together: [(9, 6)].into(),
        };
        let near = Cand { vec: Some(vec![0.1; 4]), ..Default::default() };
        let far = Cand { vec: Some(vec![2.0; 4]), ..Default::default() };
        assert!(score(&p, &near) > 0.9 && score(&p, &far) < 0.2, "{} {}", score(&p, &near), score(&p, &far));
        let base = score(&p, &far);
        let artist = Cand { artists: vec![1], ..far.clone() };
        let genre = Cand { genre: "rock".into(), ..far.clone() };
        let year = Cand { year: Some(2003), ..far.clone() };
        let together = Cand { id: 9, ..far.clone() };
        assert!((score(&p, &artist) - base - W_ARTIST).abs() < 1e-6);
        assert!((score(&p, &genre) - base - W_GENRE).abs() < 1e-6);
        assert!((score(&p, &year) - base - W_YEAR * 0.7).abs() < 1e-6);
        assert!((score(&p, &together) - base - W_TOGETHER).abs() < 1e-6, "capped at 1");
        assert_eq!(score(&Profile::default(), &near), 0.0, "nothing to compare");
    }

    #[test]
    fn diversify_spreads_artists_and_never_repeats() {
        // 6 artists, 10 tracks each, artist 0 scoring best.
        let cands: Vec<Cand> = (0..60).map(|i| cand(i, i % 6, &format!("al{}", i % 12))).collect();
        let scored: Vec<(f32, &Cand)> = cands.iter().map(|c| (if c.artists[0] == 0 { 2.0 } else { 1.0 } - c.id as f32 * 0.001, c)).collect();
        let out = diversify(scored.clone(), 20, &[0], &mut Rng::new(7));
        assert_eq!(out.len(), 20);
        assert_eq!(out.iter().collect::<HashSet<_>>().len(), 20, "no repeats");
        let artist = |id: i64| cands[id as usize].artists[0];
        assert_ne!(artist(out[0]), 0, "the seed's artist is not next");
        for w in out.windows(3) {
            assert!(artist(w[0]) != artist(w[1]) && artist(w[0]) != artist(w[2]), "{out:?}");
        }
        for a in 0..6 {
            assert!(out.iter().filter(|&&i| artist(i) == a).count() <= MAX_PER_ARTIST);
        }
        assert!(out[..6].iter().any(|&i| artist(i) == 0), "the best artist still comes early");
        assert_eq!(out, diversify(scored.clone(), 20, &[0], &mut Rng::new(7)), "same seed, same radio");
        assert_ne!(out, diversify(scored.clone(), 20, &[0], &mut Rng::new(99)), "another seed varies it");
        // One artist only: the rules relax instead of returning less.
        let solo: Vec<Cand> = (0..5).map(|i| cand(i, 1, "x")).collect();
        let out = diversify(solo.iter().map(|c| (1.0, c)).collect(), 10, &[1], &mut Rng::new(1));
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn kmeans_finds_groups() {
        let pts: Vec<Vec<f32>> = (0..30).map(|i| vec![(i / 10) as f32 * 10.0 + (i % 10) as f32 * 0.1, 0.0]).collect();
        let refs: Vec<&[f32]> = pts.iter().map(|p| p.as_slice()).collect();
        let of = kmeans(&refs, 3);
        for g in 0..3 {
            let first = of[g * 10];
            assert!(of[g * 10..g * 10 + 10].iter().all(|&o| o == first), "{of:?}");
        }
        assert_eq!(of.iter().collect::<HashSet<_>>().len(), 3);
    }

    #[test]
    fn radio_and_mixes_from_the_library() {
        let lib = Library::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for i in 0..40 {
            let (artist, genre) = (format!("Artist {}", i % 8), if i % 2 == 0 { "Rock" } else { "Jazz" });
            let id = fake_track(&lib, &format!("/m/{i}.flac"), &format!("T{i}"), &artist, &format!("genre = '{genre}', play_count = {}", 40 - i));
            // Two sound families, matching the genres.
            let v = if i % 2 == 0 { [1.0, 0.0, 0.2] } else { [0.0, 1.0, 0.8] };
            let v: Vec<f32> = v.iter().map(|x| x + i as f32 * 0.001).collect();
            lib.set_features(id, "k", Some(&v)).unwrap();
            ids.push(id);
        }
        let radio = lib.radio(ids[0], 15, &mut Rng::new(3)).unwrap();
        assert_eq!(radio[0], ids[0]);
        assert_eq!(radio.len(), 15);
        let rock = radio[1..].iter().filter(|id| ids.iter().position(|x| x == *id).unwrap() % 2 == 0).count();
        assert!(rock >= 12, "mostly the seed's family: {rock}/14");
        assert!(lib.radio(9999, 10, &mut Rng::new(1)).is_err());

        let mixes = lib.mixes(&mut Rng::new(5)).unwrap();
        assert!((MIN_MIXES..=MAX_MIXES).contains(&mixes.len()), "{}", mixes.len());
        for m in &mixes {
            assert!(!m.track_ids.is_empty() && m.cover_ids.len() <= 4 && m.artists.len() <= 3);
            assert!(m.genre.as_deref() == Some("Rock") || m.genre.as_deref() == Some("Jazz"));
            assert!(m.duration_ms > 0);
        }
        assert_eq!(lib.tracks_needing_features(5).unwrap().len(), 5, "fake key 'k' is not the current one");
        lib.set_features(ids[0], &format!("{FEATURES_REV}:0:0"), None).unwrap();
        assert!(lib.tracks_needing_features(100).unwrap().iter().all(|j| j.id != ids[0]), "unreadable is not retried");
    }

    #[test]
    fn few_plays_no_mixes() {
        let lib = Library::open_in_memory().unwrap();
        let id = fake_track(&lib, "/m/a.flac", "A", "B", "play_count = 3");
        lib.set_features(id, "k", Some(&[1.0, 2.0])).unwrap();
        assert!(lib.mixes(&mut Rng::new(1)).unwrap().is_empty());
    }
}
