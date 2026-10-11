//! Re-runs only the chord logic (the chord layers, `ChordEngine`) over notes recorded in a
//! `rekord_engine` file-mode stdout (`raw/NNN.txt`), stepping `covered` in 0.256 s blocks the
//! way the file mode does, so a parameter sweep over 100 songs takes seconds instead of the
//! model's half hour. Writes `<out_dir>/NNN.txt` with the `t=... chord: ...` lines of one layer,
//! each followed by `emit=...`: the checkpoint at which the layer announced the change (`t` is
//! when the chord starts, which may be earlier), so a delay can be measured.
//!
//! cargo run --release -p rekord_core --example chord_replay -- <raw_dir> <out_dir> [NNN ...]
//!
//! Which layer is written, and how it is tuned, comes from `REKORD_CHORD_TUNE` (comma-separated
//! key=value; the other layers keep their defaults):
//!   layer=settled (default) | instant | confirmed
//!   w=<window s> d=<decay s, 0 = flat window> h=<hold s> b=<backdate s> m=<margin>
//!   sil=<silence hold s> c=<0 full vocabulary | 1 no slash | 2 no slash, triads only> snap=<0|1>
//! (see `HoldParams::apply_tune`; `layer=confirmed` runs the 2 s layer (also run when an event log
//! is written) and takes the keys of `ConfirmedParams::apply_tune`: hop w f D p pb g k sil name kl
//! mem snap). `<out_dir>/NNN.key` lists the key changes (`<time> <name>`);
//! `REKORD_KEY_TUNE` tunes the key estimator (`KeyParams::apply_tune`) and
//! `REKORD_KEY_ORACLE="0:C,60:A,73:C"` replaces it with the given major keys from the given
//! times (what the naming would do with a perfect key). With `REKORD_CHORD_EVENTS_OUT=<dir>` it also writes
//! `<dir>/NNN.jsonl`: every event of a replayed session (notes, chord layers, active notes, key),
//! one JSON line each with `wall` = when it would arrive -- for the frontend's replay mock
//! (`rekord-piano-roll/dev/chords.html`).

use rekord_core::chord_layers::{ChordEngine, ChordLayerId, ConfirmedParams, HoldParams};
use rekord_core::chord_templates;
use rekord_core::harmonica_decode::ConfirmedNote;
use rekord_core::harmonica_model::{Device, ModelSize, Part};
use rekord_core::key_estimator::{KeyEstimate, KeyEstimator, Mode};
use rekord_core::pipeline::{KeyMode, NoteModelInfo, PipelineEvent};
use rekord_core::spelling::Accidental;
use std::fmt::Write as _;

// How long after `covered` a block's events would reach the frontend (the capture hop and the
// compute), for the event log's `wall` times.
const ARRIVAL_LAG_S: f32 = 0.4;

fn pc_of(name: &str) -> i32 {
    let mut chars = name.chars();
    let base = match chars.next().unwrap() {
        'C' => 0, 'D' => 2, 'E' => 4, 'F' => 5, 'G' => 7, 'A' => 9, _ => 11,
    };
    match chars.next() {
        Some('♯') | Some('#') => base + 1,
        Some('♭') => base - 1,
        _ => base,
    }
}

/// `[  0.06s ->   0.19s] note F♯4 (vel 0.57) [other]`
fn parse(line: &str) -> Option<(f32, f32, i32, f32, Part)> {
    let rest = line.strip_prefix('[')?;
    let (times, rest) = rest.split_once("] note ")?;
    let (s, e) = times.split_once("->")?;
    let s: f32 = s.trim().trim_end_matches('s').parse().ok()?;
    let e: f32 = e.trim().trim_end_matches('s').parse().ok()?;
    let (name, rest) = rest.split_once(" (vel ")?;
    let (vel, tag) = rest.split_once(") [")?;
    let octave: i32 = name.trim_start_matches(|c: char| c.is_alphabetic() || c == '♯' || c == '♭' || c == '#').parse().ok()?;
    let letters: String = name.chars().take_while(|c| !c.is_ascii_digit() && *c != '-').collect();
    let part = match tag.trim_end_matches(']').trim().split_whitespace().next()? {
        "vocal" => Part::Vocal,
        "bass" => Part::Bass,
        "other" => Part::Other,
        _ => Part::All,
    };
    Some((s, e, 12 * (octave + 1) + pc_of(&letters), vel.parse().ok()?, part))
}

