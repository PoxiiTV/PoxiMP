//! "Open with PoxiMP": files from the command line (an Explorer double-click, at start or forwarded
//! by the single-instance plugin), from Finder, or dropped on the window. Nothing from outside is
//! trusted: every path is checked here. An Explorer multi-select starts one process per file, so
//! launches that arrive close together are merged into one batch (one queue).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Quiet time after the last launch before the batch plays.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
const MAX_PATH: usize = 4096;
const MAX_NAME: usize = 80;

pub fn is_playlist(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))
}

/// A refused file, for the toast: only its name, never the full path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rejected {
    pub name: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// Not there (or not readable).
    Missing,
    /// A folder, another kind of file, or audio PoxiMP could not read.
    Unsupported,
    /// A playlist with nothing playable in it.
    Empty,
}

impl Rejected {
    pub fn new(path: &Path, reason: Reason) -> Self {
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
        Self { name: name.chars().take(MAX_NAME).collect(), reason }
    }
}

/// An existing regular file, audio or M3U, as an absolute path (relative ones against `cwd`).
/// It is checked through its canonical path (symlinks resolved), but the plain absolute path is
/// returned: canonical paths turn mapped drives into UNC ones and would duplicate library rows.
pub fn check(path: &Path, cwd: &Path) -> Result<PathBuf, Rejected> {
    let bad = |reason| Err(Rejected::new(path, reason));
    if path.as_os_str().len() > MAX_PATH {
        return bad(Reason::Unsupported);
    }
    let Ok(abs) = std::path::absolute(cwd.join(path)) else { return bad(Reason::Missing) };
    let Ok(real) = std::fs::canonicalize(&abs) else { return bad(Reason::Missing) };
    let known = |p: &Path| poximp_library::supported(p) || is_playlist(p);
    if !real.is_file() || !known(&abs) || !known(&real) {
        return bad(Reason::Unsupported);
    }
    Ok(abs)
}

/// Command-line arguments after the program name. Flags (`-x`) are not files and are ignored.
pub fn parse_args(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> (Vec<PathBuf>, Vec<Rejected>) {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for a in args {
        if a.is_empty() || a.to_string_lossy().starts_with('-') {
            continue;
        }
        match check(Path::new(&a), cwd) {
            Ok(p) => ok.push(p),
            Err(r) => bad.push(r),
        }
    }
    (ok, bad)
}

/// Files of launches still arriving, in arrival order, each once.
#[derive(Debug, Default)]
pub struct Batch {
    paths: Vec<PathBuf>,
    last: Option<Instant>,
}

impl Batch {
    /// Adds a launch's files. True when this starts a new batch: the caller then waits on `take`.
    pub fn push(&mut self, paths: Vec<PathBuf>, now: Instant) -> bool {
        if paths.is_empty() {
            return false;
        }
        let start = self.last.is_none();
        for p in paths {
            if !self.paths.contains(&p) {
                self.paths.push(p);
            }
        }
        self.last = Some(now);
        start
    }

    /// The whole batch once no launch came for `DEBOUNCE`, else how much longer to wait.
    pub fn take(&mut self, now: Instant) -> Result<Vec<PathBuf>, Duration> {
        let quiet = self.last.map_or(DEBOUNCE, |t| now.saturating_duration_since(t));
        if quiet < DEBOUNCE {
            return Err(DEBOUNCE - quiet);
        }
        self.last = None;
        Ok(std::mem::take(&mut self.paths))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("poximp-open-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("sub")).unwrap();
        for f in ["a.mp3", "B.FLAC", "list.m3u8", "notes.txt", "sub/c.opus"] {
            fs::write(d.join(f), b"x").unwrap();
        }
        fs::create_dir_all(d.join("folder.mp3")).unwrap();
        d
    }

    #[test]
    fn args_are_checked() {
        let d = dir();
        let args = ["--flag", "", "a.mp3", "B.FLAC", "list.m3u8", "notes.txt", "gone.mp3", "folder.mp3", "sub/c.opus"];
        let abs = d.join("a.mp3").into_os_string();
        let (ok, bad) = parse_args(args.iter().map(OsString::from).chain([abs]), &d);
        assert_eq!(ok, [d.join("a.mp3"), d.join("B.FLAC"), d.join("list.m3u8"), d.join("sub").join("c.opus"), d.join("a.mp3")]);
        let r = |name: &str, reason| Rejected { name: name.into(), reason };
        assert_eq!(bad, [r("notes.txt", Reason::Unsupported), r("gone.mp3", Reason::Missing), r("folder.mp3", Reason::Unsupported)]);
        assert!(ok.iter().all(|p| p.is_absolute()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn long_names_are_cut() {
        let long = "x".repeat(300) + ".mp3";
        let (ok, bad) = parse_args([OsString::from(&long)], Path::new("/nowhere"));
        assert!(ok.is_empty());
        assert_eq!(bad[0].name.chars().count(), MAX_NAME);
    }

    #[test]
    fn launches_close_together_merge() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let p = |s: &str| PathBuf::from(s);
        let mut b = Batch::default();
        assert!(!b.push(vec![], t0), "nothing to open arms nothing");
        assert!(b.push(vec![p("1"), p("2")], t0), "first launch arms the timer");
        assert_eq!(b.take(ms(100)), Err(Duration::from_millis(200)));
        assert!(!b.push(vec![p("3"), p("1")], ms(250)), "later launches join the batch");
        assert_eq!(b.take(ms(400)), Err(Duration::from_millis(150)), "each launch restarts the wait");
        assert_eq!(b.take(ms(550)), Ok(vec![p("1"), p("2"), p("3")]));
        assert!(b.push(vec![p("4")], ms(2000)), "after it played, a new batch starts");
        assert_eq!(b.take(ms(2300)), Ok(vec![p("4")]));
    }
}
