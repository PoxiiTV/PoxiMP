//! AutoEq headphone corrections (github.com/jaakkopasanen/AutoEq, MIT), searched inside the app:
//! the INDEX.md of the repo is cached in app data for a week (search works offline), and each
//! model's "ParametricEQ.txt" is downloaded on apply. Ported from PoxiEQ.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use poximp_engine::{Band, Kind, MAX_BANDS};
use serde::Serialize;

const RAW: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results";
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const MAX_RESULTS: usize = 40;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    /// Who measured it: oratory1990, crinacle, Rtings...
    pub source: String,
    /// `over-ear`, `in-ear` or `earbud`
    pub form: String,
    /// Path inside `results/`, as the index has it (URL-encoded). The id for `autoeq_apply`.
    pub path: String,
}

/// `- [Name](./source/kind/Name) by source on rig`
pub fn parse_index(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|l| {
            let l = l.strip_prefix("- [")?;
            let (name, rest) = l.split_once("](./")?;
            // Names carry parentheses ("(250 Ohm)"): the path ends at ") by ".
            let (path, rest) = rest.split_once(") by ")?;
            let source = rest.split(" on ").next()?.trim().to_string();
            let kind = path.split('/').nth(1)?.replace("%20", " ");
            let form = ["in-ear", "over-ear", "earbud"].into_iter().find(|f| kind.contains(f)).unwrap_or("over-ear").to_string();
            safe_path(path).then(|| Entry { name: name.to_string(), source, form, path: path.to_string() })
        })
        .collect()
}

/// Index paths only: no climbing out of `results/`, no odd characters in the URL.
pub fn safe_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() < 300
        && !p.contains("..")
        && !p.starts_with('/')
        && p.chars().all(|c| c.is_ascii_alphanumeric() || " %()&+,.-_'!/$".contains(c))
}

/// HTTP client for every download of the app (AutoEq, LRCLIB): identifies itself, as LRCLIB asks.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("PoxiMP/", env!("CARGO_PKG_VERSION"), " (https://github.com/poxiiTV/PoxiMP)"))
        .timeout(Duration::from_secs(20))
        .build()
}

