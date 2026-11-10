//! Smart playlists: a JSON rule tree `{ all | any: [rule | group], limit?, sort? }` compiled to
//! SQL. Columns and operators come from fixed whitelists; every user value is a bound parameter.

use rusqlite::types::Value as Sql;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result};

const MAX_DEPTH: usize = 8;
const MAX_RULES: usize = 200;
const MAX_TEXT: usize = 200;
pub const MAX_LIMIT: u32 = 10_000;
const DAY: f64 = 86_400.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Genre,
    Year,
    /// Seconds.
    Duration,
    Codec,
    /// kbps.
    Bitrate,
    SampleRate,
    BitDepth,
    QualityVerdict,
    PlayCount,
    SkipCount,
    /// 0-5.
    Rating,
    /// Unix seconds.
    LastPlayedAt,
    AddedAt,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Is,
    IsNot,
    Contains,
    NotContains,
    StartsWith,
    Gt,
    Lt,
    /// `value` = [low, high], inclusive.
    Between,
    /// `value` = days. Never played counts as "not in the last days".
    InLastDays,
    NotInLastDays,
    IsEmpty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmartSort {
    /// Stable for a day (seeded), so paging a random list does not reshuffle it.
    Random,
    MostPlayed,
    LeastPlayed,
    RecentlyPlayed,
    RecentlyAdded,
    MostSkipped,
    Rating,
    Title,
    Artist,
    Album,
    Year,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub field: Field,
    pub op: Op,
    #[serde(default)]
    pub value: Value,
}

/// Exactly one of `all` (AND) / `any` (OR).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<Node>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<Node>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Node {
    Rule(Rule),
    Group(Group),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmartRules {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<Node>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<Node>>,
    /// 1..=MAX_LIMIT; none = every match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<SmartSort>,
}

/// WHERE / ORDER BY fragments (over `tracks t`) and their parameters, in that order.
pub(crate) struct Compiled {
    pub wh: String,
    pub order: String,
    pub args: Vec<Sql>,
    pub limit: i64,
}

/// Parses and validates rules JSON (what gets stored in `playlists.rules_json`).
pub fn parse(json: &str) -> Result<SmartRules> {
    let r: SmartRules =
        serde_json::from_str(json).map_err(|_| Error::Invalid("malformed smart playlist rules"))?;
    r.compile(0)?;
    Ok(r)
}

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Num,
    Date,
}

/// Whitelisted column, its kind, and the factor from the UI unit to the column's.
fn column(f: Field) -> (&'static str, Kind, f64) {
    use Field::*;
    use Kind::*;
    match f {
        Title => ("t.title", Text, 1.0),
        Artist => ("t.artist", Text, 1.0),
        Album => ("t.album", Text, 1.0),
        AlbumArtist => ("t.album_artist", Text, 1.0),
        Genre => ("t.genre", Text, 1.0),
        Codec => ("t.codec", Text, 1.0),
        QualityVerdict => ("t.quality_verdict", Text, 1.0),
        Folder => ("t.folder", Text, 1.0),
        Year => ("t.year", Num, 1.0),
        Duration => ("t.duration_ms", Num, 1000.0),
        Bitrate => ("t.bitrate_kbps", Num, 1.0),
        SampleRate => ("t.sample_rate", Num, 1.0),
        BitDepth => ("t.bit_depth", Num, 1.0),
        PlayCount => ("t.play_count", Num, 1.0),
        SkipCount => ("t.skip_count", Num, 1.0),
        Rating => ("t.rating", Num, 1.0),
        LastPlayedAt => ("t.last_played_at", Date, 1.0),
        AddedAt => ("t.added_at", Date, 1.0),
    }
}

fn text(v: &Value) -> Result<String> {
    match v {
        Value::String(s) if s.chars().count() <= MAX_TEXT => Ok(s.trim().to_string()),
        _ => Err(Error::Invalid("rule value must be text (up to 200 characters)")),
    }
}

fn num(v: &Value) -> Result<f64> {
    v.as_f64()
        .filter(|n| n.is_finite() && n.abs() < 1e15)
        .ok_or(Error::Invalid("rule value must be a number"))
}

fn pair(v: &Value) -> Result<(f64, f64)> {
    match v.as_array().map(Vec::as_slice) {
        Some([a, b]) => {
            let (a, b) = (num(a)?, num(b)?);
            Ok((a.min(b), a.max(b)))
        }
        _ => Err(Error::Invalid("between needs [low, high]")),
    }
}