/// `REKORD_CHORD_TUNE`: which layer to write and the parameters of the layers.
fn tune() -> (ChordLayerId, HoldParams, HoldParams, Option<ConfirmedParams>) {
    let (mut instant, mut settled) = (HoldParams::instant(), HoldParams::settled());
    let mut confirmed = ConfirmedParams::standard();
    let mut chosen = ChordLayerId::Settled;
    let tune = std::env::var("REKORD_CHORD_TUNE").unwrap_or_default();
    let mut rest = Vec::new();
    for kv in tune.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match kv.strip_prefix("layer=") {
            Some("instant") => chosen = ChordLayerId::Instant,
            Some("settled") => chosen = ChordLayerId::Settled,
            Some("confirmed") => chosen = ChordLayerId::Confirmed,
            Some(other) => panic!("REKORD_CHORD_TUNE: unknown layer {other:?}"),
            None => rest.push(kv),
        }
    }
    let rest = rest.join(",");
    match chosen {
        ChordLayerId::Instant => instant.apply_tune(&rest),
        ChordLayerId::Settled => settled.apply_tune(&rest),
        ChordLayerId::Confirmed => confirmed.apply_tune(&rest),
    }
    .unwrap_or_else(|e| panic!("REKORD_CHORD_TUNE: {e}"));
    (chosen, instant, settled, Some(confirmed))
}

/// `REKORD_KEY_ORACLE="0:C,60:A,73:C"`: from each time on, this major key instead of the
/// estimator's (what the chord naming would do with a perfect key). Applies to every song.
fn key_oracle() -> Vec<(f32, KeyEstimate)> {
    let spec = std::env::var("REKORD_KEY_ORACLE").unwrap_or_default();
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (from, tonic) = kv.split_once(':').unwrap_or_else(|| panic!("REKORD_KEY_ORACLE: expected time:key, got {kv:?}"));
            (from.parse().unwrap(), KeyEstimate { tonic_pc: pc_of(tonic).rem_euclid(12), mode: Mode::Major })
        })
        .collect()
}

