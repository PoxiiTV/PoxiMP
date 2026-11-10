//! Engine thread: owns the output and the decks, fills the ring, emits events.

use std::collections::VecDeque;
use std::f64::consts::FRAC_PI_2;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::*;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::decode::Deck;
use crate::eq::{db_to_lin, Dsp};
use crate::output::{self, Format, Output, Shared};
use crate::{Command, Event, OutputInfo, PlayState};

const POSITION_EVERY: Duration = Duration::from_millis(250);
const DEVICE_CHECK_EVERY: Duration = Duration::from_secs(1);
const FLUSH_TIMEOUT: Duration = Duration::from_millis(250);

pub(crate) fn spawn(rx: Receiver<Command>, events: Sender<Event>, shared: Arc<Shared>, info: Arc<Mutex<Option<OutputInfo>>>) {
    // ponytail: normal thread priority; raise it (audio_thread_priority) if underruns show up under load.
    thread::Builder::new()
        .name("poximp-engine".into())
        .spawn(move || Producer::new(rx, events, shared, info).run())
        .expect("spawn engine thread");
}

/// The track the listener hears (the decoder may already be in the next one).
struct Track {
    path: PathBuf,
    duration: f64,
    /// Output frame (since the last flush) where this track starts.
    start_frame: u64,
    start_secs: f64,
    gain_db: f64,
    format: Format,
    /// Source seconds per output second
    speed: f64,
}

/// A track boundary: when the output passes `frame`, `path` becomes audible.
struct Marker {
    frame: u64,
    path: PathBuf,
    duration: f64,
    gain_db: f64,
    format: Format,
    speed: f64,
}

impl Marker {
    fn of(frame: u64, d: &Deck) -> Self {
        Self { frame, path: d.path.clone(), duration: d.duration, gain_db: d.gain_db, format: d.format(), speed: d.speed }
    }
}

/// A crossfade in progress: `deck` comes in over `len` output frames while the current one goes.
struct Fade {
    deck: Deck,
    len: u64,
    pos: u64,
    /// Frames into the current chunk where the fade begins (only the first chunk).
    start: u64,
    /// Decoded audio of `deck` not mixed yet.
    buf: Vec<f32>,
}

/// Equal-power gains (outgoing, incoming) at `pos` of a `len`-frame crossfade.
pub(crate) fn crossfade_gains(pos: u64, len: u64) -> (f64, f64) {
    if pos >= len {
        return (0.0, 1.0);
    }
    let a = pos as f64 / len as f64 * FRAC_PI_2;
    (a.cos(), a.sin())
}

/// Where a crossfade of `want` frames goes in the chunk just decoded: (start frame in the chunk,
/// length). `left` = frames of the outgoing track after this chunk, `chunk` = frames in it.
/// None = not yet. The fade always ends where the outgoing track does.
pub(crate) fn fade_window(want: u64, left: u64, chunk: u64) -> Option<(u64, u64)> {
    if want == 0 || left >= want {
        return None;
    }
    let before = left + chunk;
    let len = want.min(before);
    (len > 0).then_some((before - len, len))
}

struct Producer {
    rx: Receiver<Command>,
    events: Sender<Event>,
    shared: Arc<Shared>,
    info: Arc<Mutex<Option<OutputInfo>>>,
    out: Option<Box<dyn Output>>,
    ring: Option<rtrb::Producer<f32>>,
    rate: u32,
    dsp: Dsp,
    /// Effects speed (1 = normal); bit-perfect plays at 1 whatever it says.
    speed: f64,
    output_error_sent: bool,
    /// Chosen device (cpal id; None = follow the system default).
    device: Option<String>,
    /// Exclusive at each file's own format, nothing touching the samples.
    bit_perfect: bool,
    /// Format the open output was asked for (bit-perfect), None = shared mode.
    open_for: Option<Format>,
    cur: Option<Deck>,
    next: Option<Deck>,
    fade: Option<Fade>,
    /// Bit-perfect: the next file needs another format. It starts once the ring has drained
    /// and the device reopened (a short gap, as in foobar2000 or AIMP).
    switch: Option<Deck>,
    pending: Vec<f32>,
    pending_off: usize,
    /// Frames pushed into the ring since the last flush.
    pushed: u64,
    markers: VecDeque<Marker>,
    track: Option<Track>,
    /// A-B repeat of the audible track (seconds): decoding past B carries on from A.
    ab: Option<(f64, f64)>,
    /// Output frames where an A-B jump becomes audible, and the position it lands on.
    jumps: VecDeque<(u64, f64)>,
    state: PlayState,
    last_position: Instant,
    last_device_check: Instant,
}

