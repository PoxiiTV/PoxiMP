//! Play queue (order, shuffle, repeat) and the glue between the engine and the UI:
//! gapless preload of the next track, play counting, resume positions.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use poximp_engine::{Command, Event, PlayState};
use poximp_library::Library;
use tauri::{AppHandle, Emitter, Manager};

use crate::media::{self, NowPlaying, Update};
use crate::model::{self, Error, PlayerSync, Position, QueueEvent, QueuePage, Repeat, Result, TrackEvent};
use crate::sound::{self, Norm};
use crate::App;

/// A play counts once this much was heard: half the track or 4 minutes, whichever comes first.
const PLAY_MIN_SECS: f64 = 240.0;
/// Tracks longer than this (mixes, audiobooks, podcasts) remember where they were left.
const RESUME_MIN_SECS: f64 = 20.0 * 60.0;
const RESUME_SAVE_EVERY: f64 = 15.0;
/// Closer than this to the end counts as finished (resume forgotten).
const RESUME_END_MARGIN: f64 = 30.0;
/// "Previous" restarts the track instead when past this point.
const PREVIOUS_RESTARTS_AFTER: f64 = 3.0;
/// Leaving a track by hand before this (or before half of it, if shorter) counts as a skip.
const SKIP_BEFORE_SECS: f64 = 30.0;
/// Shortest A-B loop.
const AB_MIN_SECS: f64 = 0.5;
/// Longest sleep timer.
pub const SLEEP_MAX_MINUTES: u32 = 24 * 60;
pub const MAX_QUEUE: usize = 100_000;
/// Tracks per queue edit (add / play next).
pub const MAX_EDIT: usize = 10_000;

#[derive(Debug, Default)]
pub struct Queue {
    pub ids: Vec<i64>,
    /// Play order: positions -> indices into `ids` (identity unless shuffled).
    order: Vec<usize>,
    /// Current position in `order`.
    pos: Option<usize>,
    pub shuffle: bool,
    pub repeat: Repeat,
    seed: u64,
}

impl Queue {
    /// Replaces the queue and makes `start` (index into `ids`) current.
    pub fn set(&mut self, ids: Vec<i64>, start: usize) {
        self.order = (0..ids.len()).collect();
        self.ids = ids;
        self.pos = Some(start);
        if self.shuffle {
            self.reshuffle();
        }
    }

    /// Index into `ids` of the current track.
    pub fn current(&self) -> Option<usize> {
        self.pos.map(|p| self.order[p])
    }

    pub fn id_at(&self, pos: usize) -> i64 {
        self.ids[self.order[pos]]
    }

    pub fn go(&mut self, pos: usize) {
        self.pos = Some(pos);
    }

    /// Position that follows. `auto` = the track ended by itself (repeat one replays it).
    pub fn peek_next(&self, auto: bool) -> Option<usize> {
        let p = self.pos?;
        if auto && self.repeat == Repeat::One {
            Some(p)
        } else if p + 1 < self.order.len() {
            Some(p + 1)
        } else if self.repeat != Repeat::Off {
            Some(0)
        } else {
            None
        }
    }

    pub fn peek_previous(&self) -> Option<usize> {
        let p = self.pos?;
        if p > 0 {
            Some(p - 1)
        } else if self.repeat != Repeat::Off {
            self.order.len().checked_sub(1)
        } else {
            None
        }
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.shuffle = on;
        if on {
            self.reshuffle();
        } else {
            self.pos = self.current();
            self.order = (0..self.ids.len()).collect();
        }
    }

