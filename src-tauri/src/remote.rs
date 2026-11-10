//! Phone remote: a small HTTP server on the home network, off by default. The QR in Settings
//! carries `http://<lan ip>:<port>/#<token>`; the page reads the token from the fragment (which
//! browsers never send by themselves) and trades it once (POST /pair) for a session cookie kept
//! in memory. Hand-rolled HTTP/1.1 over std::net (httparse for the head): one request per
//! connection, plus one Server-Sent Events stream per phone for the live state.
//! The page (remote/index.html, app.css, app.js) is embedded in the binary.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use poximp_engine::PlayState;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::model::{self, Error, Repeat, Result};
use crate::App;

pub const DEFAULT_PORT: u16 = 47800;
/// Busy port: the next few are tried, then any free one.
const PORT_TRIES: u16 = 10;
const MAX_HEAD: usize = 8 * 1024;
const MAX_HEADERS: usize = 40;
const MAX_BODY: usize = 4 * 1024;
const MAX_CONNS: usize = 16;
const MAX_CONNS_PER_IP: usize = 6;
const MAX_SESSIONS: usize = 8;
/// Track ids a session was shown (queue, search, current): the only ones it may play or fetch covers of.
const MAX_SEEN: usize = 5000;
const MAX_COVER: usize = 8 << 20;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Whole request (head + body), against slow senders.
const READ_DEADLINE: Duration = Duration::from_secs(10);
const SEARCH_LIMIT: u32 = 30;
const QUEUE_PAGE: usize = 50;
/// Live stream: state checked this often (player events wake it sooner) and resent at least every
/// `RESEND` (keeps the connection alive, corrects the phone's clock).
const TICK: Duration = Duration::from_secs(1);
const RESEND: f64 = 10.0;
const COOKIE: &str = "pmr";

const INDEX_HTML: &str = include_str!("remote/index.html");
const APP_JS: &str = include_str!("remote/app.js");
const APP_CSS: &str = include_str!("remote/app.css");

/// Every response: the page runs only its own files, never framed, never cached, no referrer.
const SECURITY_HEADERS: &str = "Content-Security-Policy: default-src 'self'; img-src 'self'; object-src 'none'; \
base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n\
X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\n\
X-Frame-Options: DENY\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Resource-Policy: same-origin\r\n";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// --- token ---

/// 128 random bits from the OS, as 32 hex characters.
fn random_hex() -> Option<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Equal bytes, in a time that does not depend on where they differ (the length is public).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && std::hint::black_box(a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y))) == 0
}

// --- request origin ---

/// DNS rebinding: the browser must be talking to this exact ip:port.
fn host_ok(host: Option<&str>, addr: SocketAddr) -> bool {
    host.is_some_and(|h| h == addr.to_string())
}

/// A request another site makes the browser send says where it comes from: refused.
fn origin_ok(origin: Option<&str>, addr: SocketAddr) -> bool {
    origin.is_none_or(|o| o == format!("http://{addr}"))
}

// --- LAN addresses ---

fn lan(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

/// Who may connect: the home network (and loopback, which only this machine reaches).
fn peer_ok(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if lan(v4) || v4.is_loopback())
}

/// Private IPv4 addresses of the interfaces that are up (name for the picker), the one the
/// default route goes through first.
pub fn lan_ifaces() -> Vec<(Ipv4Addr, String)> {
    let list = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_loopback())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) => Some((a.ip, i.name)),
            if_addrs::IfAddr::V6(_) => None,
        })
        .collect();
    order(list, route_ip())
}

/// LAN addresses only, once each, `route` first.
fn order(list: Vec<(Ipv4Addr, String)>, route: Option<Ipv4Addr>) -> Vec<(Ipv4Addr, String)> {
    let mut out: Vec<(Ipv4Addr, String)> = Vec::new();
    for (ip, name) in list {
        if lan(ip) && !out.iter().any(|(o, _)| *o == ip) {
            let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
            out.push((ip, name));
        }
    }
    out.sort_by_key(|(ip, _)| Some(*ip) != route);
    out
}

/// Source address the OS picks for the internet (a UDP connect sends nothing).
fn route_ip() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect((Ipv4Addr::new(1, 1, 1, 1), 53)).ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// The saved choice if it is still there, else the first (the default route's).
pub fn pick(cands: &[Ipv4Addr], saved: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
    saved.filter(|s| cands.contains(s)).or_else(|| cands.first().copied())
}

// --- rate limits ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Limit {
    /// Token guesses: 5 a minute.
    Pair,
    /// Any request: 20 a second, bursts of 40 (a list of covers loading).
    Api,
}

impl Limit {
    /// (burst, refill per second)
    fn rule(self) -> (f64, f64) {
        match self {
            Limit::Pair => (5.0, 5.0 / 60.0),
            Limit::Api => (40.0, 20.0),
        }
    }
}

/// Token bucket per client address and kind.
#[derive(Default)]
pub struct Limiter {
    buckets: HashMap<(IpAddr, Limit), (f64, Instant)>,
}

impl Limiter {
    pub fn allow(&mut self, ip: IpAddr, kind: Limit, now: Instant) -> bool {
        if self.buckets.len() > 1024 {
            self.buckets.retain(|_, (_, at)| now.saturating_duration_since(*at) < Duration::from_secs(120));
        }
        let (burst, rate) = kind.rule();
        let b = self.buckets.entry((ip, kind)).or_insert((burst, now));
        b.0 = (b.0 + now.saturating_duration_since(b.1).as_secs_f64() * rate).min(burst);
        b.1 = now;
        let ok = b.0 >= 1.0;
        if ok {
            b.0 -= 1.0;
        }
        ok
    }
}

// --- commands ---

/// Everything a phone can ask for. Anything else (unknown command or field, wrong type,
/// out of range) is refused before it reaches the player.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase", deny_unknown_fields)]
pub enum Cmd {
    // `{}`, not unit variants: serde only refuses extra fields on struct variants.
    Play {},
    Pause {},
    Toggle {},
    Next {},
    Prev {},
    /// Seconds; the player also checks it against the track's length.
    Seek { secs: f64 },
    /// 0..=100
    Volume { value: u8 },
    Shuffle { on: bool },
    Repeat { mode: Repeat },
    /// Queue position (play order), as the queue list gives it.
    Jump { position: usize },
    /// A track the phone was shown (search result, queue).
    PlayTrack { id: i64 },
    Search { q: String },
}

