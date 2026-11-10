use super::*;
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("poximp-lib-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Minimal 16-bit mono 8 kHz PCM WAV of silence, optionally with an ID3v2 tag.
fn wav(path: &Path, samples: u32, tags: Option<(&str, &str, &str)>) {
    let data = samples * 2;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    b.resize(b.len() + data as usize, 0);
    fs::write(path, b).unwrap();
    if let Some((title, artist, album)) = tags {
        let mut f = lofty::read_from_path(path).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title(title.into());
        tag.set_artist(artist.into());
        tag.set_album(album.into());
        f.insert_tag(tag);
        f.save_to_path(path, WriteOptions::default()).unwrap();
    }
}

/// Synthetic track row (no file behind it). `extra` = test-only SQL SET list ("play_count = 3").
pub(crate) fn fake_track(lib: &Library, path: &str, title: &str, artist: &str, extra: &str) -> i64 {
    let folder = Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    lib.conn
        .execute(
            "INSERT INTO tracks (path, path_key, folder, size, mtime, title, artist, album_key, codec, container, added_at, duration_ms) \
             VALUES (?1, ?2, ?3, 0, 0, ?4, ?5, ?6, 'FLAC', 'FLAC', 0, 180000)",
            params![path, path_key(path), folder, title, artist, format!("{artist}\u{1f}{title}")],
        )
        .unwrap();
    let id = lib.conn.last_insert_rowid();
    for (pos, (name, feat)) in split_artists(artist, DEFAULT_SEPARATORS).iter().enumerate() {
        let aid: i64 = lib
            .conn
            .query_row(
                "INSERT INTO artists (name) VALUES (?1) ON CONFLICT(name) DO UPDATE SET name = name RETURNING id",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        lib.conn
            .execute(
                "INSERT OR IGNORE INTO track_artists VALUES (?1, ?2, ?3, ?4)",
                params![id, aid, if *feat { "feat" } else { "main" }, pos as i64],
            )
            .unwrap();
    }
    if !extra.is_empty() {
        lib.conn.execute(&format!("UPDATE tracks SET {extra} WHERE id = ?1"), [id]).unwrap();
    }
    id
}

#[test]
fn schema_migrates_once() {
    let dir = temp_dir("schema");
    let db = dir.join("lib.db");
    drop(Library::open(&db).unwrap());
    let lib = Library::open(&db).unwrap();
    let v: u32 = lib
        .conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v as usize, MIGRATIONS.len());
    let mode: String = lib
        .conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    for t in [
        "tracks",
        "artists",
        "track_artists",
        "playlists",
        "playlist_items",
        "plays",
        "resume_positions",
        "tracks_fts",
    ] {
        let n: i64 = lib
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                [t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{t}");
    }
}

#[test]
fn artist_splitting() {
    let s = |x: &str| split_artists(x, DEFAULT_SEPARATORS);
    let main = |v: &[&str]| -> Vec<(String, bool)> {
        v.iter().map(|n| (n.to_string(), false)).collect()
    };
    assert_eq!(
        s("Raul Clyde, Sergi El Combo, Xiyo, Fernandezz"),
        main(&["Raul Clyde", "Sergi El Combo", "Xiyo", "Fernandezz"])
    );
    assert_eq!(s("Mora & C. Tangana"), main(&["Mora", "C. Tangana"]));
    assert_eq!(
        s("X feat. Y"),
        vec![("X".into(), false), ("Y".into(), true)]
    );
    assert_eq!(
        s("Lil Jon FT. Offset & 2 Chainz"),
        vec![
            ("Lil Jon".into(), false),
            ("Offset".into(), true),
            ("2 Chainz".into(), true)
        ]
    );
    assert_eq!(
        s("Bad Bunny (feat. Jhay Cortez)"),
        vec![("Bad Bunny".into(), false), ("Jhay Cortez".into(), true)]
    );
    assert_eq!(s("Ñengo Flow x Anuel AA"), main(&["Ñengo Flow", "Anuel AA"]));
    assert_eq!(s("A;B; a"), main(&["A", "B"]));
    assert_eq!(s("Malcolm X"), main(&["Malcolm X"]));
    assert_eq!(s(""), main(&[]));
}

#[test]
fn folder_tree_compacts_chains() {
    let t = folder_tree(
        vec![
            ("F:\\Music\\Rock\\A".into(), 2),
            ("F:\\Music\\Rock\\B".into(), 3),
            ("F:\\Music\\Pop".into(), 1),
        ],
        &[],
    );
    assert_eq!(t.len(), 1);
    let music = &t[0];
    assert_eq!(music.name, format!("F:{}Music", std::path::MAIN_SEPARATOR));
    let kids: Vec<_> = music.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(kids, ["Pop", "Rock"]);
    assert_eq!(music.children[0].path, "F:\\Music\\Pop");
    assert_eq!(music.children[1].children[1].path, "F:\\Music\\Rock\\B");
    assert_eq!(music.children[1].children[1].track_count, 3);
    assert_eq!((music.total_count, music.children[1].total_count), (6, 5));

    // A music folder is its own top node (named as added), kept even with a single subfolder.
    let t = folder_tree(
        vec![("F:\\Music\\Rock\\A".into(), 2), ("G:\\loose".into(), 1)],
        &["F:\\Music\\".into()],
    );
    let names: Vec<_> = t.iter().map(|n| n.name.clone()).collect();
    assert_eq!(names, ["F:\\Music".to_string(), format!("G:{}loose", std::path::MAIN_SEPARATOR)]);
    assert_eq!(t[0].children[0].name, format!("Rock{}A", std::path::MAIN_SEPARATOR));
    assert_eq!((t[0].total_count, t[0].children[0].path.as_str()), (2, "F:\\Music\\Rock\\A"));
}

#[test]
fn folder_filter_view_ids_and_playlist_edits() {
    let mut lib = Library::open_in_memory().unwrap();
    let sep = std::path::MAIN_SEPARATOR;
    let root = format!("{sep}m");
    let a = fake_track(&lib, &format!("{root}{sep}a.flac"), "A", "X", "");
    let b = fake_track(&lib, &format!("{root}{sep}sub{sep}b.flac"), "B", "X", "");
    let c = fake_track(&lib, &format!("{root}{sep}sub_2{sep}c.flac"), "C", "X", "");
    let _other = fake_track(&lib, &format!("{sep}mm{sep}d.flac"), "D", "X", "");
    let q = |folder: String, recursive: bool| TrackQuery {
        folder: Some(folder),
        folder_recursive: recursive,
        sort: SortKey::Title,
        ..Default::default()
    };
    assert_eq!(lib.view_ids(&q(root.clone(), false)).unwrap(), [a]);
    // "/m" recursive must not pick up "/mm"; "_" in "sub_2" is literal.
    assert_eq!(lib.view_ids(&q(root.clone(), true)).unwrap(), [a, b, c]);
    assert_eq!(lib.view_ids(&q(format!("{root}{sep}sub"), true)).unwrap(), [b]);
    assert_eq!(lib.list_tracks(&q(root.clone(), true)).unwrap().total, 3);
    let got: Vec<_> = lib.tracks_by_ids(&[c, 999, a, c]).unwrap().iter().map(|t| t.id).collect();
    assert_eq!(got, [c, a, c]);

    let p = lib.create_playlist("Mix", PlaylistKind::Manual, None).unwrap();
    lib.add_to_playlist(p, &[a, b, c]).unwrap();
    lib.move_in_playlist(p, 0, 2).unwrap();
    assert_eq!(lib.playlist_ids(p).unwrap(), [b, c, a]);
    lib.remove_from_playlist(p, &[1, 0]).unwrap();
    assert_eq!(lib.playlist_ids(p).unwrap(), [a]);
    assert!(lib.remove_from_playlist(p, &[1]).is_err());
    lib.rename_playlist(p, "  Coche ").unwrap();
    assert_eq!(lib.playlist(p).unwrap().name, "Coche");
    assert!(matches!(lib.playlist(999), Err(Error::NotFound)));

    // Smart: rules validated, contents evaluated, not editable by hand.
    assert!(lib.create_playlist("S", PlaylistKind::Smart, None).is_err());
    assert!(lib.create_playlist("S", PlaylistKind::Smart, Some(r#"{"all": [{"field": "nope"}]}"#)).is_err());
    let rules = r#"{"all": [{"field": "title", "op": "is_not", "value": "a"}], "sort": "title"}"#;
    let s = lib.create_playlist("S", PlaylistKind::Smart, Some(rules)).unwrap();
    assert_eq!(lib.playlist_ids(s).unwrap(), [b, c, _other]);
    assert_eq!(lib.playlists().unwrap().iter().find(|x| x.id == s).unwrap().track_count, 3);
    assert!(lib.add_to_playlist(s, &[a]).is_err());
    let r = smart::parse(r#"{"any": [{"field": "title", "op": "is", "value": "B"}]}"#).unwrap();
    lib.set_smart_rules(s, &r).unwrap();
    assert_eq!(lib.playlist_ids(s).unwrap(), [b]);
    assert!(lib.set_smart_rules(p, &r).is_err());
}

#[test]
fn scan_search_and_incremental_rescan() {
    let dir = temp_dir("scan");
    let music = dir.join("music");
    let covers = dir.join("covers");
    fs::create_dir_all(music.join("Album")).unwrap();
    let a = music.join("Album").join("01.wav");
    let b = music.join("Album").join("02.wav");
    let c = music.join("plain.wav");
    wav(
        &a,
        800,
        Some((
            "Canción de prueba",
            "Raul Clyde, Sergi El Combo, Xiyo, Fernandezz",
            "Ñoño",
        )),
    );
    wav(&b, 800, Some(("DROGA", "Mora & C. Tangana", "Ñoño")));
    wav(&c, 800, None);
    fs::write(music.join("notes.txt"), "x").unwrap();
    fs::write(music.join(".hidden.wav"), "x").unwrap();

    let mut lib = Library::open(dir.join("lib.db")).unwrap();
    let no = AtomicBool::new(false);
    let mut last = None;
    let r = lib.scan(&[&music], &covers, &no, |p| last = Some(p)).unwrap();
    assert_eq!((r.found, r.added, r.errors), (3, 3, 0), "{r:?}");
    assert_eq!(last.map(|p| (p.done, p.total)), Some((3, 3)));

    // Accent- and case-insensitive prefix search.
    let hits = lib.search("cancion", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "Canción de prueba");
    assert_eq!(lib.search("CANCIÓN prue", 10).unwrap().len(), 1);
    assert_eq!(lib.search("nono", 10).unwrap().len(), 2);
    assert_eq!(lib.search("tangana", 10).unwrap()[0].title, "DROGA");
    assert!(lib.search("\"*)(", 10).unwrap().is_empty());

    // Untagged file falls back to its file name.
    let all = lib.list_tracks(&TrackQuery::default()).unwrap();
    assert_eq!(all.total, 3);
    assert!(all.tracks.iter().any(|t| t.title == "plain" && t.codec == "PCM"));

    // Split artists, with per-artist filtering.
    let artists = lib.artists().unwrap();
    let artist_names: Vec<_> = artists.iter().map(|a| a.name.as_str()).collect();
    for n in ["Raul Clyde", "Sergi El Combo", "Xiyo", "Fernandezz", "Mora", "C. Tangana"] {
        assert!(artist_names.contains(&n), "{n} in {artist_names:?}");
    }
    let mora = artists.iter().find(|a| a.name == "Mora").unwrap();
    let q = TrackQuery {
        artist_id: Some(mora.id),
        ..Default::default()
    };
    assert_eq!(lib.list_tracks(&q).unwrap().tracks[0].title, "DROGA");
    let first = &hits[0];
    let of: Vec<_> = lib
        .artists_of(first.id)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(of, ["Raul Clyde", "Sergi El Combo", "Xiyo", "Fernandezz"]);

    // No album artist: albums group by first main artist, so same title + different artist = 2.
    let albums = lib.albums().unwrap();
    let nono: Vec<_> = albums.iter().filter(|a| a.title == "Ñoño").collect();
    assert_eq!(nono.len(), 2);
    assert!(nono.iter().any(|a| a.artist.as_deref() == Some("Mora & C. Tangana")));

    // Rescan: nothing changed.
    let r = lib.scan(&[&music], &covers, &no, |_| {}).unwrap();
    assert_eq!((r.unchanged, r.added, r.updated, r.missing), (3, 0, 0, 0), "{r:?}");

    // Changed file is re-read, deleted file is kept but unavailable.
    lib.set_rating(first.id, 4).unwrap();
    wav(&a, 1600, Some(("Canción nueva", "Xiyo", "Ñoño")));
    fs::remove_file(&c).unwrap();
    let r = lib.scan(&[&music], &covers, &no, |_| {}).unwrap();
    assert_eq!((r.unchanged, r.updated, r.missing), (1, 1, 1), "{r:?}");
    let t = lib.track_by_id(first.id).unwrap().unwrap();
    assert_eq!((t.title.as_str(), t.rating), ("Canción nueva", 4));
    assert_eq!(lib.list_tracks(&TrackQuery::default()).unwrap().total, 2);
    let q = TrackQuery {
        include_unavailable: true,
        ..Default::default()
    };
    assert_eq!(lib.list_tracks(&q).unwrap().total, 3);
    assert!(lib.search("prueba", 10).unwrap().is_empty());
    // Orphaned artists are dropped.
    assert!(!lib.artists().unwrap().iter().any(|a| a.name == "Fernandezz"));

    // Cancelled scan marks nothing missing.
    let yes = AtomicBool::new(true);
    let r = lib.scan(&[&music], &covers, &yes, |_| {}).unwrap();
    assert!(r.cancelled && r.missing == 0);
}

#[test]
fn stats_playlists_quality_resume() {
    let dir = temp_dir("api");
    let music = dir.join("m");
    fs::create_dir_all(&music).unwrap();
    for i in 0..3 {
        wav(&music.join(format!("{i}.wav")), 100 + i * 10, None);
    }
    let mut lib = Library::open_in_memory().unwrap();
    lib.scan(&[&music], &dir.join("covers"), &AtomicBool::new(false), |_| {})
        .unwrap();
    let ids: Vec<i64> = lib
        .list_tracks(&TrackQuery {
            sort: SortKey::Path,
            ..Default::default()
        })
        .unwrap()
        .tracks
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids.len(), 3);

    lib.record_play(ids[0], 30_000).unwrap();
    lib.record_skip(ids[0]).unwrap();
    let t = lib.track_by_id(ids[0]).unwrap().unwrap();
    assert_eq!((t.play_count, t.skip_count), (1, 1));
    assert!(t.last_played_at.is_some());
    assert!(lib.set_rating(ids[0], 6).is_err());

    let jobs = lib.tracks_needing_quality(10).unwrap();
    assert_eq!(jobs.len(), 3);
    lib.set_quality(jobs[0].id, "lossless", "{}", &jobs[0].key)
        .unwrap();
    lib.set_quality(jobs[1].id, "lossless", "{}", "stale:key")
        .unwrap();
    let left: Vec<_> = lib
        .tracks_needing_quality(10)
        .unwrap()
        .into_iter()
        .map(|j| j.id)
        .collect();
    assert_eq!(left, [jobs[1].id, jobs[2].id]);

    let p = lib
        .create_playlist(" Favoritas ", PlaylistKind::Manual, None)
        .unwrap();
    assert!(lib.create_playlist("  ", PlaylistKind::Manual, None).is_err());
    lib.set_playlist_tracks(p, &[ids[2], ids[0]]).unwrap();
    lib.add_to_playlist(p, &[ids[1]]).unwrap();
    let order: Vec<_> = lib.playlist_tracks(p).unwrap().iter().map(|t| t.id).collect();
    assert_eq!(order, [ids[2], ids[0], ids[1]]);
    lib.update_playlist(p, "Top", Some(r#"{"all":[]}"#)).unwrap();
    let pl = &lib.playlists().unwrap()[0];
    assert_eq!((pl.name.as_str(), pl.track_count), ("Top", 3));
    lib.delete_playlist(p).unwrap();
    assert!(lib.playlists().unwrap().is_empty());

    assert_eq!(lib.resume_position(ids[1]).unwrap(), None);
    lib.set_resume_position(ids[1], 61_000).unwrap();
    lib.set_resume_position(ids[1], 62_000).unwrap();
    assert_eq!(lib.resume_position(ids[1]).unwrap(), Some(62_000));
    lib.set_resume_position(ids[1], 0).unwrap();
    assert_eq!(lib.resume_position(ids[1]).unwrap(), None);

    let tree = lib.folders().unwrap();
    assert_eq!(tree.len(), 1);
}

#[test]
fn roots_text_filter_page_stats_and_file_roots() {
    let dir = temp_dir("roots");
    let music = dir.join("music");
    let covers = dir.join("covers");
    fs::create_dir_all(music.join("sub")).unwrap();
    let a = music.join("a.wav");
    let b = music.join("sub").join("b.wav");
    wav(&a, 8000, Some(("Canción", "Mora", "Uno")));
    wav(&b, 16000, Some(("DROGA", "Mora", "Dos")));

    let mut lib = Library::open(dir.join("lib.db")).unwrap();
    let m = music.to_string_lossy().into_owned();
    assert!(lib.add_root(&m).unwrap());
    assert!(!lib.add_root(&format!("{m}{}", std::path::MAIN_SEPARATOR)).unwrap());
    assert!(!lib.add_root(&music.join("sub").to_string_lossy()).unwrap());
    assert_eq!(lib.roots().unwrap(), [m]);

    // A single file works as a scan root.
    let no = AtomicBool::new(false);
    let r = lib.scan(&[&b], &covers, &no, |_| {}).unwrap();
    assert_eq!((r.found, r.added), (1, 1), "{r:?}");
    let bid = lib.id_for_path(&b.to_string_lossy()).unwrap().unwrap();
    lib.scan(&[&music], &covers, &no, |_| {}).unwrap();

    let all = lib.list_tracks(&TrackQuery::default()).unwrap();
    assert_eq!((all.total, all.duration_ms), (2, 3000));
    assert_eq!(all.verdicts, [(None, 2)]);
    lib.set_quality(bid, "fake", "{}", "k").unwrap();
    let q = TrackQuery {
        text: "cancion".into(),
        ..Default::default()
    };
    let p = lib.list_tracks(&q).unwrap();
    assert_eq!((p.total, p.tracks[0].title.as_str()), (1, "Canción"));
    assert_eq!(p.verdicts, [(None, 1)]);
    let ids: Vec<i64> = all.tracks.iter().map(|t| t.id).collect();
    assert_eq!(lib.count_existing(&ids).unwrap(), 2);
    assert_eq!(lib.count_existing(&[bid, bid, 999]).unwrap(), 1);
    // quality_job: stale key → needs analysis, matching key → done.
    assert!(lib.quality_job(bid).unwrap().is_some());
    let job = lib.quality_job(bid).unwrap().unwrap();
    lib.set_quality(bid, "fake", "{}", &job.key).unwrap();
    assert!(lib.quality_job(bid).unwrap().is_none());

    // Waveform cache: missing → job; stored → served; file changed on disk → stale again.
    assert_eq!(lib.waveform(bid).unwrap(), None);
    assert!(lib.tracks_needing_waveform(10).unwrap().iter().any(|j| j.id == bid));
    let job = lib.waveform_job(bid).unwrap().unwrap();
    assert!(job.key.starts_with(&format!("{WAVEFORM_REV}:")), "{}", job.key);
    lib.set_waveform(bid, &job.key, &[1, 2, 3]).unwrap();
    assert_eq!(lib.waveform(bid).unwrap().as_deref(), Some(&[1u8, 2, 3][..]));
    assert!(lib.waveform_job(bid).unwrap().is_none());
    assert!(lib.tracks_needing_waveform(10).unwrap().iter().all(|j| j.id != bid));
    lib.conn.execute("UPDATE tracks SET mtime = mtime + 1 WHERE id = ?1", [bid]).unwrap();
    assert_eq!(lib.waveform(bid).unwrap(), None, "stale after the file changed");
    assert!(lib.waveform_job(bid).unwrap().is_some());
    lib.set_waveform(bid, &lib.waveform_job(bid).unwrap().unwrap().key, &[]).unwrap();
    assert_eq!(lib.waveform(bid).unwrap().as_deref(), Some(&[][..]), "unreadable is stored, not retried");
}

#[test]
fn replaygain_tags_and_reread_of_old_rows() {
    let dir = temp_dir("rg");
    let covers = dir.join("covers");
    let a = dir.join("rg.wav");
    wav(&a, 800, Some(("T", "A", "Al")));
    let mut f = lofty::read_from_path(&a).unwrap();
    let tag = f.tag_mut(TagType::Id3v2).unwrap();
    tag.insert_text(ItemKey::ReplayGainTrackGain, "-7.25 dB".into());
    tag.insert_text(ItemKey::ReplayGainTrackPeak, "0.988525".into());
    tag.insert_text(ItemKey::ReplayGainAlbumGain, "+2.5 dB".into());
    f.save_to_path(&a, WriteOptions::default()).unwrap();

    let mut lib = Library::open(dir.join("lib.db")).unwrap();
    let no = AtomicBool::new(false);
    lib.scan(&[&a], &covers, &no, |_| {}).unwrap();
    let t = &lib.list_tracks(&TrackQuery::default()).unwrap().tracks[0];
    assert_eq!((t.rg_track_gain, t.rg_track_peak, t.rg_album_gain, t.rg_album_peak), (Some(-7.25), Some(0.988525), Some(2.5), None));

    // A row from before the ReplayGain columns (tags_rev 0) is re-read although the file did not change.
    lib.conn.execute("UPDATE tracks SET rg_track_gain = NULL, tags_rev = 0", []).unwrap();
    let r = lib.scan(&[&a], &covers, &no, |_| {}).unwrap();
    assert_eq!((r.unchanged, r.updated), (0, 1), "{r:?}");
    let r = lib.scan(&[&a], &covers, &no, |_| {}).unwrap();
    assert_eq!(r.unchanged, 1, "{r:?}");
    assert_eq!(lib.track_by_id(t.id).unwrap().unwrap().rg_track_gain, Some(-7.25));
    assert_eq!(scan::leading_number(" -0.5dB"), Some(-0.5));
    assert_eq!(scan::leading_number("abc"), None);
}

#[test]
fn opened_files_play_but_stay_out_of_the_library() {
    let dir = temp_dir("external");
    let (music, outside, loose, covers) = (dir.join("music"), dir.join("outside"), dir.join("loose"), dir.join("covers"));
    for d in [&music, &outside, &loose] {
        fs::create_dir_all(d).unwrap();
    }
    let (a, b) = (music.join("a.wav"), music.join("b.wav"));
    wav(&a, 800, Some(("Uno", "Mora", "Estrella")));
    wav(&b, 800, Some(("Dos", "Mora", "Estrella")));
    let x = outside.join("x.wav");
    wav(&x, 800, Some(("Fuera", "Zeta", "Lejos")));
    let [y, z, w] = ["y", "z", "w"].map(|n| loose.join(format!("{n}.wav")));
    for (f, title) in [(&y, "Ye"), (&z, "Zed"), (&w, "Uve")] {
        wav(f, 800, Some((title, "Suelto", "Suelto")));
    }

    let mut lib = Library::open(dir.join("lib.db")).unwrap();
    let no = AtomicBool::new(false);
    lib.add_root(&music.to_string_lossy()).unwrap();
    lib.scan(&lib.roots().unwrap(), &covers, &no, |_| {}).unwrap();
    let id = |lib: &Library, p: &Path| lib.id_for_path(&p.to_string_lossy()).unwrap();
    let (aid, bid) = (id(&lib, &a).unwrap(), id(&lib, &b).unwrap());

    // Opening an outside file and a library one: only the outside one is new, and external.
    let r = lib.scan_external(&[&x, &a], &covers).unwrap();
    assert_eq!((r.added, r.unchanged), (1, 1), "{r:?}");
    let xid = id(&lib, &x).unwrap();
    assert!(!lib.track_by_id(xid).unwrap().unwrap().in_library);
    assert!(lib.track_by_id(aid).unwrap().unwrap().in_library, "opening a library track keeps it there");
    assert_eq!(lib.tracks_by_ids(&[xid, aid]).unwrap().len(), 2, "the queue still finds it");

    // Hidden from every library view.
    let all = lib.list_tracks(&TrackQuery::default()).unwrap();
    assert_eq!((all.total, all.tracks.len()), (2, 2));
    assert!(!lib.view_ids(&TrackQuery::default()).unwrap().contains(&xid));
    assert!(lib.search("fuera", 10).unwrap().is_empty());
    assert!(lib.search_all("zeta", 10).unwrap().is_empty(), "no track, artist...");
    assert!(lib.search_all("lejos", 10).unwrap().is_empty(), "...nor album hit");
    assert_eq!(lib.albums().unwrap().len(), 1);
    assert!(lib.artists().unwrap().iter().all(|a| a.name != "Zeta"));
    let tree = lib.folders().unwrap();
    assert_eq!((tree.len(), tree[0].total_count), (1, 2));
    let rules = |title: &str| smart::parse(&format!(r#"{{"any": [{{"field": "title", "op": "is", "value": "{title}"}}]}}"#)).unwrap();
    assert!(lib.smart_ids(&rules("Fuera")).unwrap().is_empty());
    assert_eq!(lib.smart_ids(&rules("Uno")).unwrap(), [aid]);
    assert!(lib.radio(xid, 10, &mut radio::Rng::new(1)).is_err(), "not a radio candidate");
    assert!(lib.radio(aid, 10, &mut radio::Rng::new(1)).unwrap().iter().all(|&t| t != xid));
    // No background passes for it, but its own analysis still runs while it plays.
    assert!(lib.tracks_needing_quality(10).unwrap().iter().all(|j| j.id != xid));
    assert!(lib.tracks_needing_features(10).unwrap().iter().all(|j| j.id != xid));
    assert!(lib.tracks_needing_waveform(10).unwrap().iter().all(|j| j.id != xid));
    assert!(lib.quality_job(xid).unwrap().is_some() && lib.waveform_job(xid).unwrap().is_some());

    // Its plays count in the stats; the library share only counts library tracks (2 of 2).
    for t in [aid, xid] {
        lib.conn.execute("INSERT INTO plays (track_id, played_at, ms_listened) VALUES (?1, 1740000000, 1000)", [t]).unwrap();
    }
    let wr = lib.wrapped(2025).unwrap();
    assert_eq!((wr.stats.tracks, wr.library_pct), (2, 100.0));

    // Pruning: only externals nothing refers to (y). z was played, w is in a playlist.
    lib.scan_external(&[&y, &z, &w], &covers).unwrap();
    lib.record_play(id(&lib, &z).unwrap(), 1000).unwrap();
    let p = lib.create_playlist("Sueltos", PlaylistKind::Manual, None).unwrap();
    lib.add_to_playlist(p, &[id(&lib, &w).unwrap()]).unwrap();
    assert_eq!(lib.prune_external().unwrap(), 1);
    assert_eq!(id(&lib, &y), None);
    assert!([&a, &b, &x, &z, &w].iter().all(|f| id(&lib, f).is_some()));
    assert_eq!(lib.prune_external().unwrap(), 0);
    assert_eq!(lib.wrapped(2025).unwrap().stats.tracks, 2, "stats unchanged");

    // Adding the folder that holds an opened file: the rescan takes it in, same row.
    lib.add_root(&outside.to_string_lossy()).unwrap();
    let r = lib.scan(&lib.roots().unwrap(), &covers, &no, |_| {}).unwrap();
    assert_eq!((r.added, r.missing), (0, 0), "{r:?}");
    assert_eq!(id(&lib, &x), Some(xid));
    assert!(lib.track_by_id(xid).unwrap().unwrap().in_library);
    assert_eq!(lib.list_tracks(&TrackQuery::default()).unwrap().total, 3);
    assert!(lib.artists().unwrap().iter().any(|a| a.name == "Zeta"));
    assert!(!lib.track_by_id(id(&lib, &z).unwrap()).unwrap().unwrap().in_library, "outside the roots: still external");

    // The migration marks rows of older versions outside every root as external.
    lib.conn.execute("UPDATE tracks SET in_library = 1", []).unwrap();
    let mig = MIGRATIONS[6];
    lib.conn.execute_batch(&mig[mig.find("UPDATE").unwrap()..]).unwrap();
    let ext: Vec<bool> = [aid, bid, xid].iter().map(|&t| lib.track_by_id(t).unwrap().unwrap().in_library).collect();
    assert_eq!(ext, [true, true, true]);
    assert_eq!(lib.list_tracks(&TrackQuery::default()).unwrap().total, 3, "z and w stay hidden");
}

#[test]
fn removed_folders_leave_the_library_and_can_come_back() {
    let dir = temp_dir("rmfolder");
    let music = dir.join("music");
    let covers = dir.join("covers");
    fs::create_dir_all(music.join("sub")).unwrap();
    wav(&music.join("a.wav"), 8000, Some(("A", "X", "Uno")));
    wav(&music.join("sub").join("b.wav"), 8000, Some(("B", "X", "Dos")));
    let mut lib = Library::open(dir.join("lib.db")).unwrap();
    let no = AtomicBool::new(false);
    let m = music.to_string_lossy().into_owned();
    let sub = music.join("sub").to_string_lossy().into_owned();
    let total = |lib: &Library| lib.list_tracks(&TrackQuery::default()).unwrap().total;
    lib.add_root(&m).unwrap();
    lib.scan(&lib.roots().unwrap(), &covers, &no, |_| {}).unwrap();
    assert_eq!(total(&lib), 2);

    // A folder inside a music folder: out now and skipped by later scans.
    lib.remove_folder(&sub).unwrap();
    assert_eq!(total(&lib), 1);
    lib.scan(&lib.roots().unwrap(), &covers, &no, |_| {}).unwrap();
    assert_eq!(total(&lib), 1, "the rescan keeps it out");
    // Adding it again brings it back.
    assert!(lib.add_root(&sub).unwrap());
    assert_eq!(lib.roots().unwrap(), [m.clone()]);
    lib.scan(&lib.roots().unwrap(), &covers, &no, |_| {}).unwrap();
    assert_eq!(total(&lib), 2);

    // The music folder itself: forgotten, nothing left.
    lib.remove_folder(&format!("{m}{}", std::path::MAIN_SEPARATOR)).unwrap();
    assert!(lib.roots().unwrap().is_empty());
    assert_eq!(total(&lib), 0);
    assert!(lib.remove_folder(&m).is_err(), "not in the library");
}