fn days(v: &Value) -> Result<f64> {
    let d = num(v)?;
    if (0.0..=36_500.0).contains(&d) {
        Ok(d)
    } else {
        Err(Error::Invalid("days must be 0-36500"))
    }
}

/// LIKE pattern for literal text (used with `ESCAPE '\'`).
fn like(s: &str, prefix: bool) -> String {
    let e = s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    if prefix {
        format!("{e}%")
    } else {
        format!("%{e}%")
    }
}

/// Tag string or any of the split artists (multi-artist table).
const ARTIST_IS: &str = "(t.artist = ? COLLATE NOCASE OR EXISTS (SELECT 1 FROM track_artists ta \
    JOIN artists a ON a.id = ta.artist_id WHERE ta.track_id = t.id AND a.name = ?))";

struct Ctx {
    now: i64,
    rules: usize,
    sql: String,
    args: Vec<Sql>,
}

impl Ctx {
    fn group(&mut self, all: &Option<Vec<Node>>, any: &Option<Vec<Node>>, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(Error::Invalid("smart playlist rules nested too deep"));
        }
        let (nodes, and) = match (all, any) {
            (Some(n), None) => (n, true),
            (None, Some(n)) => (n, false),
            _ => return Err(Error::Invalid("a rule group needs exactly one of all / any")),
        };
        if nodes.is_empty() {
            // Empty "all" matches everything, empty "any" nothing (as in logic).
            self.sql.push_str(if and { "1" } else { "0" });
            return Ok(());
        }
        self.sql.push('(');
        for (i, n) in nodes.iter().enumerate() {
            if i > 0 {
                self.sql.push_str(if and { " AND " } else { " OR " });
            }
            match n {
                Node::Rule(r) => self.rule(r)?,
                Node::Group(g) => self.group(&g.all, &g.any, depth + 1)?,
            }
        }
        self.sql.push(')');
        Ok(())
    }

    fn rule(&mut self, r: &Rule) -> Result<()> {
        self.rules += 1;
        if self.rules > MAX_RULES {
            return Err(Error::Invalid("too many smart playlist rules"));
        }
        let (c, kind, scale) = column(r.field);
        let v = &r.value;
        let a = &mut self.args;
        // `c` and every fragment are static whitelisted text: values only ever go through `a`.
        let frag = match (kind, r.op) {
            (Kind::Text, Op::IsEmpty) => format!("({c} IS NULL OR {c} = '')"),
            (Kind::Num, Op::IsEmpty) => format!("({c} IS NULL OR {c} = 0)"),
            (Kind::Date, Op::IsEmpty) => format!("{c} IS NULL"),
            (Kind::Text, Op::Is | Op::IsNot) => {
                let s = text(v)?;
                let not = if r.op == Op::IsNot { "NOT " } else { "" };
                if r.field == Field::Artist {
                    a.push(Sql::Text(s.clone()));
                    a.push(Sql::Text(s));
                    format!("{not}{ARTIST_IS}")
                } else {
                    a.push(Sql::Text(s));
                    format!("{not}(COALESCE({c}, '') = ? COLLATE NOCASE)")
                }
            }
            (Kind::Text, Op::Contains | Op::NotContains | Op::StartsWith) => {
                // ponytail: LIKE folds ASCII case only ("Ñ" ≠ "ñ"); FTS if accents matter here.
                a.push(Sql::Text(like(&text(v)?, r.op == Op::StartsWith)));
                let not = if r.op == Op::NotContains { "NOT " } else { "" };
                format!("{not}(COALESCE({c}, '') LIKE ? ESCAPE '\\')")
            }
            (Kind::Num, Op::Is) => {
                a.push(Sql::Real(num(v)? * scale));
                format!("{c} = ?")
            }
            (Kind::Num, Op::IsNot) => {
                a.push(Sql::Real(num(v)? * scale));
                format!("({c} IS NULL OR {c} <> ?)")
            }
            (Kind::Num | Kind::Date, Op::Gt | Op::Lt) => {
                a.push(Sql::Real(num(v)? * scale));
                format!("{c} {} ?", if r.op == Op::Gt { ">" } else { "<" })
            }
            (Kind::Num | Kind::Date, Op::Between) => {
                let (lo, hi) = pair(v)?;
                a.push(Sql::Real(lo * scale));
                a.push(Sql::Real(hi * scale));
                format!("{c} BETWEEN ? AND ?")
            }
            (Kind::Date, Op::InLastDays) => {
                a.push(Sql::Real(self.now as f64 - days(v)? * DAY));
                format!("{c} >= ?")
            }
            (Kind::Date, Op::NotInLastDays) => {
                a.push(Sql::Real(self.now as f64 - days(v)? * DAY));
                format!("({c} IS NULL OR {c} < ?)")
            }
            _ => return Err(Error::Invalid("operator not valid for this field")),
        };
        self.sql.push_str(&frag);
        Ok(())
    }
}