    /// Current track first, the rest in random order (Fisher-Yates, xorshift).
    fn reshuffle(&mut self) {
        if self.seed == 0 {
            self.seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0x9e37_79b9, |d| d.as_nanos() as u64)
                | 1;
        }
        let cur = self.current();
        self.order = cur.into_iter().chain((0..self.ids.len()).filter(|&i| Some(i) != cur)).collect();
        let first = usize::from(cur.is_some());
        for i in (first + 1..self.order.len()).rev() {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            let j = first + (self.seed % (i - first + 1) as u64) as usize;
            self.order.swap(i, j);
        }
        self.pos = cur.map(|_| 0);
    }

    /// Up to `n` ids that play after the current one (for the quality worker).
    pub fn upcoming(&self, n: usize) -> Vec<i64> {
        self.in_order(self.pos.map_or(0, |p| p + 1), n)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Current play position (index into the play order).
    pub fn position(&self) -> Option<usize> {
        self.pos
    }

    /// Up to `n` ids in play order from position `from`.
    pub fn in_order(&self, from: usize, n: usize) -> Vec<i64> {
        self.order.iter().skip(from).take(n).map(|&i| self.ids[i]).collect()
    }

    /// Inserts `new` at play position `at` and at index `idx` of the unshuffled list, so that
    /// turning shuffle off later still leaves them where they make sense.
    fn insert(&mut self, at: usize, idx: usize, new: &[i64]) {
        let k = new.len();
        for o in &mut self.order {
            if *o >= idx {
                *o += k;
            }
        }
        self.ids.splice(idx..idx, new.iter().copied());
        self.order.splice(at..at, idx..idx + k);
        // An empty queue gets a current track (not playing): play starts there.
        self.pos = Some(match self.pos {
            Some(p) if p >= at => p + k,
            Some(p) => p,
            None => 0,
        });
    }

    /// Right after the current track.
    pub fn play_next(&mut self, new: &[i64]) {
        match (self.pos, self.current()) {
            (Some(p), Some(c)) => self.insert(p + 1, c + 1, new),
            _ => self.insert(0, 0, new),
        }
    }

    /// At the end.
    pub fn add(&mut self, new: &[i64]) {
        self.insert(self.order.len(), self.ids.len(), new);
    }

    /// Removes play positions (out of range ones are ignored). True if the current one went:
    /// the track that followed it is current now.
    pub fn remove(&mut self, positions: &[usize]) -> bool {
        let gone: HashSet<usize> = positions.iter().copied().filter(|&p| p < self.order.len()).collect();
        let gone_idx: HashSet<usize> = gone.iter().map(|&p| self.order[p]).collect();
        let mut remap = vec![0; self.ids.len()];
        let mut ids = Vec::with_capacity(self.ids.len() - gone_idx.len());
        for (i, &id) in self.ids.iter().enumerate() {
            if !gone_idx.contains(&i) {
                remap[i] = ids.len();
                ids.push(id);
            }
        }
        let removed_current = self.pos.is_some_and(|p| gone.contains(&p));
        let pos = self.pos.map(|p| p - gone.iter().filter(|&&g| g < p).count());
        self.order = self.order.iter().enumerate().filter(|(p, _)| !gone.contains(p)).map(|(_, &i)| remap[i]).collect();
        self.ids = ids;
        self.pos = pos.filter(|_| !self.order.is_empty()).map(|p| p.min(self.order.len() - 1));
        removed_current
    }

    /// Moves play position `from` to `to` (both < len).
    pub fn move_item(&mut self, from: usize, to: usize) {
        if self.shuffle {
            let i = self.order.remove(from);
            self.order.insert(to, i);
        } else {
            // Unshuffled the order is the identity: move the ids themselves.
            let id = self.ids.remove(from);
            self.ids.insert(to, id);
        }
        self.pos = self.pos.map(|p| match p {
            p if p == from => to,
            p if from < p && p <= to => p - 1,
            p if to <= p && p < from => p + 1,
            p => p,
        });
    }

    /// Everything but the current track.
    pub fn clear(&mut self) {
        match self.current() {
            Some(c) => {
                self.ids = vec![self.ids[c]];
                self.order = vec![0];
                self.pos = Some(0);
            }
            None => {
                self.ids.clear();
                self.order.clear();
                self.pos = None;
            }
        }
    }
}

/// The track the engine has loaded.
pub struct Now {
    pub id: i64,
    duration: f64,
    pub started: Instant,
    /// Seconds actually heard (seeks do not count).
    listened: f64,
    last: f64,
    recorded: bool,
    saved_at: f64,
}

impl Now {
    fn new(t: &poximp_library::Track) -> Self {
        Self {
            id: t.id,
            duration: t.duration_ms as f64 / 1000.0,
            started: Instant::now(),
            listened: 0.0,
            last: 0.0,
            recorded: false,
            saved_at: 0.0,
        }
    }

    /// Left by hand this early: a skip (unless it already counted as a play).
    fn skipped(&self) -> bool {
        !self.recorded && self.last < (self.duration * 0.5).min(SKIP_BEFORE_SECS)
    }
}

/// Sleep timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sleep {
    /// Pause at this unix time (ms).
    At(u64),
    /// Stop when the current track ends (nothing is preloaded meanwhile).
    EndOfTrack,
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What was sent to the engine with `Enqueue`.
struct Next {
    pos: usize,
    id: i64,
    path: String,
}

pub struct Player {
    pub q: Queue,
    pub status: PlayState,
    pub volume: f32,
    pub norm: Norm,
    /// Effects speed the engine last reported (the UI's clock rate).
    pub speed: f64,
    /// Device, bit-perfect and crossfade settings (sound.json).
    pub output: sound::Output,
    pub now: Option<Now>,
    next: Option<Next>,
    /// A `Load` was sent and its `TrackChanged` has not arrived yet.
    loading: bool,
    /// Consecutive files that failed to open; stops skipping after a full lap of the queue.
    fails: usize,
    /// A-B repeat of the current track: (A, B once set), seconds.
    pub ab: Option<(f64, Option<f64>)>,
    pub sleep: Option<Sleep>,
    /// Bumped on every new sleep timer, so an old timer thread does nothing.
    sleep_gen: u64,
}

