mod audio_io;
mod chord_templates;
mod live_capture;
mod note_creation;
mod note_stitcher;
mod onnx_model;
mod streaming_extractor;

use std::collections::BTreeSet;

use anyhow::{bail, Context, Result};

use chord_templates::match_chord;
use note_creation::NoteEvent;
use note_stitcher::NoteStitcher;
use onnx_model::OnnxNoteModel;
use streaming_extractor::{
    StreamingNoteExtractor, AUDIO_SAMPLE_RATE, MAX_FRESH_EXTRACTOR_REAL_SAMPLES,
};

const MIDI_NOTE_NAMES: [&str; 12] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

// A note's end_s can be a touch before the true edge of what's been
// processed (edge effect from limited context there).
const ACTIVE_EDGE_TOLERANCE_S: f32 = 0.2;

fn midi_to_name(pitch: i32) -> String {
    format!("{}{}", MIDI_NOTE_NAMES[pitch.rem_euclid(12) as usize], pitch.div_euclid(12) - 1)
}

/// Chord label for whatever's sounding at time t: Some("(silence)") if
/// nothing is, None if notes are present but nothing matched well enough
/// (ambiguous -- caller should just keep showing the prior label).
fn chord_label_at(note_events: &[NoteEvent], t: f32, tolerance: f32) -> Option<String> {
    let active: Vec<i32> = note_events
        .iter()
        .filter(|&&(s, e, _p, _a)| s <= t + 1e-6 && e >= t - tolerance)
        .map(|&(_s, _e, p, _a)| p)
        .collect();
    if active.is_empty() {
        return Some("(silence)".to_string());
    }
    let bass_pc = active.iter().min().map(|p| p.rem_euclid(12));
    let pitch_classes: Vec<i32> = active.iter().map(|p| p.rem_euclid(12)).collect();
    match_chord(&pitch_classes, bass_pc).map(|c| c.label())
}

/// Windows complete in ~1.6s jumps, so a single "what's active now" check
/// can skip right over a chord shorter than that. Re-evaluate the chord at
/// every note onset within the newly covered span instead.
fn report_new_chords(
    note_events: &[NoteEvent],
    prev_covered: f32,
    covered: f32,
    last_label: &mut Option<String>,
) {
    let mut checkpoints: BTreeSet<i64> = note_events
        .iter()
        .map(|&(s, _e, _p, _a)| s)
        .filter(|&s| s > prev_covered && s <= covered)
        .map(|s| (s * 1000.0).round() as i64)
        .collect();
    checkpoints.insert((covered * 1000.0).round() as i64);

    for t_key in checkpoints {
        let t = t_key as f32 / 1000.0;
        if let Some(label) = chord_label_at(note_events, t, ACTIVE_EDGE_TOLERANCE_S) {
            if Some(&label) != last_label.as_ref() {
                println!("  t={t:6.2}s  chord: {label}");
                *last_label = Some(label);
            }
        }
    }
}

fn print_confirmed_notes(stitcher: &mut NoteStitcher, covered: f32, note_events: &[NoteEvent]) {
    for n in stitcher.update(covered, note_events) {
        println!("[{:6.2}s -> {:6.2}s] note {}", n.start_s, n.end_s, midi_to_name(n.pitch_midi));
    }
}

