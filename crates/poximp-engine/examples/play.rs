//! Headless engine check: plays, seeks, and crosses a gapless boundary, printing events.
//!
//! cargo run -p poximp-engine --example play -- <file> [next_file] [seconds]
//! POXIMP_BITPERFECT=1 opens the device in exclusive mode at each file's format;
//! POXIMP_DEVICE=<id> picks another output (the ids are printed).
//! POXIMP_CROSSFADE=<secs> crossfades into next_file (TrackChanged comes that much earlier).

use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use poximp_engine::{Command, Engine, Event};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = args.first() else {
        eprintln!("usage: play <file> [next_file] [seconds]");
        std::process::exit(2);
    };
    let second = args.get(1);
    let secs: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let mut ok = true;

    let (engine, events) = Engine::new();
    engine.send(Command::SetVolume(0.5));
    // POXIMP_BITPERFECT=1: exclusive at the file's format (volume fixed at 100 %: use quiet files).
    // POXIMP_DEVICE=<id from the list printed here>: another output than the system default.
    let device = std::env::var("POXIMP_DEVICE").ok();
    let bit_perfect = std::env::var_os("POXIMP_BITPERFECT").is_some();
    if device.is_some() || bit_perfect {
        for d in poximp_engine::output_devices() {
            println!("device {}{}: {}", d.id, if d.is_default { " (default)" } else { "" }, d.name);
        }
        engine.send(Command::SetOutput { device, bit_perfect });
    }
    engine.send(Command::Load { path: first.into(), autoplay: true, gain_db: 0.0 });
    let t0 = Instant::now();
    let log = pump(&events, secs, t0);
    println!("output: {:?}", engine.output_info());
    let duration = log.iter().find_map(|(_, e)| match e {
        Event::Position { duration, .. } => Some(*duration),
        _ => None,
    });
    // Measured from the first position event (device opening is not playback time).
    let timed: Vec<(f64, f64)> = log
        .iter()
        .filter_map(|(t, e)| match e {
            Event::Position { secs, .. } => Some((*t, *secs)),
            _ => None,
        })
        .collect();
    let (first, last) = (timed.first().copied().unwrap_or_default(), timed.last().copied().unwrap_or_default());
    let (advanced, wall) = (last.1 - first.1, last.0 - first.0);
    let advance_ok = wall > 0.5 && (advanced - wall).abs() < 0.1;
    ok &= advance_ok;
    println!("CHECK position advances: +{advanced:.3} s audio in {wall:.3} s wall -> {}", pass(advance_ok));
    let bands = engine.spectrum(32);
    println!("spectrum(32): {bands:?}");

    let duration = duration.unwrap_or(0.0);
    let target = (duration / 2.0).min(30.0);
    engine.send(Command::Seek(target));
    let t1 = Instant::now();
    let log = pump(&events, 1.0, t1);
    let after: Vec<(f64, f64)> = log
        .iter()
        .filter_map(|(t, e)| match e {
            Event::Position { secs, .. } => Some((*t, *secs)),
            _ => None,
        })
        .collect();
    let landed = after.first().map(|p| p.1).unwrap_or(-1.0);
    let seek_ok = (landed - target).abs() <= 0.05;
    ok &= seek_ok;
    println!("CHECK seek lands: target {target:.3}, reported {landed:.3} -> {}", pass(seek_ok));
    let (wall, pos) = after.last().copied().unwrap_or((0.0, 0.0));
    let drift = (pos - target) - wall;
    let resume_ok = drift.abs() < 0.1;
    ok &= resume_ok;
    println!("CHECK plays on from the seek point: +{:.3} s audio in {wall:.3} s wall (diff {drift:+.3}) -> {}", pos - target, pass(resume_ok));

    if let Some(second) = second {
        let xf: f64 = std::env::var("POXIMP_CROSSFADE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        engine.send(Command::Enqueue { path: second.into(), gain_db: 0.0, crossfade: xf });
        let lead = 2.0;
        engine.send(Command::Seek(duration - lead));
        let t2 = Instant::now();
        let log = pump(&events, lead + 1.5, t2);
        let changed = log.iter().find_map(|(t, e)| match e {
            Event::TrackChanged { path } if path.to_string_lossy() == second.as_str() => Some(*t),
            _ => None,
        });
        let ended = log.iter().any(|(_, e)| matches!(e, Event::TrackEnded));
        let expected = lead - xf.min(lead);
        let gapless_ok = changed.is_some_and(|t| (t - expected).abs() < 0.15) && !ended;
        ok &= gapless_ok;
        println!("CHECK gapless TrackChanged: after {changed:?} s (expected ~{expected:.1}), TrackEnded={ended} -> {}", pass(gapless_ok));
        println!("output now: {:?}", engine.output_info());
        let new_pos = positions(&log).last().copied().unwrap_or(-1.0);
        let new_ok = changed.is_some_and(|t| (new_pos - (lead + 1.5 - t)).abs() < 0.3);
        ok &= new_ok;
        println!("CHECK second track position restarts: {new_pos:.3} s -> {}", pass(new_ok));
    }

    engine.send(Command::Stop);
    pump(&events, 0.2, Instant::now());
    println!("{}", if ok { "ALL PASS" } else { "SOME CHECKS FAILED" });
    std::process::exit(if ok { 0 } else { 1 });
}

/// Prints and collects events for `secs`, timestamped from `t0`.
fn pump(events: &Receiver<Event>, secs: f64, t0: Instant) -> Vec<(f64, Event)> {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut log = Vec::new();
    while let Ok(e) = events.recv_deadline(end) {
        let t = t0.elapsed().as_secs_f64();
        println!("[{t:7.3}] {e:?}");
        log.push((t, e));
    }
    log
}

fn positions(log: &[(f64, Event)]) -> Vec<f64> {
    log.iter()
        .filter_map(|(_, e)| match e {
            Event::Position { secs, .. } => Some(*secs),
            _ => None,
        })
        .collect()
}

fn pass(ok: bool) -> &'static str {
    if ok { "PASS" } else { "FAIL" }
}