impl Player {
    pub fn new(volume: f32) -> Self {
        Self {
            q: Queue::default(),
            status: PlayState::Stopped,
            volume,
            norm: Norm::Off,
            speed: 1.0,
            output: sound::Output::default(),
            now: None,
            next: None,
            loading: false,
            fails: 0,
            ab: None,
            sleep: None,
            sleep_gen: 0,
        }
    }
}

/// Forgets or saves the resume point of the track being left.
fn leave(lib: &Library, now: &Now, finished: bool) {
    if now.duration < RESUME_MIN_SECS {
        return;
    }
    let at = if finished || now.last > now.duration - RESUME_END_MARGIN { 0.0 } else { now.last };
    let _ = lib.set_resume_position(now.id, (at * 1000.0) as i64);
}

impl App {
    pub fn lock(&self) -> (std::sync::MutexGuard<'_, Player>, std::sync::MutexGuard<'_, Library>) {
        // Lock order everywhere: player, then library.
        let p = self.player.lock().unwrap_or_else(|e| e.into_inner());
        let l = self.lib.lock().unwrap_or_else(|e| e.into_inner());
        (p, l)
    }

    fn state(&self, p: &Player, lib: &Library) -> model::PlayerState {
        let next = p.q.peek_next(true).and_then(|pos| lib.track_by_id(p.q.id_at(pos)).ok().flatten());
        let locked = self.volume_locked();
        model::PlayerState {
            status: p.status,
            volume: if locked { 1.0 } else { p.volume },
            volume_locked: locked,
            shuffle: p.q.shuffle,
            repeat: p.q.repeat,
            index: p.q.current().map_or(-1, |i| i as i64),
            queue_length: p.q.ids.len(),
            next: next.map(|t| model::track(&t, self.analyzing())),
            ab: p.ab.map(|(a, b)| model::AbState { a, b }),
            sleep: p.sleep.map(|s| model::SleepState {
                ends_at: match s {
                    Sleep::At(at) => Some(at),
                    Sleep::EndOfTrack => None,
                },
                end_of_track: s == Sleep::EndOfTrack,
            }),
        }
    }

    fn current(&self, p: &Player, lib: &Library) -> Option<model::Track> {
        let t = lib.track_by_id(p.now.as_ref()?.id).ok()??;
        Some(model::track(&t, self.analyzing()))
    }

    /// Exclusive output plays at 100 %: the slider is locked meanwhile.
    fn volume_locked(&self) -> bool {
        self.engine.output_info().is_some_and(|o| o.exclusive)
    }

    pub fn emit_state(&self, p: &Player, lib: &Library) {
        let _ = self.app.emit("player:state", self.state(p, lib));
        self.media_playback(p);
    }

    fn media_playback(&self, p: &Player) {
        media::update(&self.app, Update::Playback { status: p.status, secs: p.now.as_ref().map_or(0.0, |n| n.last) });
    }

    fn emit_track(&self, p: &Player, lib: &Library) {
        let t = p.now.as_ref().and_then(|n| lib.track_by_id(n.id).ok().flatten());
        let _ = self.app.emit("player:track", TrackEvent { track: t.as_ref().map(|t| model::track(t, self.analyzing())) });
        let now = t.map(|t| NowPlaying {
            cover: t.cover.as_ref().map(|c| self.covers.join(c)),
            duration: t.duration_ms as f64 / 1000.0,
            title: t.title,
            artist: t.artist,
            album: t.album,
        });
        media::update(&self.app, Update::Track(now));
    }

    pub fn sync(&self) -> PlayerSync {
        let (p, lib) = self.lock();
        let position = p.now.as_ref().map_or(Position { secs: 0.0, duration: 0.0, speed: p.speed }, |n| Position { secs: n.last, duration: n.duration, speed: p.speed });
        PlayerSync { state: self.state(&p, &lib), track: self.current(&p, &lib), position }
    }

    /// Loads queue position `pos` and plays it (resuming long tracks where they were left).
    /// `by_hand` = the user chose to leave the current track (next, another track): may be a skip.
    fn play_at(&self, p: &mut Player, lib: &Library, pos: usize, by_hand: bool) -> Result<()> {
        let t = lib.track_by_id(p.q.id_at(pos))?.ok_or(Error::NotFound)?;
        if let Some(old) = p.now.take() {
            if by_hand && old.skipped() {
                let _ = lib.record_skip(old.id);
            }
            leave(lib, &old, false);
        }
        p.q.go(pos);
        p.next = None;
        p.loading = true;
        p.ab = None;
        let mut now = Now::new(&t);
        self.engine.send(Command::Load { path: PathBuf::from(&t.path), autoplay: true, gain_db: sound::gain_db(&t, p.norm) });
        if now.duration >= RESUME_MIN_SECS {
            if let Ok(Some(ms)) = lib.resume_position(t.id) {
                let at = ms as f64 / 1000.0;
                self.engine.send(Command::Seek(at));
                // Leaving before the first position event must keep (not erase) the resume point.
                now.last = at;
                now.saved_at = at;
            }
        }
        p.now = Some(now);
        let _ = self.wake_quality.send(());
        let _ = self.wake_wave.send(());
        Ok(())
    }

