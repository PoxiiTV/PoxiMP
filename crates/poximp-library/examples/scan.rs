//! cargo run -p poximp-library --example scan --release -- <music dir> [search text]
//! Scans into a fresh DB in the temp dir, prints what it found, then rescans to show the
//! incremental path.

use poximp_library::{Library, TrackQuery};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().ok_or("usage: scan <music dir> [search text]")?;
    let query = args.next().unwrap_or_else(|| "cancion".into());
    let work = std::env::temp_dir().join("poximp-scan-example");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;

    let mut lib = Library::open(work.join("library.db"))?;
    let cancel = AtomicBool::new(false);
    let covers = work.join("covers");
    let first = lib.scan(&[&dir], &covers, &cancel, |_| {})?;
    println!("first scan:  {first:?}");

    let page = lib.list_tracks(&TrackQuery::default())?;
    println!("\n{} tracks:", page.total);
    for t in &page.tracks {
        println!(
            "  [{}] {} - {} | {} | {} {} {} Hz {} bit {} ch {} kbps {} ms | cover {:?}",
            t.id,
            t.artist,
            t.title,
            t.album,
            t.container,
            t.codec,
            t.sample_rate.unwrap_or(0),
            t.bit_depth.unwrap_or(0),
            t.channels.unwrap_or(0),
            t.bitrate_kbps.unwrap_or(0),
            t.duration_ms,
            t.cover,
        );
        let split: Vec<String> = lib
            .artists_of(t.id)?
            .into_iter()
            .map(|a| format!("{} ({})", a.name, a.role))
            .collect();
        if !split.is_empty() {
            println!("      artists: {}", split.join(", "));
        }
    }
    println!("\nalbums:");
    for a in lib.albums()? {
        println!("  {} - {} ({} tracks, cover {:?})", a.artist.as_deref().unwrap_or("<varios>"), a.title, a.track_count, a.cover);
    }
    let artists: Vec<String> = lib.artists()?.into_iter().map(|a| format!("{} ({})", a.name, a.track_count)).collect();
    println!("\nartists: {}", artists.join(" | "));
    println!("\nfolders: {:#?}", lib.folders()?);

    let t = Instant::now();
    let hits = lib.search(&query, 20)?;
    println!("\nsearch {query:?}: {} hits in {} us", hits.len(), t.elapsed().as_micros());
    for h in hits {
        println!("  {} - {}", h.artist, h.title);
    }

    let again = lib.scan(&[&dir], &covers, &cancel, |_| {})?;
    println!("\nrescan:      {again:?}");
    Ok(())
}