impl SmartRules {
    pub(crate) fn compile(&self, now: i64) -> Result<Compiled> {
        if self.limit.is_some_and(|l| l == 0 || l > MAX_LIMIT) {
            return Err(Error::Invalid("limit must be 1-10000"));
        }
        let mut cx = Ctx { now, rules: 0, sql: String::new(), args: Vec::new() };
        cx.group(&self.all, &self.any, 0)?;
        let album = "t.album COLLATE NOCASE, t.album_key, t.disc_no, t.track_no, t.path_key";
        let order = match self.sort.unwrap_or(SmartSort::Artist) {
            SmartSort::Random => {
                // Seeded hash of the id: a new shuffle every day, stable while paging.
                cx.args.push(Sql::Integer(now / 86_400));
                "((t.id * 2654435761 + ?) % 4294967291), t.id".to_string()
            }
            SmartSort::MostPlayed => "t.play_count DESC, t.last_played_at DESC, t.id".into(),
            SmartSort::LeastPlayed => "t.play_count, t.last_played_at, t.id".into(),
            SmartSort::RecentlyPlayed => "t.last_played_at DESC, t.id".into(),
            SmartSort::RecentlyAdded => "t.added_at DESC, t.id DESC".into(),
            SmartSort::MostSkipped => "t.skip_count DESC, t.id".into(),
            SmartSort::Rating => "t.rating DESC, t.play_count DESC, t.id".into(),
            SmartSort::Title => "t.title COLLATE NOCASE, t.id".into(),
            SmartSort::Artist => format!("t.artist COLLATE NOCASE, {album}"),
            SmartSort::Album => album.into(),
            SmartSort::Year => format!("t.year DESC, {album}"),
        };
        Ok(Compiled { wh: cx.sql, order, args: cx.args, limit: self.limit.map_or(-1, i64::from) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fake_track;
    use crate::{now, Library};
    use serde_json::json;

    const YEAR: i64 = 365 * 86_400;

    fn ids(lib: &Library, rules: Value) -> Vec<i64> {
        let r = parse(&rules.to_string()).unwrap();
        let mut v = lib.smart_ids(&r).unwrap();
        if r.sort.is_none() {
            v.sort_unstable();
        }
        v
    }

    #[test]
    fn owners_example_and_nested_groups() {
        let lib = Library::open_in_memory().unwrap();
        let t = now();
        let old_unskipped = fake_track(&lib, "/m/a.flac", "A", "Mora", &format!("last_played_at = {}, play_count = 9", t - 3 * YEAR));
        let old_skipped = fake_track(&lib, "/m/b.flac", "B", "Mora", &format!("last_played_at = {}, skip_count = 2", t - 3 * YEAR));
        let recent = fake_track(&lib, "/m/c.flac", "C", "Quevedo", &format!("last_played_at = {}", t - 86_400));
        let never = fake_track(&lib, "/m/d.mp3", "D", "Mora & C. Tangana", "codec = 'MP3', genre = 'Trap', year = 2019");

        // "Not played in more than 2 years and never skipped".
        let owner = json!({"all": [
            {"field": "last_played_at", "op": "not_in_last_days", "value": 730},
            {"field": "skip_count", "op": "is", "value": 0}
        ]});
        assert_eq!(ids(&lib, owner), [old_unskipped, never]);
        // Same, but only things that were ever played.
        let played = json!({"all": [
            {"field": "last_played_at", "op": "not_in_last_days", "value": 730},
            {"field": "skip_count", "op": "is", "value": 0},
            {"not": "a rule"}
        ]});
        assert!(parse(&played.to_string()).is_err());
        let played = json!({"all": [
            {"field": "last_played_at", "op": "not_in_last_days", "value": 730},
            {"field": "skip_count", "op": "is", "value": 0},
            {"field": "last_played_at", "op": "is_empty", "value": null}
        ]});
        assert_eq!(ids(&lib, played), [never]);

        // any [ all[Mora, skipped], all[recent] ]
        let nested = json!({"any": [
            {"all": [{"field": "artist", "op": "is", "value": "mora"}, {"field": "skip_count", "op": "gt", "value": 0}]},
            {"all": [{"field": "last_played_at", "op": "in_last_days", "value": 7}]}
        ]});
        assert_eq!(ids(&lib, nested), [old_skipped, recent]);

        // Artist "is" sees the split artists of "Mora & C. Tangana"; "is_not" is its negation.
        assert_eq!(ids(&lib, json!({"all": [{"field": "artist", "op": "is", "value": "C. TANGANA"}]})), [never]);
        assert_eq!(ids(&lib, json!({"all": [{"field": "artist", "op": "is_not", "value": "Mora"}]})), [recent]);
        // Text ops, LIKE wildcards in values are literal.
        assert_eq!(ids(&lib, json!({"all": [{"field": "genre", "op": "contains", "value": "ra"}]})), [never]);
        assert!(ids(&lib, json!({"all": [{"field": "title", "op": "contains", "value": "%"}]})).is_empty());
        assert_eq!(ids(&lib, json!({"all": [{"field": "folder", "op": "starts_with", "value": "/m"}]})).len(), 4);
        assert_eq!(ids(&lib, json!({"all": [{"field": "codec", "op": "is_not", "value": "flac"}]})), [never]);
        // Numbers: duration in seconds (fake tracks are 180 s), between, empty year.
        assert_eq!(ids(&lib, json!({"all": [{"field": "duration", "op": "between", "value": [200, 170]}]})).len(), 4);
        assert_eq!(ids(&lib, json!({"all": [{"field": "year", "op": "is", "value": 2019}]})), [never]);
        assert_eq!(ids(&lib, json!({"all": [{"field": "year", "op": "is_empty"}]})).len(), 3);
        // Empty groups: all = everything, any = nothing.
        assert_eq!(ids(&lib, json!({"all": []})).len(), 4);
        assert!(ids(&lib, json!({"any": []})).is_empty());

        // Sort + limit.
        let top = json!({"all": [], "sort": "most_played", "limit": 1});
        assert_eq!(ids(&lib, top), [old_unskipped]);
        let r = parse(&json!({"all": [], "sort": "random"}).to_string()).unwrap();
        assert_eq!(lib.smart_ids(&r).unwrap(), lib.smart_ids(&r).unwrap());
    }

    #[test]
    fn rejects_bad_rules() {
        let bad = [
            json!({"all": [{"field": "path", "op": "is", "value": "x"}]}),
            json!({"all": [{"field": "title", "op": "gt", "value": 3}]}),
            json!({"all": [{"field": "year", "op": "contains", "value": "19"}]}),
            json!({"all": [{"field": "year", "op": "is", "value": "1999; DROP TABLE tracks"}]}),
            json!({"all": [{"field": "added_at", "op": "in_last_days", "value": -1}]}),
            json!({"all": [{"field": "year", "op": "between", "value": [1]}]}),
            json!({"all": [], "any": []}),
            json!({}),
            json!({"all": [], "limit": 0}),
            json!({"all": [], "sort": "loudest"}),
            json!({"all": [], "extra": 1}),
        ];
        for b in bad {
            assert!(parse(&b.to_string()).is_err(), "{b}");
        }
        let mut deep = json!({"all": []});
        for _ in 0..10 {
            deep = json!({"all": [deep]});
        }
        assert!(parse(&deep.to_string()).is_err());
        let many: Vec<Value> = (0..201).map(|_| json!({"field": "rating", "op": "gt", "value": 1})).collect();
        assert!(parse(&json!({ "all": many }).to_string()).is_err());
        // Round trip keeps the shape the UI sent.
        let ok = json!({"any": [{"field": "rating", "op": "gt", "value": 3}, {"all": []}], "limit": 5, "sort": "rating"});
        assert_eq!(serde_json::to_value(parse(&ok.to_string()).unwrap()).unwrap(), ok);
    }
}