    /// Keeps the engine's preloaded track in line with the queue (gapless).
    fn refresh_next(&self, p: &mut Player, lib: &Library) {
        if p.loading || p.now.is_none() {
            return;
        }
        // "Sleep at the end of this track": nothing follows it.
        let last = p.sleep == Some(Sleep::EndOfTrack);
        let want = p.q.peek_next(true).filter(|_| !last).map(|pos| (pos, p.q.id_at(pos)));
        match (want, p.next.as_mut()) {
            // Same track preloaded; a queue edit may have moved it.
            (Some((pos, id)), Some(n)) if n.id == id => {
                n.pos = pos;
                return;
            }
            (None, None) => return,
            _ => {}
        }
        match want.and_then(|(pos, id)| Some((pos, id, lib.track_by_id(id).ok()??))) {
            Some((pos, id, t)) => {
                let crossfade = if p.output.crossfade > 0.0 {
                    let cur = p.now.as_ref().and_then(|n| lib.track_by_id(n.id).ok().flatten());
                    p.output.crossfade_between(cur.as_ref().map_or("", |c| c.album_key.as_str()), &t.album_key)
                } else {
                    0.0
                };
                self.engine.send(Command::Enqueue { path: PathBuf::from(&t.path), gain_db: sound::gain_db(&t, p.norm), crossfade });
                p.next = Some(Next { pos, id, path: t.path });
            }
            None => {
                if p.next.take().is_some() {
                    self.engine.send(Command::ClearNext);
                }
            }
        }
    }

    // --- commands ---

    pub fn play_ids(&self, ids: Vec<i64>, start: usize) -> Result<()> {
        if ids.is_empty() || ids.len() > 100_000 || start >= ids.len() {
            return Err(Error::Invalid);
        }
        let (mut p, lib) = self.lock();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        if lib.count_existing(&unique)? as usize != unique.len() {
            return Err(Error::NotFound);
        }
        p.q.set(ids, start);
        let pos = p.q.pos.unwrap_or(0);
        self.play_at(&mut p, &lib, pos, true)?;
        self.emit_state(&p, &lib);
        self.emit_queue(&p);
        Ok(())
    }

    /// Plays a whole listing (`ids` in view order) from `start_id` (first track if None).
    pub fn play_view(&self, ids: Vec<i64>, start_id: Option<i64>) -> Result<()> {
        let start = match start_id {
            Some(s) => ids.iter().position(|&i| i == s).ok_or(Error::NotFound)?,
            None => 0,
        };
        self.play_window(ids, start)
    }

    /// `play_ids`, but lists longer than the queue play a window around `start`.
    pub fn play_window(&self, ids: Vec<i64>, start: usize) -> Result<()> {
        if start >= ids.len() {
            return Err(Error::Invalid);
        }
        // ponytail: past MAX_QUEUE only a window plays; page the queue lazily if anyone needs more.
        let lo = start.saturating_sub(MAX_QUEUE / 2).min(ids.len().saturating_sub(MAX_QUEUE));
        let hi = (lo + MAX_QUEUE).min(ids.len());
        self.play_ids(ids[lo..hi].to_vec(), start - lo)
    }

    fn emit_queue(&self, p: &Player) {
        let _ = self.app.emit("player:queue", QueueEvent { total: p.q.len(), index: p.q.position().map_or(-1, |i| i as i64) });
    }

    pub fn queue_page(&self, offset: usize, limit: usize) -> Result<QueuePage> {
        let (p, lib) = self.lock();
        let tracks = lib.tracks_by_ids(&p.q.in_order(offset, limit))?;
        Ok(QueuePage {
            total: p.q.len(),
            index: p.q.position().map_or(-1, |i| i as i64),
            offset,
            tracks: tracks.iter().map(|t| model::track(t, self.analyzing())).collect(),
        })
    }

    /// Runs a queue edit, then keeps the gapless preload in line and tells the UI.
    fn edit_queue(&self, f: impl FnOnce(&mut Player, &Library) -> Result<()>) -> Result<()> {
        let (mut p, lib) = self.lock();
        f(&mut p, &lib)?;
        self.refresh_next(&mut p, &lib);
        self.emit_state(&p, &lib);
        self.emit_queue(&p);
        Ok(())
    }