fn main() {
    let (chosen, instant, settled, confirmed) = tune();
    let oracle = key_oracle();
    let events_out = std::env::var("REKORD_CHORD_EVENTS_OUT").ok();
    let args: Vec<String> = std::env::args().collect();
    let (raw_dir, out_dir) = (&args[1], &args[2]);
    std::fs::create_dir_all(out_dir).unwrap();
    if let Some(dir) = &events_out {
        std::fs::create_dir_all(dir).unwrap();
    }
    let ids: Vec<String> = if args.len() > 3 {
        args[3..].iter().map(|s| format!("{:03}", s.parse::<u32>().unwrap())).collect()
    } else {
        let mut v: Vec<String> = std::fs::read_dir(raw_dir)
            .unwrap()
            .filter_map(|e| e.ok()?.path().file_stem()?.to_str().map(String::from))
            .collect();
        v.sort();
        v
    };
    for id in ids {
        let text = std::fs::read_to_string(format!("{raw_dir}/{id}.txt")).unwrap();
        let notes: Vec<_> = text.lines().filter_map(parse).collect();
        let end = notes.iter().map(|n| n.1).fold(0.0f32, f32::max) + 1.0;
        let mut out = String::new();
        let mut keys = String::new(); // `<time> <key name>` per key change, written to NNN.key
        let mut log = String::new(); // the event log (only filled with REKORD_CHORD_EVENTS_OUT)
        // (the 2 s layer costs a third more time; only run it when it is wanted)
        let mut engine = match confirmed {
            Some(confirmed) if chosen == ChordLayerId::Confirmed || events_out.is_some() => {
                ChordEngine::with_confirmed(instant, settled, confirmed)
            }
            _ => ChordEngine::with_params(instant, settled),
        };
        let mut key = KeyEstimator::new();
        let mut last_key = None;
        let mut prev = 0.0f32;
        let mut k = 1;
        let mut fed = vec![false; notes.len()];
        let mut started = vec![false; notes.len()];
        let keep_log = events_out.is_some();
        let log_event = |wall: f32, event: &PipelineEvent, log: &mut String| {
            if keep_log {
                writeln!(log, "{{\"wall\":{wall:.4},\"event\":{}}}", serde_json::to_string(event).unwrap()).unwrap();
            }
        };
        log_event(
            0.0,
            &PipelineEvent::Started {
                total_latency_s: 0.88,
                note_model: NoteModelInfo { size: ModelSize::Large, device: Device::DirectML, future_frames: 8, over_budget: false },
                chord_vocabulary: chord_templates::vocabulary(),
            },
            &mut log,
        );
        loop {
            let covered = (k as f32 * 0.256).min(end);
            let wall = covered + ARRIVAL_LAG_S;
            let mut events: Vec<_> =
                notes.iter().filter(|n| n.0 <= covered).map(|&(s, e, p, v, part)| (s, e.min(covered), p, v, part)).collect();
            events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            // The key estimator is fed each note once it has ended, like the file mode does.
            for (i, n) in notes.iter().enumerate() {
                if !started[i] && n.0 <= covered {
                    started[i] = true;
                    log_event(wall, &PipelineEvent::NoteStarted { start_s: n.0, pitch_midi: n.2, velocity: n.3, part: n.4 }, &mut log);
                }
                if !fed[i] && n.1 <= covered {
                    fed[i] = true;
                    let note = ConfirmedNote { start_s: n.0, end_s: n.1, pitch_midi: n.2, part: n.4, velocity: n.3 };
                    key.observe(&note);
                    log_event(
                        wall,
                        &PipelineEvent::NoteConfirmed {
                            start_s: n.0,
                            end_s: n.1,
                            pitch_midi: n.2,
                            velocity: n.3,
                            part: n.4,
                            melody: false,
                            bass: false,
                        },
                        &mut log,
                    );
                }
            }
            let mut engine_events = Vec::new();
            let key_mode = match oracle.iter().rev().find(|(from, _)| *from <= covered) {
                Some((_, k)) => KeyMode::Manual(*k),
                None => KeyMode::Auto,
            };
            engine.advance(&events, prev, covered, &mut key, &key_mode, Accidental::Sharp, None, &mut last_key, |e| engine_events.push(e));
            prev = covered;
            let done = covered >= end;
            if done {
                engine.finish(covered, Accidental::Sharp, |e| engine_events.push(e));
            }
            for e in &engine_events {
                if let PipelineEvent::KeyChange { t, key_name } = e {
                    writeln!(keys, "{t:.2} {key_name}").unwrap();
                }
                if let PipelineEvent::ChordLayer { layer, changes, .. } = e {
                    if *layer == chosen {
                        for c in changes {
                            writeln!(out, "  t={:6.2}s  chord: {}  emit={:.2}", c.t, c.label, c.emit_s).unwrap();
                        }
                    }
                }
                log_event(wall, e, &mut log);
            }
            if done {
                break;
            }
            k += 1;
        }
        std::fs::write(format!("{out_dir}/{id}.txt"), out).unwrap();
        std::fs::write(format!("{out_dir}/{id}.key"), keys).unwrap();
        if let Some(dir) = &events_out {
            std::fs::write(format!("{dir}/{id}.jsonl"), log).unwrap();
        }
    }
}
