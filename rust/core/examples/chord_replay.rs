//! Re-runs only the chord logic (window chroma, matching, stabilizer) over notes recorded in a
//! `rekord_engine` file-mode stdout (`raw/NNN.txt`), stepping `covered` in 0.256 s blocks the
//! way the file mode does, so a parameter sweep over 100 songs takes seconds instead of the
//! model's half hour. Writes `<out_dir>/NNN.txt` with the `t=... chord: ...` lines only.
//!
//! cargo run --release -p rekord_core --example chord_replay -- <raw_dir> <out_dir> [NNN ...]

use rekord_core::harmonica_decode::ConfirmedNote;
use rekord_core::harmonica_model::Part;
use rekord_core::key_estimator::KeyEstimator;
use rekord_core::pipeline::{self, ChordStabilizer, KeyMode, PipelineEvent};
use rekord_core::spelling::Accidental;
use std::fmt::Write as _;

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (raw_dir, out_dir) = (&args[1], &args[2]);
    std::fs::create_dir_all(out_dir).unwrap();
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
        let mut stab = ChordStabilizer::new();
        let mut key = KeyEstimator::new();
        let mut last_key = None;
        let mut prev = 0.0f32;
        let mut k = 1;
        let mut fed = vec![false; notes.len()];
        loop {
            let covered = (k as f32 * 0.256).min(end);
            let mut events: Vec<_> =
                notes.iter().filter(|n| n.0 <= covered).map(|&(s, e, p, v, part)| (s, e.min(covered), p, v, part)).collect();
            events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            // The key estimator is fed each note once it has ended, like the file mode does.
            for (i, n) in notes.iter().enumerate() {
                if !fed[i] && n.1 <= covered {
                    fed[i] = true;
                    key.observe(&ConfirmedNote { start_s: n.0, end_s: n.1, pitch_midi: n.2, part: n.4, velocity: n.3 });
                }
            }
            pipeline::report_new_chords(&events, prev, covered, &mut stab, &mut key, &KeyMode::Auto, Accidental::Sharp, None, &mut last_key, |e| {
                if let PipelineEvent::ChordChange { t, label, .. } = e {
                    writeln!(out, "  t={t:6.2}s  chord: {label}").unwrap();
                }
            });
            prev = covered;
            if covered >= end {
                break;
            }
            k += 1;
        }
        std::fs::write(format!("{out_dir}/{id}.txt"), out).unwrap();
    }
}