impl Producer {
    fn new(rx: Receiver<Command>, events: Sender<Event>, shared: Arc<Shared>, info: Arc<Mutex<Option<OutputInfo>>>) -> Self {
        let now = Instant::now();
        Self {
            rx,
            events,
            rate: shared.rate.load(Relaxed),
            dsp: Dsp::new(shared.rate.load(Relaxed)),
            speed: 1.0,
            shared,
            info,
            out: None,
            ring: None,
            output_error_sent: false,
            device: None,
            bit_perfect: false,
            open_for: None,
            cur: None,
            next: None,
            fade: None,
            switch: None,
            pending: Vec::new(),
            pending_off: 0,
            pushed: 0,
            markers: VecDeque::new(),
            track: None,
            ab: None,
            jumps: VecDeque::new(),
            state: PlayState::Stopped,
            last_position: now,
            last_device_check: now,
        }
    }

    fn run(mut self) {
        self.rebuild_output();
        loop {
            let wait = if self.state == PlayState::Playing { Duration::from_millis(5) } else { Duration::from_millis(50) };
            match self.rx.recv_timeout(wait) {
                Ok(cmd) => {
                    self.handle(cmd);
                    while let Ok(cmd) = self.rx.try_recv() {
                        self.handle(cmd);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.fill();
            self.tick();
        }
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Load { path, autoplay, gain_db } => self.load(path, autoplay, gain_db),
            Command::Enqueue { path, gain_db, crossfade } => self.enqueue(path, gain_db, crossfade),
            Command::SetGain { path, gain_db } => {
                let fading = self.fade.as_mut().map(|f| &mut f.deck);
                for d in [self.cur.as_mut(), self.next.as_mut(), fading, self.switch.as_mut()].into_iter().flatten().filter(|d| d.path == path) {
                    d.gain_db = gain_db;
                }
                for m in self.markers.iter_mut().filter(|m| m.path == path) {
                    m.gain_db = gain_db;
                }
                if let Some(t) = self.track.as_mut().filter(|t| t.path == path) {
                    t.gain_db = gain_db;
                }
            }
            Command::SetEq(eq) => self.dsp.set_eq(eq),
            Command::SetFx(fx) => {
                self.dsp.set_fx(fx);
                let speed = self.dsp.fx().speed;
                if speed != self.speed {
                    self.speed = speed;
                    self.respeed();
                }
            }
            Command::ClearNext => {
                self.next = None;
                // Only set while the current file is fully decoded: it was the "next" one.
                self.switch = None;
            }
            Command::Play => self.play(),
            Command::Pause => {
                if self.state == PlayState::Playing {
                    self.set_state(PlayState::Paused);
                    self.emit_position();
                }
            }
            Command::Toggle => match self.state {
                PlayState::Playing => self.handle(Command::Pause),
                _ => self.play(),
            },
            Command::Stop => self.stop(),
            Command::Seek(secs) => self.seek(secs),
            Command::SetLoop(ab) => self.set_loop(ab),
            Command::SetVolume(v) => self.shared.volume.store(output::volume_gain(v).to_bits(), Relaxed),
            Command::SetOutput { device, bit_perfect } => {
                let moved = device != self.device;
                self.device = device;
                self.bit_perfect = bit_perfect;
                // Bit-perfect toggled while stopped changes nothing yet: the next play opens it.
                if moved || self.wanted() != self.open_for {
                    self.rebuild_output();
                }
            }
        }
    }

    fn set_state(&mut self, s: PlayState) {
        self.shared.playing.store(s == PlayState::Playing, Relaxed);
        if s != self.state {
            self.state = s;
            self.emit(Event::StateChanged(s));
        }
    }

    fn exclusive_active(&self) -> bool {
        self.out.as_ref().is_some_and(|o| o.info().exclusive)
    }

    /// Speed decks open at now. Bit-perfect never resamples.
    fn speed(&self) -> f64 {
        if self.bit_perfect { 1.0 } else { self.speed }
    }

    /// Output rate of a deck at `speed`: fewer frames per source second play it faster
    /// (pitch follows, like a turntable).
    fn deck_rate(&self, speed: f64) -> u32 {
        (f64::from(self.rate) / speed).round() as u32
    }

    fn open(&self, path: &Path, gain_db: f64) -> Result<Deck, &'static str> {
        let speed = self.speed();
        let mut d = Deck::open(path, self.deck_rate(speed))?;
        d.gain_db = gain_db;
        d.speed = speed;
        Ok(d)
    }

    /// Forgets everything queued after the current deck.
    fn clear_following(&mut self) {
        self.markers.clear();
        self.next = None;
        self.fade = None;
        self.dsp.fading = false;
        self.switch = None;
    }

    fn load(&mut self, path: PathBuf, autoplay: bool, gain_db: f64) {
        let mut deck = match self.open(&path, gain_db) {
            Ok(d) => d,
            Err(code) => return self.emit(Event::Error(code.into())),
        };
        self.flush();
        self.clear_following();
        self.ab = None;
        let format = deck.format();
        if self.bit_perfect && self.open_for != Some(format) {
            self.open_output(Some(format));
            deck.set_out_rate(self.rate);
        }
        let speed = deck.speed;
        self.track = Some(Track { path: path.clone(), duration: deck.duration, start_frame: 0, start_secs: 0.0, gain_db, format, speed });
        self.cur = Some(deck);
        self.emit(Event::TrackChanged { path });
        self.set_state(if autoplay { PlayState::Playing } else { PlayState::Paused });
        self.emit_position();
    }

    fn enqueue(&mut self, path: PathBuf, gain_db: f64, crossfade: f64) {
        let mut deck = match self.open(&path, gain_db) {
            Ok(d) => d,
            Err(code) => return self.emit(Event::Error(code.into())),
        };
        deck.crossfade = if crossfade.is_finite() { crossfade.max(0.0) } else { 0.0 };
        if self.cur.is_none() && self.state != PlayState::Stopped {
            // The current track is fully decoded and draining: continue straight after it.
            self.switch = None;
            self.chain(deck);
        } else {
            self.next = Some(deck);
        }
    }

    /// Continues into `d` right after everything decoded so far: in the same ring (gapless), or
    /// in bit-perfect mode with another format, after the ring drains and the device reopens.
    fn chain(&mut self, d: Deck) {
        if self.bit_perfect && self.open_for.is_some_and(|f| f != d.format()) {
            self.switch = Some(d);
        } else {
            let unpushed = ((self.pending.len() - self.pending_off) / 2) as u64;
            self.markers.push_back(Marker::of(self.pushed + unpushed, &d));
            self.cur = Some(d);
        }
    }

    fn play(&mut self) {
        match self.state {
            PlayState::Playing => {}
            PlayState::Paused => self.set_state(PlayState::Playing),
            PlayState::Stopped => {
                if let Some((path, gain)) = self.track.as_ref().map(|t| (t.path.clone(), t.gain_db)) {
                    self.load(path, true, gain);
                }
            }
        }
    }

    fn stop(&mut self) {
        if self.state == PlayState::Stopped {
            return;
        }
        self.set_state(PlayState::Stopped);
        self.flush();
        self.cur = None;
        self.clear_following();
        self.ab = None;
        // Stopped = back at the start (play loads the track again from 0).
        if let Some(t) = &mut self.track {
            t.start_frame = 0;
            t.start_secs = 0.0;
        }
        if self.open_for.is_some() {
            // Bit-perfect holds the device only while playing or paused.
            self.open_output(None);
        }
        self.emit_position();
    }

    /// New A-B loop. Set where the listener is (B = now), the decoder is already past B: the
    /// loop starts at once.
    fn set_loop(&mut self, ab: Option<(f64, f64)>) {
        self.ab = ab.filter(|&(a, b)| a.is_finite() && b.is_finite() && a >= 0.0 && b > a);
        let Some((a, b)) = self.ab else { return };
        let decoded = self
            .cur
            .as_ref()
            .filter(|_| self.markers.is_empty())
            .map_or(0.0, |d| d.out_frames as f64 * d.speed / f64::from(self.rate));
        if decoded > b || self.position() > b {
            self.seek(a);
        }
    }

    /// Ends a crossfade or a pending format switch at once (seek, device rebuild): the incoming
    /// deck becomes the current one, or the next one.
    fn settle(&mut self) {
        if let Some(f) = self.fade.take() {
            self.cur = Some(f.deck);
            self.dsp.fading = false;
        }
        if let Some(d) = self.switch.take() {
            self.next = Some(d);
        }
    }

    fn seek(&mut self, secs: f64) {
        if self.state == PlayState::Stopped {
            return;
        }
        let Some((path, gain)) = self.track.as_ref().map(|t| (t.path.clone(), t.gain_db)) else { return };
        self.settle();
        if self.cur.is_none() || !self.markers.is_empty() {
            // The decoder already finished (or moved past) the audible track: reopen it.
            let deck = match self.open(&path, gain) {
                Ok(d) => d,
                Err(code) => return self.emit(Event::Error(code.into())),
            };
            if let Some(mut following) = self.cur.replace(deck).filter(|_| !self.markers.is_empty()) {
                following.seek(0.0);
                self.next = Some(following);
            }
            self.markers.clear();
        }
        self.flush();
        let (landed, speed) = self.cur.as_mut().map_or((0.0, 1.0), |d| (d.seek(secs), d.speed));
        if let Some(t) = &mut self.track {
            t.start_frame = 0;
            t.start_secs = landed;
            t.speed = speed;
        }
        self.emit_position();
    }

    /// Fades out and empties the ring (the callback does it), then restarts frame counting at 0.
    fn flush(&mut self) {
        self.pending.clear();
        self.pending_off = 0;
        self.pushed = 0;
        self.jumps.clear();
        if self.ring.is_none() {
            self.shared.frames.store(0, Release);
            return;
        }
        self.shared.flush.store(true, Release);
        let t0 = Instant::now();
        while self.shared.flush.load(Acquire) {
            if t0.elapsed() > FLUSH_TIMEOUT {
                // The callback is not running: the stream is dead. Rebuild on the next tick.
                self.out = None;
                self.ring = None;
                self.shared.flush.store(false, Release);
                self.shared.frames.store(0, Release);
                self.last_device_check = t0 - DEVICE_CHECK_EVERY;
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn fill(&mut self) {
        if self.ring.is_none() {
            return;
        }
        loop {
            if self.pending_off >= self.pending.len() {
                self.pending.clear();
                self.pending_off = 0;
                if !self.decode() {
                    return;
                }
                continue;
            }
            let Some(ring) = self.ring.as_mut() else { return };
            let (done, _) = ring.push_partial_slice(&self.pending[self.pending_off..]);
            if done.is_empty() {
                return;
            }
            self.pending_off += done.len();
            self.pushed += (done.len() / 2) as u64;
        }
    }

    /// Decodes the next chunk of the current deck into the (empty) `pending`, crossfading into
    /// the next deck when its time comes. False = nothing left to decode.
    fn decode(&mut self) -> bool {
        let Some(deck) = self.cur.as_mut() else { return false };
        let mut more = deck.read(&mut self.pending).unwrap_or_else(|code| {
            let _ = self.events.send(Event::Error(code.into()));
            false
        });
        // A-B: cut the chunk exactly at B and go on decoding from A (no gap, no flush). Only while
        // the decoder is still in the audible track.
        if let Some((a, b)) = self.ab.filter(|_| self.markers.is_empty() && self.fade.is_none()) {
            let end = (b * f64::from(self.rate) / deck.speed).round() as u64;
            if deck.out_frames >= end {
                let chunk = (self.pending.len() / 2) as u64;
                let keep = end.saturating_sub(deck.out_frames.saturating_sub(chunk)).min(chunk);
                self.pending.truncate(keep as usize * 2);
                let landed = deck.seek(a);
                self.jumps.push_back((self.pushed + keep, landed));
                more = true;
            }
        }
        let gain_a = deck.gain_db;
        if self.exclusive_active() {
            // Bit-perfect: nothing touches the samples (no EQ, normalization, limiter or fades).
            if !more {
                self.advance();
            }
            return true;
        }
        let chunk = (self.pending.len() / 2) as u64;
        if self.fade.is_none() && !self.bit_perfect && self.ab.is_none() {
            self.start_fade(chunk);
        }
        let Some(f) = self.fade.as_mut() else {
            self.dsp.process(&mut self.pending, gain_a);
            if !more {
                self.advance();
            }
            return true;
        };
        let from = f.start.min(chunk) as usize;
        f.start = 0;
        let need = (chunk as usize - from) * 2;
        while f.buf.len() < need {
            if !matches!(f.deck.read(&mut f.buf), Ok(true)) {
                // The incoming track is shorter than the fade: silence after its end.
                f.buf.resize(need.max(f.buf.len()), 0.0);
            }
        }
        let gain_b = f.deck.gain_db;
        // The DSP runs at the incoming deck's normalization from `from` on: pre-scale the
        // outgoing one so it keeps its own.
        let ka = db_to_lin(gain_a - gain_b);
        for (i, &[bl, br]) in (from..chunk as usize).zip(f.buf.as_chunks::<2>().0) {
            let (ga, gb) = crossfade_gains(f.pos, f.len);
            f.pos += 1;
            let mix = |a: f32, b: f32| (f64::from(a) * ga * ka + f64::from(b) * gb) as f32;
            self.pending[2 * i] = mix(self.pending[2 * i], bl);
            self.pending[2 * i + 1] = mix(self.pending[2 * i + 1], br);
        }
        f.buf.drain(..need);
        let done = f.pos >= f.len || !more;
        if done {
            // The outgoing track is over (anything left of it was at zero gain): carry on with
            // the incoming one, starting with what was decoded of it beyond the mix.
            let f = self.fade.take().expect("checked");
            self.pending.extend_from_slice(&f.buf);
            self.cur = Some(f.deck);
        }
        self.dsp.fading = !done;
        let (head, tail) = self.pending.split_at_mut(from * 2);
        self.dsp.process(head, gain_a);
        if gain_a != gain_b {
            self.dsp.jump_gain(gain_b);
        }
        self.dsp.process(tail, gain_b);
        true
    }

    /// Starts a crossfade into the next deck if the current one ends within its length.
    fn start_fade(&mut self, chunk: u64) {
        let (Some(a), Some(b)) = (&self.cur, &self.next) else { return };
        let total = a.total_frames();
        let mut want = (b.crossfade * f64::from(self.rate)).round() as u64;
        if b.total_frames() > 0 {
            // Never fade a short track away entirely.
            want = want.min(b.total_frames() / 2);
        }
        if total == 0 {
            return;
        }
        let Some((start, len)) = fade_window(want, total.saturating_sub(a.out_frames), chunk) else { return };
        let deck = self.next.take().expect("checked");
        self.markers.push_back(Marker::of(self.pushed + start, &deck));
        self.fade = Some(Fade { deck, len, pos: 0, start, buf: Vec::new() });
    }

    /// The current deck is fully decoded: continue into the next one (if any).
    fn advance(&mut self) {
        self.cur = None;
        if let Some(d) = self.next.take() {
            self.chain(d);
        }
    }

    fn tick(&mut self) {
        let ring_empty = self.pending_off >= self.pending.len() && self.shared.frames.load(Acquire) >= self.pushed;
        if self.switch.is_some() && ring_empty && self.ring.is_some() {
            let mut d = self.switch.take().expect("checked");
            self.open_output(Some(d.format()));
            d.set_out_rate(self.rate);
            self.pending.clear();
            self.pending_off = 0;
            self.pushed = 0;
            self.jumps.clear();
            self.markers.push_back(Marker::of(0, &d));
            self.cur = Some(d);
        }
        let played = self.shared.frames.load(Acquire);
        while self.jumps.front().is_some_and(|&(f, _)| f <= played) {
            let (frame, secs) = self.jumps.pop_front().expect("checked");
            if let Some(t) = &mut self.track {
                t.start_frame = frame;
                t.start_secs = secs;
            }
            self.emit_position();
        }
        while self.markers.front().is_some_and(|m| m.frame <= played) {
            let m = self.markers.pop_front().expect("checked");
            self.ab = None;
            self.track = Some(Track {
                path: m.path.clone(),
                duration: m.duration,
                start_frame: m.frame,
                start_secs: 0.0,
                gain_db: m.gain_db,
                format: m.format,
                speed: m.speed,
            });
            self.emit(Event::TrackChanged { path: m.path });
            self.emit_position();
        }
        let drained = self.cur.is_none() && self.switch.is_none() && self.markers.is_empty() && self.pending_off >= self.pending.len();
        if self.state == PlayState::Playing && self.ring.is_some() && drained && played >= self.pushed {
            self.emit(Event::TrackEnded);
            self.stop();
        }
        if self.state == PlayState::Playing && self.last_position.elapsed() >= POSITION_EVERY {
            self.emit_position();
        }
        if self.last_device_check.elapsed() >= DEVICE_CHECK_EVERY {
            self.last_device_check = Instant::now();
            if self.output_stale() {
                self.rebuild_output();
            }
        }
    }

    /// The stream died, or it follows the system default and that changed.
    fn output_stale(&self) -> bool {
        let Some(o) = &self.out else { return true };
        let info = o.info();
        let follows_default = self.device.is_none() || info.fallback == Some("deviceMissing");
        o.broken() || (follows_default && output::default_id().is_some_and(|id| id != info.device_id))
    }

    fn position(&self) -> f64 {
        let Some(t) = &self.track else { return 0.0 };
        let played = self.shared.frames.load(Acquire).saturating_sub(t.start_frame);
        // ponytail: ignores the device buffer (~10 ms); use cpal timestamps if lyric sync needs it.
        let secs = t.start_secs + played as f64 * t.speed / f64::from(self.rate);
        if t.duration > 0.0 { secs.min(t.duration) } else { secs }
    }

    fn emit_position(&mut self) {
        self.last_position = Instant::now();
        let duration = self.track.as_ref().map_or(0.0, |t| t.duration);
        let speed = self.track.as_ref().map_or(1.0, |t| t.speed);
        self.emit(Event::Position { secs: self.position(), duration, speed });
    }

    /// Format the output should be open at now: the audible file's, in bit-perfect mode and
    /// only while playing or paused (stopped = shared, the device is free for other apps).
    fn wanted(&self) -> Option<Format> {
        if self.bit_perfect && self.state != PlayState::Stopped {
            self.track.as_ref().map(|t| t.format)
        } else {
            None
        }
    }

    /// Drops the old stream and opens the chosen device (`want` = bit-perfect for that format).
    /// False = no output at all.
    fn open_output(&mut self, want: Option<Format>) -> bool {
        self.out = None; // drops the old stream (and its ring consumer) first
        self.ring = None;
        self.open_for = want;
        let ok = match output::open(&self.shared, self.device.as_deref(), want) {
            Ok((out, ring)) => {
                self.rate = out.info().sample_rate;
                self.dsp.set_rate(self.rate);
                *self.info.lock().unwrap_or_else(|e| e.into_inner()) = Some(out.info().clone());
                self.out = Some(out);
                self.ring = Some(ring);
                self.output_error_sent = false;
                true
            }
            Err(code) => {
                *self.info.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if !self.output_error_sent {
                    self.output_error_sent = true;
                    self.emit(Event::Error(code.into()));
                }
                false
            }
        };
        self.emit(Event::OutputChanged);
        ok
    }

    /// (Re)opens the output (device lost or changed, new settings). The ring is new and may run
    /// at another rate, so the audible track is reopened where it was.
    fn rebuild_output(&mut self) {
        let pos = self.position();
        self.settle();
        let following = if self.markers.is_empty() { self.next.as_ref() } else { self.cur.as_ref() }
            .map(|d| (d.path.clone(), d.gain_db, d.crossfade));
        if !self.open_output(self.wanted()) {
            return;
        }
        self.pending.clear();
        self.pending_off = 0;
        self.pushed = 0;
        self.jumps.clear();
        self.cur = None;
        self.clear_following();
        if self.state == PlayState::Stopped {
            return;
        }
        self.reopen_at(pos, following);
    }

    /// New speed: the decks reopen where the listener is, at the new rate (a short gap).
    fn respeed(&mut self) {
        if self.bit_perfect || self.state == PlayState::Stopped || self.track.is_none() {
            return;
        }
        let pos = self.position();
        self.settle();
        let following = if self.markers.is_empty() { self.next.as_ref() } else { self.cur.as_ref() }
            .map(|d| (d.path.clone(), d.gain_db, d.crossfade));
        self.flush();
        self.cur = None;
        self.clear_following();
        self.reopen_at(pos, following);
        self.emit_position();
    }

    /// Opens the audible track at `pos` (and the one after it), after the ring was emptied.
    fn reopen_at(&mut self, pos: f64, following: Option<(PathBuf, f64, f64)>) {
        let Some(t) = &self.track else { return };
        let (path, gain) = (t.path.clone(), t.gain_db);
        match self.open(&path, gain) {
            Ok(mut d) => {
                let landed = d.seek(pos);
                if let Some(t) = &mut self.track {
                    t.start_secs = landed;
                    t.start_frame = 0;
                    t.speed = d.speed;
                }
                self.cur = Some(d);
            }
            Err(code) => return self.emit(Event::Error(code.into())),
        }
        self.next = following.and_then(|(p, g, x)| {
            let mut d = self.open(&p, g).ok()?;
            d.crossfade = x;
            Some(d)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfade_is_equal_power_and_ends_on_the_new_track() {
        assert_eq!(crossfade_gains(0, 100), (1.0, 0.0));
        assert_eq!(crossfade_gains(100, 100), (0.0, 1.0));
        assert_eq!(crossfade_gains(500, 100), (0.0, 1.0), "past the end: only the new track");
        let mut last = (1.0, 0.0);
        for pos in 1..100 {
            let (a, b) = crossfade_gains(pos, 100);
            assert!((a * a + b * b - 1.0).abs() < 1e-12, "constant power at {pos}");
            assert!(a < last.0 && b > last.1, "monotonic at {pos}");
            last = (a, b);
        }
        let (a, b) = crossfade_gains(50, 100);
        assert!((a - b).abs() < 1e-12 && (a - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12, "-3 dB each at the middle");
    }

    /// Stereo 16-bit WAV holding one constant value.
    fn dc_wav(path: &Path, rate: u32, frames: u32, value: i16) {
        wav(path, rate, frames, |_| value);
    }

    /// Stereo 16-bit WAV, `sample(frame)` on both channels.
    fn wav(path: &Path, rate: u32, frames: u32, sample: impl Fn(u32) -> i16) {
        let data = frames * 4;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        for f in 0..frames {
            let v = sample(f).to_le_bytes();
            b.extend_from_slice(&v);
            b.extend_from_slice(&v);
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn producer_crossfades_into_the_next_deck() {
        let dir = std::env::temp_dir().join(format!("poximp-xfade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
        let rate = 8000;
        dc_wav(&a, rate, 16_000, 16_384); // 2 s at 0.5
        dc_wav(&b, rate, 12_000, 8_192); // 1.5 s at 0.25
        let (_tx, rx) = crossbeam_channel::unbounded();
        let (ev, _events) = crossbeam_channel::unbounded();
        let mut p = Producer::new(rx, ev, Arc::new(Shared::new()), Arc::new(Mutex::new(None)));
        let (ring, mut out) = rtrb::RingBuffer::new(1 << 17);
        p.ring = Some(ring);
        p.rate = rate;
        p.state = PlayState::Playing;
        p.cur = Some(Deck::open(&a, rate).unwrap());
        let mut next = Deck::open(&b, rate).unwrap();
        next.crossfade = 0.5; // 4000 frames
        p.next = Some(next);
        p.fill();
        let got: Vec<f32> = out.read_chunk(out.slots()).unwrap().into_iter().collect();
        let fade = 4000usize;
        assert_eq!(got.len() / 2, 16_000 + 12_000 - fade, "the fade overlaps the two tracks");
        assert_eq!(p.markers.front().map(|m| m.frame), Some((16_000 - fade) as u64), "the next track starts with the fade");
        assert!(p.cur.is_none() && p.fade.is_none() && !p.dsp.fading, "both decks done");
        let at = |f: usize| got[2 * f];
        assert_eq!(at(0), 0.5);
        assert_eq!(at(16_000 - fade - 1), 0.5, "untouched before the fade");
        for f in [0, 1000, 2000, 3999] {
            let (ga, gb) = crossfade_gains(f as u64, fade as u64);
            let want = (0.5 * ga + 0.25 * gb) as f32;
            assert!((at(16_000 - fade + f) - want).abs() < 1e-6, "frame {f} of the fade: {} vs {want}", at(16_000 - fade + f));
        }
        assert_eq!(at(16_000), 0.25, "only the new track after the fade");
        assert_eq!(*got.last().unwrap(), 0.25);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn ab_loop_jumps_back_to_a_without_a_gap() {
        let dir = std::env::temp_dir().join(format!("poximp-ab-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ramp.wav");
        let rate = 8000;
        // Each frame holds its own index, so the output says where every sample came from.
        wav(&path, rate, 16_000, |f| f as i16);
        let (_tx, rx) = crossbeam_channel::unbounded();
        let (ev, _events) = crossbeam_channel::unbounded();
        let mut p = Producer::new(rx, ev, Arc::new(Shared::new()), Arc::new(Mutex::new(None)));
        let (ring, mut out) = rtrb::RingBuffer::new(1 << 15);
        p.ring = Some(ring);
        p.rate = rate;
        p.state = PlayState::Playing;
        p.cur = Some(Deck::open(&path, rate).unwrap());
        p.ab = Some((0.5, 1.0)); // frames 4000..8000
        p.fill();
        let got: Vec<f32> = out.read_chunk(out.slots()).unwrap().into_iter().collect();
        let frame = |i: usize| (got[2 * i] * 32768.0).round() as i64;
        assert_eq!(got.len(), 1 << 15, "it keeps looping until the ring is full");
        assert_eq!((frame(0), frame(7999)), (0, 7999), "plays up to B");
        assert_eq!((frame(8000), frame(11_999)), (4000, 7999), "then straight on from A");
        assert_eq!(frame(12_000), 4000, "and again");
        assert_eq!(p.jumps.front(), Some(&(8000, 0.5)), "the position jumps when B is heard");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn speed_plays_the_file_in_fewer_frames_and_the_clock_follows() {
        let dir = std::env::temp_dir().join(format!("poximp-speed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("two.wav");
        let rate = 8000;
        wav(&path, rate, 16_000, |f| (f % 100) as i16);
        let (_tx, rx) = crossbeam_channel::unbounded();
        let (ev, _events) = crossbeam_channel::unbounded();
        let mut p = Producer::new(rx, ev, Arc::new(Shared::new()), Arc::new(Mutex::new(None)));
        let (ring, mut out) = rtrb::RingBuffer::new(1 << 15);
        p.ring = Some(ring);
        p.rate = rate;
        p.state = PlayState::Playing;
        p.speed = 1.25;
        let deck = p.open(&path, 0.0).unwrap();
        let format = deck.format();
        p.track = Some(Track { path: path.clone(), duration: deck.duration, start_frame: 0, start_secs: 0.0, gain_db: 0.0, format, speed: deck.speed });
        p.cur = Some(deck);
        p.fill();
        let frames = out.slots() / 2;
        assert!((12_780..=12_820).contains(&frames), "2 s at 1.25x = 1.6 s of output: {frames}");
        p.shared.frames.store(6400, Release);
        assert!((p.position() - 1.0).abs() < 1e-9, "0.8 s heard = 1 s of the file: {}", p.position());
        // Bit-perfect never resamples: the deck opens at normal speed.
        p.bit_perfect = true;
        assert_eq!(p.open(&path, 0.0).unwrap().speed, 1.0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn fade_window_ends_with_the_outgoing_track() {
        // 5 s fade at 1 kHz = 5000 frames, chunks of 1000.
        assert_eq!(fade_window(5000, 6000, 1000), None, "too early");
        assert_eq!(fade_window(5000, 5000, 1000), None, "exactly at the edge: next chunk");
        assert_eq!(fade_window(5000, 4500, 1000), Some((500, 5000)), "starts mid-chunk");
        // Seek into the last 2 s: the fade shortens to what is left.
        assert_eq!(fade_window(5000, 1000, 1000), Some((0, 2000)));
        assert_eq!(fade_window(0, 10, 1000), None, "off");
        assert_eq!(fade_window(5000, 0, 0), None, "nothing left");
        let (start, len) = fade_window(5000, 4500, 1000).unwrap();
        assert_eq!(start + len, 4500 + 1000, "ends exactly at the end of the track");
    }
}