    /// Ids for queue_add / queue_play_next: known tracks, within the size limits.
    fn check_new(p: &Player, lib: &Library, ids: &[i64]) -> Result<()> {
        if ids.is_empty() || ids.len() > MAX_EDIT || p.q.len() + ids.len() > MAX_QUEUE {
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

    pub fn queue_play_next(&self, ids: Vec<i64>) -> Result<()> {
        self.edit_queue(|p, lib| {
            Self::check_new(p, lib, &ids)?;
            p.q.play_next(&ids);
            Ok(())
        })
    }

    pub fn queue_add(&self, ids: Vec<i64>) -> Result<()> {
        self.edit_queue(|p, lib| {
            Self::check_new(p, lib, &ids)?;
            p.q.add(&ids);
            Ok(())
        })
    }

    /// Removing the track that plays moves on to the one after it (or stops at the end).
    pub fn queue_remove(&self, positions: Vec<usize>) -> Result<()> {
        self.edit_queue(|p, lib| {
            if positions.is_empty() || positions.len() > MAX_QUEUE || positions.iter().any(|&i| i >= p.q.len()) {
                return Err(Error::Invalid);
            }
            if p.q.remove(&positions) && p.now.is_some() {
                match p.q.position() {
                    Some(pos) if p.status == PlayState::Playing => self.play_at(p, lib, pos, false)?,
                    _ => {
                        if let Some(old) = p.now.take() {
                            leave(lib, &old, false);
                        }
                        p.next = None;
                        self.engine.send(Command::Stop);
                        self.emit_track(p, lib);
                    }
                }
            }
            Ok(())
        })
    }

    pub fn queue_move(&self, from: usize, to: usize) -> Result<()> {
        self.edit_queue(|p, _| {
            if from >= p.q.len() || to >= p.q.len() {
                return Err(Error::Invalid);
            }
            p.q.move_item(from, to);
            Ok(())
        })
    }

    pub fn queue_clear(&self) -> Result<()> {
        self.edit_queue(|p, _| {
            p.q.clear();
            Ok(())
        })
    }

    /// Saved resume point of a long track (seconds).
    pub fn resume_get(&self, track_id: i64) -> Result<Option<f64>> {
        let (p, lib) = self.lock();
        if let Some(n) = p.now.as_ref().filter(|n| n.id == track_id && n.duration >= RESUME_MIN_SECS) {
            return Ok((n.last > 0.0).then_some(n.last));
        }
        Ok(lib.resume_position(track_id)?.map(|ms| ms as f64 / 1000.0))
    }

    /// "Start from the beginning": forgets the resume point (and rewinds it if it is playing).
    pub fn resume_clear(&self, track_id: i64) -> Result<()> {
        let (mut p, lib) = self.lock();
        lib.set_resume_position(track_id, 0)?;
        if let Some(n) = p.now.as_mut().filter(|n| n.id == track_id) {
            n.last = 0.0;
            n.saved_at = 0.0;
            self.engine.send(Command::Seek(0.0));
        }
        Ok(())
    }

    pub fn toggle(&self) -> Result<()> {
        let (mut p, lib) = self.lock();
        match (p.status, p.q.pos) {
            // The engine forgets a finished track: reload it through the queue.
            (PlayState::Stopped, Some(pos)) => self.play_at(&mut p, &lib, pos, false),
            _ => {
                self.engine.send(Command::Toggle);
                Ok(())
            }
        }
    }

    pub fn play(&self) -> Result<()> {
        let status = self.player.lock().unwrap_or_else(|e| e.into_inner()).status;
        if status == PlayState::Playing { Ok(()) } else { self.toggle() }
    }

    pub fn pause(&self) -> Result<()> {
        if self.player.lock().unwrap_or_else(|e| e.into_inner()).status == PlayState::Playing {
            self.engine.send(Command::Pause);
        }
        Ok(())
    }

    /// Stop and back to 0. The queue stays: play starts the current track again.
    pub fn stop(&self) -> Result<()> {
        let (mut p, lib) = self.lock();
        p.ab = None;
        p.next = None;
        self.engine.send(Command::Stop);
        self.emit_state(&p, &lib);
        Ok(())
    }

    /// A-B repeat: `a` alone marks A, `a` and `b` loop between them, neither turns it off.
    pub fn ab_set(&self, a: Option<f64>, b: Option<f64>) -> Result<()> {
        let (mut p, lib) = self.lock();
        let duration = p.now.as_ref().map_or(0.0, |n| n.duration);
        let ok = |x: f64| x.is_finite() && x >= 0.0 && (duration <= 0.0 || x <= duration);
        match (a, b) {
            (None, None) => p.ab = None,
            (Some(a), None) if p.now.is_some() && ok(a) => p.ab = Some((a, None)),
            (Some(a), Some(b)) if p.now.is_some() && ok(a) && ok(b) && b - a >= AB_MIN_SECS => p.ab = Some((a, Some(b))),
            _ => return Err(Error::Invalid),
        }
        let looping = p.ab.and_then(|(a, b)| Some((a, b?)));
        self.engine.send(Command::SetLoop(looping));
        self.emit_state(&p, &lib);
        Ok(())
    }

    /// Sleep timer: pause in `minutes`, or stop at the end of the current track; neither = off.
    pub fn sleep_set(&self, minutes: Option<u32>, end_of_track: bool) -> Result<()> {
        if minutes.is_some_and(|m| !(1..=SLEEP_MAX_MINUTES).contains(&m)) || (minutes.is_some() && end_of_track) {
            return Err(Error::Invalid);
        }
        let (mut p, lib) = self.lock();
        p.sleep_gen += 1;
        let gen = p.sleep_gen;
        p.sleep = match minutes {
            Some(m) => Some(Sleep::At(unix_ms() + u64::from(m) * 60_000)),
            None if end_of_track => Some(Sleep::EndOfTrack),
            None => None,
        };
        if let Some(m) = minutes {
            let app = self.app.clone();
            std::thread::Builder::new()
                .name("poximp-sleep".into())
                .spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(u64::from(m) * 60));
                    app.state::<App>().sleep_fire(gen);
                })
                .map_err(|_| Error::Io)?;
        }
        // Back from "end of track": the following track preloads again (and vice versa).
        p.next = None;
        self.engine.send(Command::ClearNext);
        self.refresh_next(&mut p, &lib);
        self.emit_state(&p, &lib);
        Ok(())
    }

    /// A timer ran out: pause, unless it was replaced or switched off meanwhile.
    fn sleep_fire(&self, gen: u64) {
        let (mut p, lib) = self.lock();
        if p.sleep_gen != gen || !matches!(p.sleep, Some(Sleep::At(_))) {
            return;
        }
        p.sleep = None;
        if p.status == PlayState::Playing {
            self.engine.send(Command::Pause);
        }
        self.emit_state(&p, &lib);
    }

    pub fn seek_by(&self, delta: f64) -> Result<()> {
        let (at, duration) = {
            let p = self.player.lock().unwrap_or_else(|e| e.into_inner());
            let Some(n) = &p.now else { return Err(Error::Invalid) };
            (n.last, n.duration)
        };
        let to = at + delta;
        self.seek(if duration > 0.0 { to.clamp(0.0, duration) } else { to.max(0.0) })
    }

    /// New normalization mode: the loaded and the preloaded track get their new gain right away.
    pub fn set_normalization(&self, mode: Norm) {
        let (mut p, lib) = self.lock();
        p.norm = mode;
        let ids = [p.now.as_ref().map(|n| n.id), p.next.as_ref().map(|n| n.id)];
        for t in ids.into_iter().flatten().filter_map(|id| lib.track_by_id(id).ok().flatten()) {
            self.engine.send(Command::SetGain { path: PathBuf::from(&t.path), gain_db: sound::gain_db(&t, mode) });
        }
    }

    pub fn next(&self) -> Result<()> {
        let (mut p, lib) = self.lock();
        match p.q.peek_next(false) {
            Some(pos) => self.play_at(&mut p, &lib, pos, true)?,
            None => return Ok(()),
        }
        self.emit_state(&p, &lib);
        Ok(())
    }

    /// Play the queue item at `pos` (play order) without reordering the queue.
    pub fn queue_jump(&self, pos: usize) -> Result<()> {
        let (mut p, lib) = self.lock();
        if pos >= p.q.len() {
            return Err(Error::Invalid);
        }
        self.play_at(&mut p, &lib, pos, true)?;
        self.emit_state(&p, &lib);
        self.emit_queue(&p);
        Ok(())
    }

    pub fn previous(&self) -> Result<()> {
        let (mut p, lib) = self.lock();
        let at = p.now.as_ref().map_or(0.0, |n| n.last);
        match p.q.peek_previous().filter(|_| at <= PREVIOUS_RESTARTS_AFTER) {
            Some(pos) => self.play_at(&mut p, &lib, pos, false)?,
            None => self.engine.send(Command::Seek(0.0)),
        }
        self.emit_state(&p, &lib);
        Ok(())
    }

    pub fn seek(&self, secs: f64) -> Result<()> {
        let p = self.player.lock().unwrap_or_else(|e| e.into_inner());
        let Some(now) = &p.now else { return Err(Error::Invalid) };
        if !secs.is_finite() || secs < 0.0 || (now.duration > 0.0 && secs > now.duration + 1.0) {
            return Err(Error::Invalid);
        }
        let secs = secs.min(now.duration.max(0.0));
        self.engine.send(Command::Seek(secs));
        media::update(&self.app, Update::Playback { status: p.status, secs });
        Ok(())
    }

    pub fn set_volume(&self, v: f32) -> Result<()> {
        if !(0.0..=1.0).contains(&v) {
            return Err(Error::Invalid);
        }
        let (mut p, lib) = self.lock();
        if self.volume_locked() {
            // Bit-perfect: stays at 100 %; the state tells the UI to snap back.
            self.emit_state(&p, &lib);
            return Ok(());
        }
        p.volume = v;
        self.engine.send(Command::SetVolume(v));
        self.emit_state(&p, &lib);
        Ok(())
    }

    /// New device / bit-perfect / crossfade settings: to the engine, and the preloaded next
    /// track again (its crossfade may have changed).
    pub fn set_output(&self, o: sound::Output) {
        let (mut p, lib) = self.lock();
        self.engine.send(Command::SetOutput { device: o.device_id.clone(), bit_perfect: o.bit_perfect });
        p.output = o;
        p.next = None;
        self.refresh_next(&mut p, &lib);
    }

    pub fn set_shuffle(&self, on: bool) {
        let (mut p, lib) = self.lock();
        p.q.set_shuffle(on);
        self.refresh_next(&mut p, &lib);
        self.emit_state(&p, &lib);
        self.emit_queue(&p);
    }

    pub fn set_repeat(&self, r: Repeat) {
        let (mut p, lib) = self.lock();
        p.q.repeat = r;
        self.refresh_next(&mut p, &lib);
        self.emit_state(&p, &lib);
    }

    // --- engine events ---

    fn on_event(&self, ev: Event) {
        let (mut guard, mut lib) = self.lock();
        let p = &mut *guard;
        match ev {
            Event::StateChanged(s) => {
                p.status = s;
                self.emit_state(p, &lib);
            }
            Event::Position { secs, duration, speed } => {
                p.speed = speed;
                // While loading, positions still belong to the previous track.
                let playing = p.status == PlayState::Playing && !p.loading;
                if let Some(n) = p.now.as_mut().filter(|_| !p.loading) {
                    let d = secs - n.last;
                    if playing && d > 0.0 && d < 1.5 {
                        n.listened += d;
                    }
                    n.last = secs;
                    if !n.recorded && n.listened >= (n.duration * 0.5).min(PLAY_MIN_SECS) {
                        n.recorded = true;
                        let _ = lib.record_play(n.id, (n.listened * 1000.0) as i64);
                    }
                    if n.duration >= RESUME_MIN_SECS && (secs - n.saved_at).abs() >= RESUME_SAVE_EVERY {
                        n.saved_at = secs;
                        leave(&lib, n, false);
                    }
                }
                let _ = self.app.emit("player:position", Position { secs, duration, speed });
            }
            Event::TrackChanged { path } => {
                if p.loading {
                    p.loading = false;
                    p.fails = 0;
                    if let Some(n) = &mut p.now {
                        n.started = Instant::now();
                    }
                } else {
                    // Gapless transition into the preloaded track.
                    if let Some(old) = p.now.take() {
                        leave(&lib, &old, true);
                    }
                    let path = path.to_string_lossy();
                    let pos = match p.next.take() {
                        Some(n) if n.path == path => Some(n.pos),
                        // ponytail: a stale preload played; find it in the queue by id.
                        _ => lib.id_for_path(&path).ok().flatten().and_then(|id| {
                            (0..p.q.ids.len()).find(|&pos| p.q.id_at(pos) == id)
                        }),
                    };
                    if let Some(pos) = pos {
                        p.q.go(pos);
                        p.now = lib.track_by_id(p.q.id_at(pos)).ok().flatten().map(|t| Now::new(&t));
                    }
                    p.ab = None;
                    let _ = self.wake_quality.send(());
                    let _ = self.wake_wave.send(());
                }
                self.emit_track(p, &lib);
                self.refresh_next(p, &lib);
                self.emit_state(p, &lib);
            }
            Event::TrackEnded => {
                if let Some(old) = &p.now {
                    leave(&lib, old, true);
                }
                p.next = None;
                p.ab = None;
                if p.sleep == Some(Sleep::EndOfTrack) {
                    // The sleep timer's "end of this track": stop here.
                    p.sleep = None;
                } else if let Some(pos) = p.q.peek_next(true) {
                    // Only reached when nothing was preloaded (end of queue, or the preload failed).
                    let _ = self.play_at(p, &lib, pos, false);
                }
                self.emit_state(p, &lib);
            }
            Event::OutputChanged => {
                let _ = self.app.emit("player:output", model::output_info(self.engine.output_info()));
                self.emit_state(p, &lib);
            }
            Event::Error(_) => {
                if p.loading {
                    // Unreadable file: skip ahead, but stop once every track in the queue failed.
                    p.loading = false;
                    p.fails += 1;
                    if p.fails < p.q.ids.len() {
                        if let Some(pos) = p.q.peek_next(false) {
                            if self.play_at(p, &lib, pos, false).is_ok() {
                                return;
                            }
                        }
                    }
                    p.fails = 0;
                    p.status = PlayState::Stopped;
                    self.engine.send(Command::Stop);
                    self.emit_track(p, &lib);
                    self.emit_state(p, &lib);
                }
            }
        }
    }
}