pub fn parse_cmd(body: &[u8]) -> Result<Cmd> {
    // Objects only: serde would also take `["seek", 5]` for a tagged enum.
    if body.trim_ascii_start().first() != Some(&b'{') {
        return Err(Error::Invalid);
    }
    let c: Cmd = serde_json::from_slice(body).map_err(|_| Error::Invalid)?;
    let ok = match &c {
        Cmd::Seek { secs } => secs.is_finite() && *secs >= 0.0,
        Cmd::Volume { value } => *value <= 100,
        Cmd::Jump { position } => *position < crate::player::MAX_QUEUE,
        Cmd::PlayTrack { id } => *id > 0,
        // Same limit as the app's search bar.
        Cmd::Search { q } => q.len() <= crate::MAX_TEXT * 4,
        _ => true,
    };
    if ok { Ok(c) } else { Err(Error::Invalid) }
}

// --- what the phone gets ---

/// A track as the phone sees it: no file path, no report.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub format: String,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub bit_depth: Option<u8>,
    pub lossless: bool,
    pub quality: String,
}

impl From<model::Track> for PhoneTrack {
    fn from(t: model::Track) -> Self {
        Self {
            id: t.id,
            title: t.title,
            artist: t.artist,
            album: t.album,
            duration: t.duration,
            format: t.format,
            bitrate: t.bitrate,
            sample_rate: t.sample_rate,
            bit_depth: t.bit_depth,
            lossless: t.lossless,
            quality: t.quality,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub status: PlayState,
    /// 0..=100
    pub volume: u8,
    pub volume_locked: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// Queue position of the current track (play order), -1 if none.
    pub position: i64,
    pub queue_length: usize,
    /// Changes whenever the queue does (the phone refetches its list).
    pub queue_rev: u64,
    pub track: Option<PhoneTrack>,
    pub secs: f64,
    pub duration: f64,
    /// Source seconds per second (Efectos speed): the phone's clock runs at it.
    pub speed: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueList {
    pub total: usize,
    pub position: i64,
    pub offset: usize,
    pub tracks: Vec<PhoneTrack>,
}

/// Worth sending: something besides the clock changed, the clock jumped (seek) or `RESEND` passed.
fn news(last: Option<&(Snapshot, Instant)>, s: &Snapshot, now: Instant) -> bool {
    let Some((l, at)) = last else { return true };
    let elapsed = now.saturating_duration_since(*at).as_secs_f64();
    let expected = if l.status == PlayState::Playing { l.secs + elapsed * l.speed } else { l.secs };
    let rest = |x: &Snapshot| Snapshot { secs: 0.0, ..x.clone() };
    rest(s) != rest(l) || (s.secs - expected).abs() > 1.5 || elapsed >= RESEND
}

/// The player, as the server needs it (the app; a fake in tests).
pub trait Backend: Send + Sync {
    /// `queue_rev` is filled in by the server.
    fn snapshot(&self) -> Snapshot;
    fn queue(&self, offset: usize, limit: usize) -> Result<QueueList>;
    fn search(&self, q: &str) -> Result<Vec<PhoneTrack>>;
    /// Any command but `Search`.
    fn run(&self, cmd: Cmd) -> Result<()>;
    /// Cover bytes and MIME type.
    fn cover(&self, track_id: i64) -> Option<(Vec<u8>, &'static str)>;
}

/// Lives as long as the app: player events wake the phones' live streams.
#[derive(Default)]
pub struct Hub {
    seq: Mutex<u64>,
    cv: Condvar,
    queue_rev: AtomicU64,
}

impl Hub {
    pub fn poke(&self, queue: bool) {
        if queue {
            self.queue_rev.fetch_add(1, Relaxed);
        }
        *lock(&self.seq) += 1;
        self.cv.notify_all();
    }

    /// Waits up to `timeout` for a poke after `seen`; returns the latest count.
    fn wait(&self, seen: u64, timeout: Duration) -> u64 {
        let g = lock(&self.seq);
        let (g, _) = self.cv.wait_timeout_while(g, timeout, |s| *s == seen).unwrap_or_else(|e| e.into_inner());
        *g
    }
}

// --- server ---

struct Session {
    ip: IpAddr,
    device: &'static str,
    last: Instant,
    seen: HashSet<i64>,
    /// Number of the newest live stream, and how many are open.
    stream: u64,
    streams: usize,
}

struct Shared {
    backend: Arc<dyn Backend>,
    hub: Arc<Hub>,
    addr: SocketAddr,
    token: Mutex<String>,
    sessions: Mutex<HashMap<String, Session>>,
    limiter: Mutex<Limiter>,
    conns: Mutex<HashMap<IpAddr, usize>>,
    stop: AtomicBool,
}

/// A paired phone, for Settings.
#[derive(Debug, Clone, Serialize)]
pub struct Phone {
    /// iphone | ipad | android | other
    pub device: &'static str,
    pub ip: String,
    /// Its page is open (live stream connected).
    pub live: bool,
}

/// Rough kind of device from the User-Agent (never shown raw).
fn device(ua: &str) -> &'static str {
    let u = ua.to_ascii_lowercase();
    ["iphone", "ipad", "android"].into_iter().find(|d| u.contains(d)).unwrap_or("other")
}

impl Shared {
    /// A connection slot for `ip`, if the caps allow it.
    fn enter(&self, ip: IpAddr) -> bool {
        let mut c = lock(&self.conns);
        if c.values().sum::<usize>() >= MAX_CONNS {
            return false;
        }
        let mine = c.entry(ip).or_default();
        if *mine >= MAX_CONNS_PER_IP {
            return false;
        }
        *mine += 1;
        true
    }

    fn leave(&self, ip: IpAddr) {
        let mut c = lock(&self.conns);
        if let Some(n) = c.get_mut(&ip) {
            *n -= 1;
            if *n == 0 {
                c.remove(&ip);
            }
        }
    }

    /// A known session from that same address: marked as seen now.
    fn touch(&self, sid: &str, ip: IpAddr) -> bool {
        lock(&self.sessions).get_mut(sid).filter(|s| s.ip == ip).map(|s| s.last = Instant::now()).is_some()
    }

    fn saw(&self, sid: &str, ids: impl IntoIterator<Item = i64>) {
        if let Some(s) = lock(&self.sessions).get_mut(sid) {
            if s.seen.len() > MAX_SEEN {
                s.seen.clear();
            }
            s.seen.extend(ids);
        }
    }

    fn seen(&self, sid: &str, id: i64) -> bool {
        lock(&self.sessions).get(sid).is_some_and(|s| s.seen.contains(&id))
    }

    fn snapshot(&self, sid: &str) -> Snapshot {
        let mut s = self.backend.snapshot();
        s.queue_rev = self.hub.queue_rev.load(Relaxed);
        if let Some(t) = &s.track {
            self.saw(sid, [t.id]);
        }
        s
    }

    fn open_stream(&self, sid: &str) -> Option<u64> {
        let mut ss = lock(&self.sessions);
        let s = ss.get_mut(sid)?;
        s.stream += 1;
        s.streams += 1;
        Some(s.stream)
    }

    fn stream_current(&self, sid: &str, id: u64) -> bool {
        // The two newest of a phone live on (two tabs); an older one (a phone that slept) stops.
        !self.stop.load(Relaxed) && lock(&self.sessions).get(sid).is_some_and(|s| s.stream - id < 2)
    }

    fn close_stream(&self, sid: &str) {
        if let Some(s) = lock(&self.sessions).get_mut(sid) {
            s.streams = s.streams.saturating_sub(1);
        }
    }
}

/// Releases the connection slot however the connection ends.
struct Slot(Arc<Shared>, IpAddr);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.leave(self.1);
    }
}

pub struct Server {
    sh: Arc<Shared>,
    acceptor: Option<JoinHandle<()>>,
}

impl Server {
    /// Listens on `ip`, at `port` or one of the next few if taken (else any free one).
    pub fn start(backend: Arc<dyn Backend>, hub: Arc<Hub>, ip: Ipv4Addr, port: u16) -> io::Result<Self> {
        let token = random_hex().ok_or_else(|| io::Error::other("no randomness"))?;
        let listener = (0..PORT_TRIES)
            .filter_map(|i| port.checked_add(i))
            .chain([0])
            .find_map(|p| TcpListener::bind((ip, p)).ok())
            .ok_or_else(|| io::Error::other("no free port"))?;
        let sh = Arc::new(Shared {
            backend,
            hub,
            addr: listener.local_addr()?,
            token: Mutex::new(token),
            sessions: Mutex::new(HashMap::new()),
            limiter: Mutex::new(Limiter::default()),
            conns: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false),
        });
        let sh2 = sh.clone();
        let acceptor = std::thread::Builder::new().name("poximp-remote".into()).spawn(move || accept(&sh2, &listener))?;
        Ok(Self { sh, acceptor: Some(acceptor) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.sh.addr
    }

    /// What the QR carries.
    pub fn url(&self) -> String {
        format!("http://{}/#{}", self.sh.addr, lock(&self.sh.token))
    }

    /// New code: the old one stops working and every paired phone is signed out.
    pub fn rotate(&self) -> bool {
        let Some(t) = random_hex() else { return false };
        *lock(&self.sh.token) = t;
        lock(&self.sh.sessions).clear();
        self.sh.hub.poke(false);
        true
    }

    pub fn phones(&self) -> Vec<Phone> {
        let mut v: Vec<(Instant, Phone)> = lock(&self.sh.sessions)
            .values()
            .map(|s| (s.last, Phone { device: s.device, ip: s.ip.to_string(), live: s.streams > 0 }))
            .collect();
        v.sort_by_key(|(at, _)| *at);
        v.into_iter().map(|(_, p)| p).collect()
    }
}

impl Drop for Server {
    /// Sessions end with the server; the port is free again when this returns.
    fn drop(&mut self) {
        self.sh.stop.store(true, Relaxed);
        lock(&self.sh.sessions).clear();
        self.sh.hub.poke(false);
        // Wakes the blocking accept; only waited for if that worked (never hangs the UI).
        if TcpStream::connect_timeout(&self.sh.addr, Duration::from_millis(500)).is_ok() {
            if let Some(t) = self.acceptor.take() {
                let _ = t.join();
            }
        }
    }
}

fn accept(sh: &Arc<Shared>, listener: &TcpListener) {
    for conn in listener.incoming() {
        if sh.stop.load(Relaxed) {
            break;
        }
        let Ok(stream) = conn else {
            // Out of descriptors and the like: do not spin.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        let Some(ip) = stream.peer_addr().ok().map(|a| a.ip()).filter(|ip| peer_ok(*ip)) else { continue };
        if !sh.enter(ip) {
            continue;
        }
        let slot = Slot(sh.clone(), ip);
        // If the thread cannot start, the closure (and the slot with it) is dropped.
        let _ = std::thread::Builder::new().name("poximp-remote-conn".into()).spawn(move || handle(&slot.0, stream, slot.1));
    }
}

// --- HTTP ---

struct Req {
    method: String,
    path: String,
    host: Option<String>,
    origin: Option<String>,
    cookie: Option<String>,
    json: bool,
    ua: String,
    body: Vec<u8>,
}

/// One header, at most once; not UTF-8 or repeated is a bad request.
fn header(hs: &[httparse::Header], name: &str) -> std::result::Result<Option<String>, u16> {
    let mut it = hs.iter().filter(|h| h.name.eq_ignore_ascii_case(name));
    let v = it.next().map(|h| std::str::from_utf8(h.value).map(|s| s.trim().to_string()).map_err(|_| 400u16)).transpose()?;
    if it.next().is_some() {
        return Err(400);
    }
    Ok(v)
}

/// Head (max 8 KB) and body (max 4 KB, Content-Length only), or the status to answer with.
fn read_req(s: &mut impl Read) -> std::result::Result<Req, u16> {
    let start = Instant::now();
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() >= MAX_HEAD {
            return Err(431);
        }
        if start.elapsed() > READ_DEADLINE {
            return Err(408);
        }
        let n = s.read(&mut chunk).map_err(|_| 408u16)?;
        if n == 0 {
            return Err(400);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let mut hs = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut r = httparse::Request::new(&mut hs);
    if !matches!(r.parse(&buf[..head_end]), Ok(httparse::Status::Complete(_))) {
        return Err(400);
    }
    let (Some(method), Some(path)) = (r.method, r.path) else { return Err(400) };
    let hs = &*r.headers;
    if header(hs, "transfer-encoding")?.is_some() {
        return Err(400);
    }
    let len = match header(hs, "content-length")? {
        Some(l) => l.parse::<usize>().map_err(|_| 400u16)?,
        None => 0,
    };
    if len > MAX_BODY {
        return Err(413);
    }
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        if start.elapsed() > READ_DEADLINE {
            return Err(408);
        }
        let n = s.read(&mut chunk).map_err(|_| 408u16)?;
        if n == 0 {
            return Err(400);
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Ok(Req {
        method: method.to_string(),
        path: path.to_string(),
        host: header(hs, "host")?,
        origin: header(hs, "origin")?,
        cookie: header(hs, "cookie")?,
        json: header(hs, "content-type")?.is_some_and(|c| c.to_ascii_lowercase().starts_with("application/json")),
        ua: header(hs, "user-agent")?.unwrap_or_default(),
        body,
    })
}

struct Resp {
    status: u16,
    ctype: &'static str,
    body: Vec<u8>,
    cookie: Option<String>,
}

fn resp(ctype: &'static str, body: Vec<u8>) -> Resp {
    Resp { status: 200, ctype, body, cookie: None }
}

fn json<T: Serialize>(v: &T) -> Resp {
    resp("application/json", serde_json::to_vec(v).unwrap_or_default())
}

/// `code` is always one of ours (short, fixed): never a path, a token or an internal message.
fn fail(status: u16, code: &str) -> Resp {
    Resp { status, ..resp("application/json", format!("{{\"error\":\"{code}\"}}").into_bytes()) }
}

fn error(e: Error) -> Resp {
    let code = serde_json::to_value(e).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    fail(match e {
        Error::Invalid => 400,
        Error::NotFound => 404,
        _ => 500,
    }, &code)
}

fn head(status: u16, ctype: &str, len: Option<usize>, cookie: Option<&str>) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    };
    let mut h = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nConnection: close\r\n{SECURITY_HEADERS}");
    if let Some(n) = len {
        h += &format!("Content-Length: {n}\r\n");
    }
    if let Some(c) = cookie {
        h += &format!("Set-Cookie: {c}\r\n");
    }
    h + "\r\n"
}

fn send(s: &mut TcpStream, r: &Resp) {
    let h = head(r.status, r.ctype, Some(r.body.len()), r.cookie.as_deref());
    let _ = s.write_all(h.as_bytes()).and_then(|()| s.write_all(&r.body)).and_then(|()| s.flush());
}

enum Route {
    Reply(Resp),
    /// Session id of a live stream.
    Events(String),
}

fn handle(sh: &Shared, mut s: TcpStream, ip: IpAddr) {
    // Accepted sockets may inherit the listener's mode on some systems: make sure they block.
    let _ = s.set_nonblocking(false);
    let _ = s.set_read_timeout(Some(IO_TIMEOUT));
    let _ = s.set_write_timeout(Some(IO_TIMEOUT));
    let _ = s.set_nodelay(true);
    let req = match read_req(&mut s) {
        Ok(r) => r,
        Err(status) => return send(&mut s, &fail(status, "bad_request")),
    };
    if !lock(&sh.limiter).allow(ip, Limit::Api, Instant::now()) {
        return send(&mut s, &fail(429, "rate_limited"));
    }
    match route(sh, &req, ip) {
        Route::Reply(r) => send(&mut s, &r),
        Route::Events(sid) => events(sh, &mut s, &sid),
    }
}

fn route(sh: &Shared, req: &Req, ip: IpAddr) -> Route {
    if !host_ok(req.host.as_deref(), sh.addr) || !origin_ok(req.origin.as_deref(), sh.addr) {
        return Route::Reply(fail(403, "forbidden"));
    }
    let (path, query) = req.path.split_once('?').unwrap_or((&req.path, ""));
    Route::Reply(match (req.method.as_str(), path) {
        ("GET", "/") => resp("text/html; charset=utf-8", INDEX_HTML.into()),
        ("GET", "/app.js") => resp("text/javascript; charset=utf-8", APP_JS.into()),
        ("GET", "/app.css") => resp("text/css; charset=utf-8", APP_CSS.into()),
        ("POST", "/pair") => pair(sh, req, ip),
        (_, p) if p.starts_with("/api/") => return api(sh, req, ip, p, query),
        _ => fail(404, "not_found"),
    })
}

/// `pmr=<32 hex>` from the Cookie header.
fn session_id(cookie: Option<&str>) -> Option<String> {
    cookie?
        .split(';')
        .filter_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='))
        .find(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(String::from)
}

fn pair(sh: &Shared, req: &Req, ip: IpAddr) -> Resp {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        token: String,
    }
    if !req.json {
        return fail(415, "bad_request");
    }
    if !lock(&sh.limiter).allow(ip, Limit::Pair, Instant::now()) {
        return fail(429, "rate_limited");
    }
    let Ok(b) = serde_json::from_slice::<Body>(&req.body) else { return fail(400, "bad_request") };
    if !ct_eq(b.token.as_bytes(), lock(&sh.token).as_bytes()) {
        return fail(401, "unauthorized");
    }
    let Some(sid) = random_hex() else { return fail(500, "server_error") };
    let mut ss = lock(&sh.sessions);
    if ss.len() >= MAX_SESSIONS {
        // The phone unused the longest makes room (its stream stops).
        if let Some(old) = ss.iter().min_by_key(|(_, s)| s.last).map(|(k, _)| k.clone()) {
            ss.remove(&old);
        }
    }
    let s = Session { ip, device: device(&req.ua), last: Instant::now(), seen: HashSet::new(), stream: 0, streams: 0 };
    ss.insert(sid.clone(), s);
    Resp { cookie: Some(format!("{COOKIE}={sid}; HttpOnly; SameSite=Strict; Path=/")), ..json(&serde_json::json!({ "ok": true })) }
}

fn api(sh: &Shared, req: &Req, ip: IpAddr, path: &str, query: &str) -> Route {
    let Some(sid) = session_id(req.cookie.as_deref()).filter(|sid| sh.touch(sid, ip)) else {
        return Route::Reply(fail(401, "unauthorized"));
    };
    let sid = sid.as_str();
    Route::Reply(match (req.method.as_str(), path) {
        ("GET", "/api/events") => return Route::Events(sid.to_string()),
        ("GET", "/api/state") => json(&sh.snapshot(sid)),
        ("GET", "/api/queue") => {
            let offset = if query.is_empty() { Some(0) } else { query.strip_prefix("offset=").and_then(|n| n.parse::<usize>().ok()) };
            match offset.filter(|&o| o < crate::player::MAX_QUEUE).map(|o| sh.backend.queue(o, QUEUE_PAGE)) {
                Some(Ok(q)) => {
                    sh.saw(sid, q.tracks.iter().map(|t| t.id));
                    json(&q)
                }
                Some(Err(e)) => error(e),
                None => fail(400, "invalid_input"),
            }
        }
        ("POST", "/api/cmd") => cmd(sh, req, sid),
        ("GET", p) if p.starts_with("/api/cover/") => {
            let id = p["/api/cover/".len()..].parse::<i64>().ok().filter(|&id| sh.seen(sid, id));
            match id.and_then(|id| sh.backend.cover(id)) {
                Some((bytes, ctype)) if bytes.len() <= MAX_COVER => resp(ctype, bytes),
                _ => fail(404, "not_found"),
            }
        }
        _ => fail(404, "not_found"),
    })
}

fn cmd(sh: &Shared, req: &Req, sid: &str) -> Resp {
    if !req.json {
        return fail(415, "bad_request");
    }
    match parse_cmd(&req.body) {
        Err(e) => error(e),
        Ok(Cmd::Search { q }) => match sh.backend.search(&q) {
            Ok(tracks) => {
                sh.saw(sid, tracks.iter().map(|t| t.id));
                json(&serde_json::json!({ "tracks": tracks }))
            }
            Err(e) => error(e),
        },
        Ok(Cmd::PlayTrack { id }) if !sh.seen(sid, id) => fail(404, "not_found"),
        Ok(c) => match sh.backend.run(c) {
            Ok(()) => json(&serde_json::json!({ "ok": true })),
            Err(e) => error(e),
        },
    }
}

/// Server-Sent Events: `state` with a Snapshot whenever it has news, until the phone leaves, its
/// session ends (new code, server off) or a newer stream of the same phone takes over.
fn events(sh: &Shared, s: &mut TcpStream, sid: &str) {
    let Some(me) = sh.open_stream(sid) else { return };
    let mut ok = s
        .write_all(head(200, "text/event-stream; charset=utf-8", None, None).as_bytes())
        .and_then(|()| s.write_all(b"retry: 3000\n\n"))
        .is_ok();
    let mut last: Option<(Snapshot, Instant)> = None;
    let mut seq = 0;
    while ok && sh.stream_current(sid, me) {
        let now = Instant::now();
        let snap = sh.snapshot(sid);
        if news(last.as_ref(), &snap, now) {
            let data = serde_json::to_string(&snap).unwrap_or_default();
            ok = write!(s, "event: state\ndata: {data}\n\n").and_then(|()| s.flush()).is_ok();
            last = Some((snap, now));
        }
        seq = sh.hub.wait(seq, TICK);
    }
    sh.close_stream(sid);
}

// --- the app side ---

impl Backend for AppHandle {
    fn snapshot(&self) -> Snapshot {
        let app = self.state::<App>();
        let s = app.sync();
        let position = lock(&app.player).q.position().map_or(-1, |p| p as i64);
        Snapshot {
            status: s.state.status,
            volume: (s.state.volume * 100.0).round().clamp(0.0, 100.0) as u8,
            volume_locked: s.state.volume_locked,
            shuffle: s.state.shuffle,
            repeat: s.state.repeat,
            position,
            queue_length: s.state.queue_length,
            queue_rev: 0,
            track: s.track.map(PhoneTrack::from),
            secs: s.position.secs,
            duration: s.position.duration,
            speed: s.position.speed,
        }
    }

    fn queue(&self, offset: usize, limit: usize) -> Result<QueueList> {
        let q = self.state::<App>().queue_page(offset, limit)?;
        Ok(QueueList { total: q.total, position: q.index, offset: q.offset, tracks: q.tracks.into_iter().map(PhoneTrack::from).collect() })
    }

    fn search(&self, q: &str) -> Result<Vec<PhoneTrack>> {
        let q = crate::trimmed(q);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let app = self.state::<App>();
        let a = app.analyzing();
        let hits = app.library().search(q, SEARCH_LIMIT)?;
        Ok(hits.iter().map(|t| model::track(t, a).into()).collect())
    }

    fn run(&self, cmd: Cmd) -> Result<()> {
        let app = self.state::<App>();
        match cmd {
            Cmd::Play {} => app.play(),
            Cmd::Pause {} => app.pause(),
            Cmd::Toggle {} => app.toggle(),
            Cmd::Next {} => app.next(),
            Cmd::Prev {} => app.previous(),
            Cmd::Seek { secs } => app.seek(secs),
            Cmd::Volume { value } => app.set_volume(f32::from(value) / 100.0),
            Cmd::Shuffle { on } => {
                app.set_shuffle(on);
                Ok(())
            }
            Cmd::Repeat { mode } => {
                app.set_repeat(mode);
                Ok(())
            }
            Cmd::Jump { position } => app.queue_jump(position),
            Cmd::PlayTrack { id } => app.play_ids(vec![id], 0),
            Cmd::Search { .. } => Err(Error::Invalid),
        }
    }

    fn cover(&self, track_id: i64) -> Option<(Vec<u8>, &'static str)> {
        self.state::<App>().cover(track_id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Iface {
    pub ip: String,
    pub name: String,
}

/// What Settings shows.
#[derive(Debug, Clone, Serialize)]
pub struct Info {
    pub on: bool,
    /// With the token (fragment): what the QR says.
    pub url: Option<String>,
    /// The QR, as an SVG document.
    pub qr: Option<String>,
    /// Address in use (or the saved choice while off).
    pub ip: Option<String>,
    /// LAN addresses to choose from, the default route's first. Empty = not on a home network.
    pub ips: Vec<Iface>,
    pub port: u16,
    pub phones: Vec<Phone>,
}

fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M).ok()?;
    Some(code.render::<qrcode::render::svg::Color>().min_dimensions(240, 240).quiet_zone(true).build())
}

pub fn info(app: &App) -> Info {
    let ips = lan_ifaces().into_iter().map(|(ip, name)| Iface { ip: ip.to_string(), name }).collect();
    let srv = lock(&app.remote);
    match srv.as_ref() {
        Some(s) => {
            let url = s.url();
            Info { on: true, qr: qr_svg(&url), url: Some(url), ip: Some(s.addr().ip().to_string()), ips, port: s.addr().port(), phones: s.phones() }
        }
        None => {
            let snd = app.sound();
            Info { on: false, url: None, qr: None, ip: snd.cfg.remote_ip.clone(), ips, port: snd.cfg.remote_port, phones: Vec::new() }
        }
    }
}

/// On (at `ip`, one of `info().ips`; else the saved one or the default route's) or off.
/// Turning it on again, or at another address, starts over: new code, phones signed out.
pub fn set(app: &App, on: bool, ip: Option<String>) -> Result<Info> {
    let mut srv = lock(&app.remote);
    *srv = None;
    if on {
        let cands: Vec<Ipv4Addr> = lan_ifaces().into_iter().map(|(ip, _)| ip).collect();
        let wanted = match ip {
            Some(s) => Some(s.parse::<Ipv4Addr>().ok().filter(|i| cands.contains(i)).ok_or(Error::Invalid)?),
            None => None,
        };
        let (saved, port) = {
            let s = app.sound();
            (s.cfg.remote_ip.as_deref().and_then(|i| i.parse().ok()), s.cfg.remote_port)
        };
        let ip = wanted.or_else(|| pick(&cands, saved)).ok_or(Error::Network)?;
        let server = Server::start(Arc::new(app.app.clone()), app.hub.clone(), ip, port).map_err(|_| Error::Io)?;
        let mut s = app.sound();
        s.cfg.remote_ip = Some(ip.to_string());
        s.cfg.remote_port = server.addr().port();
        s.save();
        drop(s);
        *srv = Some(server);
    }
    drop(srv);
    Ok(info(app))
}

pub fn rotate(app: &App) -> Result<Info> {
    let rotated = lock(&app.remote).as_ref().ok_or(Error::Invalid)?.rotate();
    if !rotated {
        return Err(Error::Io);
    }
    Ok(info(app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compare() {
        let t = random_hex().unwrap();
        assert_eq!(t.len(), 32);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(t, random_hex().unwrap());
        assert!(ct_eq(t.as_bytes(), t.clone().as_bytes()));
        let mut other = t.clone().into_bytes();
        other[31] ^= 1;
        assert!(!ct_eq(t.as_bytes(), &other));
        assert!(!ct_eq(t.as_bytes(), &t.as_bytes()[..31]), "shorter");
        assert!(!ct_eq(b"", t.as_bytes()));
    }

    #[test]
    fn host_and_origin() {
        let a: SocketAddr = "192.168.1.20:47800".parse().unwrap();
        assert!(host_ok(Some("192.168.1.20:47800"), a));
        assert!(!host_ok(None, a));
        assert!(!host_ok(Some("192.168.1.20"), a), "port is part of it");
        assert!(!host_ok(Some("evil.example:47800"), a), "DNS rebinding");
        assert!(!host_ok(Some("localhost:47800"), a));
        assert!(origin_ok(None, a), "no Origin: same-origin GET");
        assert!(origin_ok(Some("http://192.168.1.20:47800"), a));
        assert!(!origin_ok(Some("http://evil.example"), a));
        assert!(!origin_ok(Some("null"), a));
        assert!(!origin_ok(Some("https://192.168.1.20:47800"), a));
        assert_eq!(session_id(Some("a=1; pmr=0123456789abcdef0123456789abcdef")).as_deref(), Some("0123456789abcdef0123456789abcdef"));
        assert_eq!(session_id(Some("pmr=../../etc")), None);
        assert_eq!(session_id(Some("pmrx=0123456789abcdef0123456789abcdef")), None);
    }

    #[test]
    fn rate_limiter() {
        let mut l = Limiter::default();
        let ip: IpAddr = "192.168.1.30".parse().unwrap();
        let other: IpAddr = "192.168.1.31".parse().unwrap();
        let t0 = Instant::now();
        assert!((0..5).all(|_| l.allow(ip, Limit::Pair, t0)));
        assert!(!l.allow(ip, Limit::Pair, t0), "6th guess in the same minute");
        assert!(l.allow(other, Limit::Pair, t0), "per address");
        assert!(l.allow(ip, Limit::Api, t0), "per kind");
        assert!(!l.allow(ip, Limit::Pair, t0 + Duration::from_secs(5)));
        assert!(l.allow(ip, Limit::Pair, t0 + Duration::from_secs(13)), "one more every 12 s");
        let n = (0..100).filter(|_| l.allow(other, Limit::Api, t0)).count();
        assert_eq!(n, 40, "burst");
        assert!(l.allow(other, Limit::Api, t0 + Duration::from_millis(60)), "20 a second refill");
    }

    #[test]
    fn command_validation() {
        let ok = |s: &str| parse_cmd(s.as_bytes()).ok();
        assert_eq!(ok(r#"{"cmd":"toggle"}"#), Some(Cmd::Toggle {}));
        assert_eq!(ok(r#"{"cmd":"seek","secs":12.5}"#), Some(Cmd::Seek { secs: 12.5 }));
        assert_eq!(ok(r#"{"cmd":"volume","value":100}"#), Some(Cmd::Volume { value: 100 }));
        assert_eq!(ok(r#"{"cmd":"repeat","mode":"one"}"#), Some(Cmd::Repeat { mode: Repeat::One }));
        assert_eq!(ok(r#"{"cmd":"playTrack","id":7}"#), Some(Cmd::PlayTrack { id: 7 }));
        assert_eq!(ok(r#"{"cmd":"jump","position":3}"#), Some(Cmd::Jump { position: 3 }));
        for bad in [
            r#"{"cmd":"volume","value":101}"#,
            r#"{"cmd":"volume","value":-1}"#,
            r#"{"cmd":"volume","value":"50"}"#,
            r#"{"cmd":"seek","secs":-1}"#,
            r#"{"cmd":"seek"}"#,
            r#"{"cmd":"repeat","mode":"twice"}"#,
            r#"{"cmd":"jump","position":-2}"#,
            r#"{"cmd":"jump","position":100000}"#,
            r#"{"cmd":"playTrack","id":0}"#,
            r#"{"cmd":"next","path":"C:/x"}"#,
            r#"{"cmd":"seek","secs":1,"extra":true}"#,
            r#"{"cmd":"delete_everything"}"#,
            r#"{"cmd":"library_add_folder","path":"C:/"}"#,
            r#"["toggle"]"#,
            r#"["seek",5]"#,
            r#"{"cmd":"toggle","cmd":"next"}"#,
            "toggle",
            "",
        ] {
            assert!(parse_cmd(bad.as_bytes()).is_err(), "{bad}");
        }
        let long = format!(r#"{{"cmd":"search","q":"{}"}}"#, "a".repeat(801));
        assert!(parse_cmd(long.as_bytes()).is_err());
        assert!(ok(&format!(r#"{{"cmd":"search","q":"{}"}}"#, "a".repeat(800))).is_some());
    }

    #[test]
    fn lan_ip_selection() {
        let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
        let list = vec![
            (ip("8.8.4.4"), "public".into()),
            (ip("172.20.0.1"), "vEthernet (WSL)".into()),
            (ip("192.168.1.20"), "Wi-Fi\u{0}".into()),
            (ip("192.168.1.20"), "dup".into()),
            (ip("10.0.0.5"), "Ethernet".into()),
            (ip("169.254.3.3"), "link".into()),
            (ip("100.64.0.1"), "CGNAT".into()),
        ];
        let o = order(list, Some(ip("192.168.1.20")));
        let ips: Vec<Ipv4Addr> = o.iter().map(|(i, _)| *i).collect();
        assert_eq!(ips, [ip("192.168.1.20"), ip("172.20.0.1"), ip("10.0.0.5"), ip("169.254.3.3")], "route first, no public, once each");
        assert_eq!(o[0].1, "Wi-Fi", "control characters dropped");
        assert_eq!(pick(&ips, None), Some(ip("192.168.1.20")));
        assert_eq!(pick(&ips, Some(ip("10.0.0.5"))), Some(ip("10.0.0.5")), "saved choice kept");
        assert_eq!(pick(&ips, Some(ip("192.168.7.7"))), Some(ip("192.168.1.20")), "saved one gone");
        assert_eq!(pick(&[], Some(ip("10.0.0.5"))), None);
        assert!(peer_ok("192.168.1.9".parse().unwrap()) && peer_ok("127.0.0.1".parse().unwrap()));
        assert!(!peer_ok("8.8.8.8".parse().unwrap()) && !peer_ok("::1".parse().unwrap()));
        assert_eq!(device("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0)"), "iphone");
        assert_eq!(device("Mozilla/5.0 (Linux; Android 15)"), "android");
        assert_eq!(device("<script>"), "other");
    }

    #[test]
    fn live_news() {
        let s = Snapshot {
            status: PlayState::Playing,
            volume: 50,
            volume_locked: false,
            shuffle: false,
            repeat: Repeat::Off,
            position: 0,
            queue_length: 1,
            queue_rev: 0,
            track: None,
            secs: 10.0,
            duration: 100.0,
            speed: 1.0,
        };
        let t0 = Instant::now();
        let last = (s.clone(), t0);
        assert!(news(None, &s, t0));
        assert!(!news(Some(&last), &Snapshot { secs: 12.0, ..s.clone() }, t0 + Duration::from_secs(2)), "clock as expected");
        assert!(news(Some(&last), &Snapshot { secs: 60.0, ..s.clone() }, t0 + Duration::from_secs(2)), "seek");
        assert!(news(Some(&last), &Snapshot { volume: 51, secs: 12.0, ..s.clone() }, t0 + Duration::from_secs(2)));
        assert!(news(Some(&last), &Snapshot { secs: 20.0, ..s.clone() }, t0 + Duration::from_secs(10)), "resend");
    }

    // --- real HTTP on 127.0.0.1 ---

    struct Fake {
        ran: Mutex<Vec<Cmd>>,
    }

    fn track(id: i64) -> PhoneTrack {
        PhoneTrack {
            id,
            title: format!("T{id}"),
            artist: "A".into(),
            album: "B".into(),
            duration: 100.0,
            format: "FLAC".into(),
            bitrate: 900,
            sample_rate: 44_100,
            bit_depth: Some(16),
            lossless: true,
            quality: "real".into(),
        }
    }

    impl Backend for Fake {
        fn snapshot(&self) -> Snapshot {
            Snapshot {
                status: PlayState::Paused,
                volume: 80,
                volume_locked: false,
                shuffle: false,
                repeat: Repeat::Off,
                position: 0,
                queue_length: 2,
                queue_rev: 0,
                track: Some(track(1)),
                secs: 3.0,
                duration: 100.0,
                speed: 1.0,
            }
        }
        fn queue(&self, offset: usize, _: usize) -> Result<QueueList> {
            Ok(QueueList { total: 2, position: 0, offset, tracks: vec![track(1), track(2)] })
        }
        fn search(&self, _: &str) -> Result<Vec<PhoneTrack>> {
            Ok(vec![track(7)])
        }
        fn run(&self, cmd: Cmd) -> Result<()> {
            lock(&self.ran).push(cmd);
            Ok(())
        }
        fn cover(&self, id: i64) -> Option<(Vec<u8>, &'static str)> {
            (id == 7 || id == 1).then(|| (vec![0x89, b'P', b'N', b'G'], "image/png"))
        }
    }

    fn http(addr: SocketAddr, req: &str) -> (u16, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let out = String::from_utf8_lossy(&out).into_owned();
        (out.get(9..12).and_then(|c| c.parse().ok()).unwrap_or(0), out)
    }

    fn get(addr: SocketAddr, path: &str, extra: &str) -> (u16, String) {
        http(addr, &format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}\r\n"))
    }

    fn post(addr: SocketAddr, path: &str, extra: &str, body: &str) -> (u16, String) {
        let n = body.len();
        http(addr, &format!("POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {n}\r\n{extra}\r\n{body}"))
    }

    #[test]
    fn server_end_to_end() {
        let fake = Arc::new(Fake { ran: Mutex::new(Vec::new()) });
        let srv = Server::start(fake.clone(), Arc::new(Hub::default()), Ipv4Addr::LOCALHOST, 0).unwrap();
        let a = srv.addr();
        let token = srv.url().rsplit_once('#').unwrap().1.to_string();
        assert!(srv.url().starts_with(&format!("http://{a}/#")));

        // The page: own files only, security headers.
        let (st, page) = get(a, "/", "");
        assert_eq!(st, 200);
        for h in ["Content-Security-Policy: default-src 'self'", "X-Content-Type-Options: nosniff", "Referrer-Policy: no-referrer", "Cache-Control: no-store"] {
            assert!(page.contains(h), "{h}");
        }
        assert!(!page.contains("<script>"), "no inline script");
        assert_eq!(get(a, "/app.js", "").0, 200);
        // DNS rebinding / other sites / no session.
        assert_eq!(http(a, "GET / HTTP/1.1\r\nHost: evil.example:47800\r\n\r\n").0, 403);
        assert_eq!(http(a, "GET / HTTP/1.1\r\n\r\n").0, 403, "no Host");
        assert_eq!(get(a, "/api/state", "").0, 401);
        assert_eq!(post(a, "/pair", "Origin: http://evil.example\r\n", &format!(r#"{{"token":"{token}"}}"#)).0, 403);
        assert_eq!(get(a, "/../../etc/passwd", "").0, 404);
        assert_eq!(get(a, "/api/cover/../../x", "").0, 401);
        // Bodies: size cap, JSON only.
        assert_eq!(post(a, "/pair", "", &"x".repeat(MAX_BODY + 1)).0, 413);
        let plain = format!("POST /pair HTTP/1.1\r\nHost: {a}\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n{{}}");
        assert_eq!(http(a, &plain).0, 415);

        // Pairing.
        assert_eq!(post(a, "/pair", "", r#"{"token":"00000000000000000000000000000000"}"#).0, 401);
        let (st, res) = post(a, "/pair", &format!("Origin: http://{a}\r\nUser-Agent: Android\r\n"), &format!(r#"{{"token":"{token}"}}"#));
        assert_eq!(st, 200);
        assert!(res.contains("HttpOnly; SameSite=Strict"));
        let sid = res.split("pmr=").nth(1).unwrap()[..32].to_string();
        let c = format!("Cookie: pmr={sid}\r\n");
        assert_eq!(srv.phones().len(), 1);
        assert_eq!(srv.phones()[0].device, "android");

        let (st, state) = get(a, "/api/state", &c);
        assert_eq!(st, 200);
        assert!(state.contains("\"volume\":80") && !state.contains("path"));
        // Commands.
        assert_eq!(post(a, "/api/cmd", &c, r#"{"cmd":"volume","value":42}"#).0, 200);
        assert_eq!(post(a, "/api/cmd", &c, r#"{"cmd":"volume","value":420}"#).0, 400);
        assert_eq!(post(a, "/api/cmd", &c, r#"{"cmd":"playTrack","id":7}"#).0, 404, "not shown to this phone yet");
        assert_eq!(get(a, "/api/cover/7", &c).0, 404, "same for covers");
        let (st, found) = post(a, "/api/cmd", &c, r#"{"cmd":"search","q":"t"}"#);
        assert!(st == 200 && found.contains("\"id\":7"));
        assert_eq!(post(a, "/api/cmd", &c, r#"{"cmd":"playTrack","id":7}"#).0, 200);
        let (st, img) = get(a, "/api/cover/7", &c);
        assert!(st == 200 && img.contains("Content-Type: image/png"));
        assert_eq!(get(a, "/api/cover/1", &c).0, 200, "current track");
        assert_eq!(get(a, "/api/queue?offset=0", &c).0, 200);
        assert_eq!(get(a, "/api/queue?offset=x", &c).0, 400);
        assert_eq!(*lock(&fake.ran), [Cmd::Volume { value: 42 }, Cmd::PlayTrack { id: 7 }]);

        // Live stream.
        let mut es = TcpStream::connect(a).unwrap();
        es.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(es, "GET /api/events HTTP/1.1\r\nHost: {a}\r\n{c}\r\n").unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while !String::from_utf8_lossy(&got).contains("event: state") {
            let n = es.read(&mut buf).unwrap();
            assert!(n > 0);
            got.extend_from_slice(&buf[..n]);
        }
        assert!(String::from_utf8_lossy(&got).contains("text/event-stream"));
        assert!(srv.phones()[0].live);

        // New code: old token and old session are dead, the stream ends.
        assert!(srv.rotate());
        assert_eq!(get(a, "/api/state", &c).0, 401);
        assert_eq!(post(a, "/pair", "", &format!(r#"{{"token":"{token}"}}"#)).0, 401);
        let _ = es.read_to_end(&mut got);
        assert!(srv.phones().is_empty());
        let new = srv.url().rsplit_once('#').unwrap().1.to_string();
        assert_ne!(new, token);

        // Pairing guesses are limited (5 a minute: three used above, two more here).
        let bad = r#"{"token":"00000000000000000000000000000000"}"#;
        let codes: Vec<u16> = (0..3).map(|_| post(a, "/pair", "", bad).0).collect();
        assert_eq!(codes, [401, 401, 429]);

        // Stopping frees the port.
        drop(srv);
        assert!(TcpListener::bind(a).is_ok());
    }
}