async fn get(url: String, max: usize) -> Option<String> {
    let res = client().ok()?.get(url).send().await.ok()?.error_for_status().ok()?;
    let bytes = res.bytes().await.ok()?;
    (bytes.len() <= max).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

static INDEX: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// The index: in memory, else the cache if under a week old, else downloaded (and if that fails,
/// the old cache). `None` = offline with no cache.
pub async fn index(cache: &Path) -> Option<Vec<Entry>> {
    {
        let mem = INDEX.lock().unwrap_or_else(|e| e.into_inner());
        if !mem.is_empty() {
            return Some(mem.clone());
        }
    }
    let fresh = std::fs::metadata(cache)
        .and_then(|m| m.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < MAX_AGE);
    let cached = || std::fs::read_to_string(cache).ok().map(|t| parse_index(&t)).filter(|l| !l.is_empty());
    let list = match fresh.then(cached).flatten() {
        Some(l) => l,
        None => match get(format!("{RAW}/INDEX.md"), 8 << 20).await.map(|t| (parse_index(&t), t)) {
            Some((l, text)) if l.len() >= 100 => {
                let _ = std::fs::write(cache, text);
                l
            }
            _ => cached()?,
        },
    };
    *INDEX.lock().unwrap_or_else(|e| e.into_inner()) = list.clone();
    Some(list)
}

/// Every word of `q` in the name (case-insensitive); names that start with it first, then shorter.
pub fn search(list: &[Entry], q: &str) -> Vec<Entry> {
    let q = q.to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(bool, usize, &Entry)> = list
        .iter()
        .filter_map(|e| {
            let n = e.name.to_lowercase();
            words.iter().all(|w| n.contains(w)).then(|| (!n.starts_with(words[0]), n.len(), e))
        })
        .collect();
    hits.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
    hits.into_iter().take(MAX_RESULTS).map(|h| h.2.clone()).collect()
}

/// "ParametricEQ.txt" of an index entry.
pub async fn fetch(path: &str) -> Option<String> {
    if !safe_path(path) {
        return None;
    }
    let file = path.rsplit('/').next().unwrap_or_default();
    get(format!("{RAW}/{path}/{file}%20ParametricEQ.txt"), 64 * 1024).await
}

fn field(rest: &str, key: &str) -> Option<f64> {
    let mut it = rest.split_whitespace();
    while let Some(w) = it.next() {
        if w.eq_ignore_ascii_case(key) {
            return it.next()?.parse().ok().filter(|v: &f64| v.is_finite());
        }
    }
    None
}

/// Equalizer APO text (what AutoEq publishes): `Preamp: -6.2 dB` + `Filter N: ON PK Fc 105 Hz Gain -3.1 dB Q 0.70`.
/// `None` without a single usable filter. Values are clamped later by `EqSettings::sanitize`.
pub fn parse_parametric(text: &str) -> Option<(f64, Vec<Band>)> {
    let mut preamp = 0.0;
    let mut bands = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("Preamp:") {
            preamp = rest.trim().trim_end_matches("dB").trim().parse().unwrap_or(0.0);
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else { continue };
        if !head.trim_start().to_ascii_lowercase().starts_with("filter") {
            continue;
        }
        let mut words = rest.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("ON")) {
            continue;
        }
        let kind = match words.next().map(str::to_ascii_uppercase).as_deref() {
            Some("PK" | "PEQ") => Kind::Peak,
            Some("LSC" | "LS") => Kind::LowShelf,
            Some("HSC" | "HS") => Kind::HighShelf,
            Some("LPQ" | "LP") => Kind::LowPass,
            Some("HPQ" | "HP") => Kind::HighPass,
            _ => continue,
        };
        let Some(freq) = field(rest, "Fc") else { continue };
        let def_q = if kind == Kind::Peak { 1.0 } else { std::f64::consts::FRAC_1_SQRT_2 };
        bands.push(Band { kind, freq, gain: field(rest, "Gain").unwrap_or(0.0), q: field(rest, "Q").unwrap_or(def_q) });
        if bands.len() == MAX_BANDS {
            break;
        }
    }
    // Cascaded biquads commute: low to high reads better as sliders.
    bands.sort_by(|a, b| a.freq.total_cmp(&b.freq));
    (!bands.is_empty()).then_some((preamp, bands))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_index_and_searches() {
        let text = "# Index\n\n- [Beyerdynamic DT 770 Pro](./oratory1990/over-ear/Beyerdynamic%20DT%20770%20Pro) by oratory1990\n\
- [Beyerdynamic DT 770 Pro (250 Ohm)](./crinacle/GRAS%2043AG-7%20over-ear/Beyerdynamic%20DT%20770%20Pro%20(250%20Ohm)) by crinacle on GRAS 43AG-7\n\
- [1Custom SA02](./crinacle/711%20in-ear/1Custom%20SA02) by crinacle on 711\n\
- [Bad](./a/../../etc) by nobody\n";
        let l = parse_index(text);
        assert_eq!(l.len(), 3, "the path with .. is dropped");
        assert_eq!(
            l[0],
            Entry {
                name: "Beyerdynamic DT 770 Pro".into(),
                source: "oratory1990".into(),
                form: "over-ear".into(),
                path: "oratory1990/over-ear/Beyerdynamic%20DT%20770%20Pro".into()
            }
        );
        assert_eq!((l[1].source.as_str(), l[1].form.as_str(), l[2].form.as_str()), ("crinacle", "over-ear", "in-ear"));
        assert!(!safe_path("a/../b") && !safe_path("/etc") && !safe_path("a\\b") && !safe_path("a?x=1"));
        let hits = search(&l, "dt 770");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "Beyerdynamic DT 770 Pro", "shorter name first");
        assert_eq!(search(&l, "1custom")[0].name, "1Custom SA02");
        assert!(search(&l, "  ").is_empty() && search(&l, "sennheiser").is_empty());
    }

    #[test]
    fn parses_parametric_eq() {
        let text = "Preamp: -6.4 dB\r\nFilter 1: ON LSC Fc 105 Hz Gain 5.6 dB Q 0.70\r\nFilter 2: ON PK Fc 2265 Hz Gain -3.1 dB Q 1.87\r\n\
Filter 3: OFF PK Fc 100 Hz Gain 9 dB Q 1\r\nFilter 4: ON HSC Fc 10000 Hz Gain -2.5 dB Q 0.70\r\nFilter 5: ON LPQ Fc 18000 Hz\r\nnoise\r\n";
        let (pre, b) = parse_parametric(text).unwrap();
        assert_eq!(pre, -6.4);
        assert_eq!(b.len(), 4, "OFF filters are skipped");
        assert_eq!(b[0], Band { kind: Kind::LowShelf, freq: 105.0, gain: 5.6, q: 0.70 });
        assert_eq!(b[1].kind, Kind::Peak);
        assert_eq!((b[2].kind, b[3].kind), (Kind::HighShelf, Kind::LowPass), "sorted by frequency");
        assert!((b[3].q - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert!(parse_parametric("Preamp: -3 dB\n").is_none());
        let many: String = (0..15).map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain 1 dB Q 1\n", 100 + i)).collect();
        assert_eq!(parse_parametric(&many).unwrap().1.len(), MAX_BANDS);
    }
}

#[cfg(test)]
mod online {
    /// Needs internet: `cargo test -p poximp -- --ignored autoeq_online --nocapture`
    #[test]
    #[ignore]
    fn autoeq_online() {
        tauri::async_runtime::block_on(async {
            let cache = std::env::temp_dir().join(format!("poximp-autoeq-{}.md", std::process::id()));
            let list = super::index(&cache).await.expect("index");
            let hit = super::search(&list, "DT 770 Pro").into_iter().next().expect("a DT 770 Pro");
            let text = super::fetch(&hit.path).await.expect("ParametricEQ.txt");
            let (pre, bands) = super::parse_parametric(&text).expect("filters");
            println!("{} entries; {} ({}): preamp {pre}, {bands:?}", list.len(), hit.name, hit.source);
            assert!(list.len() > 1000 && pre < 0.0 && bands.len() >= 5);
            let _ = std::fs::remove_file(cache);
        });
    }
}