/// Forwards engine events into the queue logic and the UI. Runs until the engine is dropped.
pub fn spawn_events(app: AppHandle, rx: crossbeam_channel::Receiver<Event>) {
    std::thread::Builder::new()
        .name("poximp-events".into())
        .spawn(move || {
            for ev in rx {
                app.state::<App>().on_event(ev);
            }
        })
        .expect("spawn events thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(n: i64) -> Queue {
        let mut q = Queue::default();
        q.set((10..10 + n).collect(), 1);
        q
    }

    #[test]
    fn next_previous_and_repeat() {
        let mut q = q(3);
        assert_eq!((q.current(), q.peek_next(true), q.peek_previous()), (Some(1), Some(2), Some(0)));
        q.go(2);
        assert_eq!(q.peek_next(true), None);
        q.repeat = Repeat::All;
        assert_eq!(q.peek_next(true), Some(0));
        q.repeat = Repeat::One;
        assert_eq!((q.peek_next(true), q.peek_next(false)), (Some(2), Some(0)));
        q.go(0);
        assert_eq!(q.peek_previous(), Some(2));
        assert_eq!(q.upcoming(5), [11, 12]);
    }

    fn order(q: &Queue) -> Vec<i64> {
        q.in_order(0, usize::MAX)
    }

    #[test]
    fn queue_edits_unshuffled() {
        let mut q = q(4); // 10 11 12 13, current 11
        q.play_next(&[20, 21]);
        assert_eq!(order(&q), [10, 11, 20, 21, 12, 13]);
        assert_eq!((q.position(), q.peek_next(false).map(|p| q.id_at(p))), (Some(1), Some(20)));
        q.add(&[30]);
        assert_eq!(order(&q), [10, 11, 20, 21, 12, 13, 30]);
        // Moving the current track carries the position with it; others shift around it.
        q.move_item(1, 4);
        assert_eq!((order(&q), q.position()), (vec![10, 20, 21, 12, 11, 13, 30], Some(4)));
        q.move_item(0, 6);
        assert_eq!((order(&q), q.position()), (vec![20, 21, 12, 11, 13, 30, 10], Some(3)));
        q.move_item(6, 0);
        assert_eq!(q.position(), Some(4));
        // Removing the current one makes the next current.
        assert!(q.remove(&[4, 0]));
        assert_eq!((order(&q), q.position()), (vec![20, 21, 12, 13, 30], Some(3)));
        assert!(!q.remove(&[0]));
        assert_eq!((order(&q), q.position().map(|p| q.id_at(p))), (vec![21, 12, 13, 30], Some(13)));
        // Removing the last current one clamps to the end.
        assert!(q.remove(&[2, 3]));
        assert_eq!((order(&q), q.position()), (vec![21, 12], Some(1)));
        q.clear();
        assert_eq!((order(&q), q.position()), (vec![12], Some(0)));
        assert!(q.remove(&[0]));
        assert_eq!((q.len(), q.position()), (0, None));
        // An empty queue gets a current track from the first add.
        q.add(&[1, 2]);
        assert_eq!((order(&q), q.position()), (vec![1, 2], Some(0)));
    }

    #[test]
    fn queue_edits_shuffled_then_unshuffled() {
        let mut q = q(20);
        q.set_shuffle(true);
        q.play_next(&[99]);
        assert_eq!((q.id_at(0), q.id_at(1)), (11, 99));
        q.add(&[98]);
        assert_eq!(q.id_at(q.len() - 1), 98);
        q.move_item(1, 5);
        assert_eq!((q.id_at(5), q.position()), (99, Some(0)));
        let all: HashSet<i64> = order(&q).into_iter().collect();
        assert_eq!(all.len(), 22);
        // Unshuffled: "play next" sits right after the current track, "add" at the end.
        q.set_shuffle(false);
        let o = order(&q);
        assert_eq!((&o[..3], o[o.len() - 1], q.current().map(|c| q.ids[c])), (&[10, 11, 99][..], 98, Some(11)));
        assert!(!q.remove(&[0, 2]));
        // Position 0 (10) and 2 (99) gone: current 11 stays, now first.
        assert_eq!(order(&q)[..2], [11, 12]);
    }

    #[test]
    fn skip_rule() {
        let t = poximp_library::Track { id: 1, duration_ms: 200_000, ..Default::default() };
        let mut n = Now::new(&t);
        n.last = 10.0;
        assert!(n.skipped());
        n.last = 31.0;
        assert!(!n.skipped());
        // Short track: half of it.
        let mut s = Now::new(&poximp_library::Track { duration_ms: 40_000, ..t });
        s.last = 19.0;
        assert!(s.skipped());
        s.last = 21.0;
        assert!(!s.skipped());
        s.last = 1.0;
        s.recorded = true;
        assert!(!s.skipped());
    }

    #[test]
    fn shuffle_keeps_current_first_and_every_track_once() {

        let mut q = q(50);
        q.set_shuffle(true);
        assert_eq!(q.current(), Some(1));
        assert_eq!(q.pos, Some(0));
        let mut seen: Vec<i64> = (0..50).map(|p| q.id_at(p)).collect();
        let unshuffled: Vec<i64> = [11, 10].into_iter().chain(12..60).collect();
        assert_ne!(seen, unshuffled);
        seen.sort_unstable();
        assert_eq!(seen, (10..60).collect::<Vec<_>>());
        q.go(7);
        let cur = q.current();
        q.set_shuffle(false);
        assert_eq!(q.current(), cur);
        assert_eq!(q.pos, cur);
    }
}