fn run_file(audio_path: &str, hop_seconds: f32) -> Result<()> {
    println!("Loading {audio_path} ...");
    let audio = audio_io::load_audio(audio_path)?;
    let duration = audio.len() as f32 / AUDIO_SAMPLE_RATE as f32;
    println!("Duration: {duration:.1}s, simulating stream with {hop_seconds:.2}s hops\n");

    let mut model = OnnxNoteModel::load("models/nmp.onnx").context("loading ONNX model")?;
    let mut stitcher = NoteStitcher::new();
    let mut last_chord_label: Option<String> = None;
    let mut last_covered = 0.0f32;

    let hop_samples = (hop_seconds * AUDIO_SAMPLE_RATE as f32).round() as usize;
    let main_final_notes = {
        let mut extractor = StreamingNoteExtractor::new(&mut model);
        let mut pos = 0usize;
        while pos < audio.len() {
            let end = (pos + hop_samples).min(audio.len());
            let completed = extractor.push(&audio[pos..end])?;
            pos = end;
            if completed > 0 {
                let note_events = extractor.note_events();
                let covered = extractor.covered_seconds();
                print_confirmed_notes(&mut stitcher, covered, &note_events);
                report_new_chords(&note_events, last_covered, covered, &mut last_chord_label);
                last_covered = covered;
            }
        }
        // Kept for the tail-reconciliation chord check below: the tail
        // pass re-analyzes its own short clip from scratch and otherwise
        // has no memory of notes the main loop already established.
        extractor.note_events()
    };

    // The tail shorter than one window (~1.6s) never completes on its own.
    // Re-run extraction on the real last bit of audio (no padding needed)
    // instead of zero-padding, which showed the model a mostly-synthetic-
    // silence window and suppressed real notes there (verified empirically
    // in the Python prototype). Cap what's fed in at
    // MAX_FRESH_EXTRACTOR_REAL_SAMPLES: a fresh extractor's buffer already
    // starts with some front-padding, so pushing a full window's worth of
    // real audio on top of that makes `push` complete a window on its own
    // before `finish` ever runs, leaving `finish` to pad a second, mostly-
    // synthetic window instead of the small bounded pad it's meant for.
    if last_covered < duration && !audio.is_empty() {
        let clip_start = audio.len().saturating_sub(MAX_FRESH_EXTRACTOR_REAL_SAMPLES);
        let clip_start_s = clip_start as f32 / AUDIO_SAMPLE_RATE as f32;
        let mut tail_extractor = StreamingNoteExtractor::new(&mut model);
        tail_extractor.push(&audio[clip_start..])?;
        tail_extractor.finish()?;
        let tail_notes: Vec<NoteEvent> = tail_extractor
            .note_events()
            .into_iter()
            .map(|(s, e, p, a)| (s + clip_start_s, e + clip_start_s, p, a))
            .collect();

        // Combine with what the main loop already knew, so a note the
        // tail's independent re-analysis fails to re-surface doesn't
        // silently vanish from the final chord/notes report.
        let mut combined_notes = main_final_notes;
        combined_notes.extend(tail_notes);

        print_confirmed_notes(&mut stitcher, duration, &combined_notes);
        report_new_chords(&combined_notes, last_covered, duration, &mut last_chord_label);
    }

    Ok(())
}

fn run_live(hop_seconds: f32, duration_s: Option<f32>) -> Result<()> {
    println!("Listening to default speaker output via loopback capture ...");
    match duration_s {
        Some(d) => println!("Will stop after {d:.1}s (Ctrl+C to stop earlier)\n"),
        None => println!("Press Ctrl+C to stop.\n"),
    }

    let mut model = OnnxNoteModel::load("models/nmp.onnx").context("loading ONNX model")?;
    let mut stitcher = NoteStitcher::new();
    let mut last_chord_label: Option<String> = None;
    let mut last_covered = 0.0f32;
    let mut elapsed = 0.0f32;

    let (_stream, rx) = live_capture::spawn_loopback_capture(hop_seconds)?;
    let mut extractor = StreamingNoteExtractor::new(&mut model);
    while let Ok(chunk) = rx.recv() {
        let completed = extractor.push(&chunk)?;
        elapsed += hop_seconds;
        if completed > 0 {
            let note_events = extractor.note_events();
            let covered = extractor.covered_seconds();
            print_confirmed_notes(&mut stitcher, covered, &note_events);
            report_new_chords(&note_events, last_covered, covered, &mut last_chord_label);
            last_covered = covered;
        }
        if let Some(d) = duration_s {
            if elapsed >= d {
                break;
            }
        }
    }

    // NoteStitcher deliberately holds back anything within the last
    // CONFIRM_MARGIN_S of "covered" time, in case more audio revises it.
    // Unlike run_file (which has a tail-reconciliation pass that re-covers
    // that span), a live session has no more audio coming once the loop
    // above exits -- flush what's left so it isn't silently lost.
    let final_notes = extractor.note_events();
    for n in stitcher.flush(&final_notes) {
        println!("[{:6.2}s -> {:6.2}s] note {}", n.start_s, n.end_s, midi_to_name(n.pitch_midi));
    }
    report_new_chords(&final_notes, last_covered, extractor.covered_seconds(), &mut last_chord_label);

    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut audio_file: Option<String> = None;
    let mut hop = 0.5f32;
    let mut live = false;
    let mut duration: Option<f32> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--hop" => {
                i += 1;
                let value = args.get(i).context("--hop expects a number")?;
                hop = value.parse().context("--hop expects a number")?;
                if !(hop > 0.0) {
                    bail!("--hop must be greater than 0, got {hop}");
                }
            }
            "--live" => live = true,
            "--duration" => {
                i += 1;
                let value = args.get(i).context("--duration expects a number")?;
                let parsed: f32 = value.parse().context("--duration expects a number")?;
                if !(parsed > 0.0) {
                    bail!("--duration must be greater than 0, got {parsed}");
                }
                duration = Some(parsed);
            }
            other => audio_file = Some(other.to_string()),
        }
        i += 1;
    }

    if live {
        run_live(hop, duration)
    } else {
        let audio_file = audio_file.context("usage: chord_engine <audio_file> [--hop 0.25]")?;
        run_file(&audio_file, hop)
    }
}
