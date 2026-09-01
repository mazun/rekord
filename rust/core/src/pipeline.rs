//! Shared live-session driver: turns the extractor/stitcher/chord-matching
//! pipeline into a stream of `PipelineEvent`s so any frontend (the CLI's
//! `--live` mode, a future GUI) can drive the exact same logic without
//! duplicating it -- only what happens with each event differs.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;

use crate::chord_templates::match_chord;
use crate::live_capture;
use crate::note_creation::NoteEvent;
use crate::note_stitcher::NoteStitcher;
use crate::onnx_model::OnnxNoteModel;
use crate::streaming_extractor::StreamingNoteExtractor;

// A note's end_s can be a touch before the true edge of what's been
// processed (edge effect from limited context there).
const ACTIVE_EDGE_TOLERANCE_S: f32 = 0.2;

pub const MIDI_NOTE_NAMES: [&str; 12] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

pub fn midi_to_name(pitch: i32) -> String {
    format!("{}{}", MIDI_NOTE_NAMES[pitch.rem_euclid(12) as usize], pitch.div_euclid(12) - 1)
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "type")]
pub enum PipelineEvent {
    /// Fired once, right before the capture loop begins -- i.e. once model
    /// loading and audio device setup are actually done, not merely once
    /// the start command has been invoked. A frontend timing its own
    /// clock against this pipeline's timestamps should treat this event's
    /// arrival as t=0, not whenever the user clicked "start": model
    /// load + WASAPI initialization can add anywhere from tens of ms to a
    /// couple of seconds before the pipeline truly starts, and that delay
    /// isn't part of the pipeline's own fixed per-note confirm latency.
    Started,
    NoteConfirmed { start_s: f32, end_s: f32, pitch_midi: i32 },
    ChordChange { t: f32, label: String },
}

/// Chord label for whatever's sounding at time t: Some("(silence)") if
/// nothing is, None if notes are present but nothing matched well enough
/// (ambiguous -- caller should just keep showing the prior label).
pub fn chord_label_at(note_events: &[NoteEvent], t: f32, tolerance: f32) -> Option<String> {
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
pub fn report_new_chords(
    note_events: &[NoteEvent],
    prev_covered: f32,
    covered: f32,
    last_label: &mut Option<String>,
    mut on_event: impl FnMut(PipelineEvent),
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
                on_event(PipelineEvent::ChordChange { t, label: label.clone() });
                *last_label = Some(label);
            }
        }
    }
}

pub fn emit_confirmed_notes(
    stitcher: &mut NoteStitcher,
    covered: f32,
    note_events: &[NoteEvent],
    mut on_event: impl FnMut(PipelineEvent),
) {
    for n in stitcher.update(covered, note_events) {
        on_event(PipelineEvent::NoteConfirmed {
            start_s: n.start_s,
            end_s: n.end_s,
            pitch_midi: n.pitch_midi,
        });
    }
}

/// Runs a live WASAPI-loopback session, invoking `on_event` for every
/// confirmed note and chord change, until `duration_s` elapses (if given),
/// `stop` is set to true, or the capture stream ends. Shared by the CLI's
/// `--live` mode and any other frontend driving the same pipeline (e.g. a
/// GUI, which can flip `stop` from a "Stop" button to end the session
/// early -- checked once per hop, so it stops within about one hop's
/// worth of latency).
pub fn run_live_session(
    hop_seconds: f32,
    duration_s: Option<f32>,
    stop: Arc<AtomicBool>,
    mut on_event: impl FnMut(PipelineEvent),
) -> Result<()> {
    let mut model = OnnxNoteModel::load()?;
    let mut stitcher = NoteStitcher::new();
    let mut last_chord_label: Option<String> = None;
    let mut last_covered = 0.0f32;
    let mut elapsed = 0.0f32;

    let (_stream, rx) = live_capture::spawn_loopback_capture(hop_seconds)?;
    let mut extractor = StreamingNoteExtractor::new(&mut model);
    on_event(PipelineEvent::Started);
    while let Ok(chunk) = rx.recv() {
        let completed = extractor.push(&chunk)?;
        elapsed += hop_seconds;
        if completed > 0 {
            let note_events = extractor.note_events();
            let covered = extractor.covered_seconds();
            emit_confirmed_notes(&mut stitcher, covered, &note_events, &mut on_event);
            report_new_chords(&note_events, last_covered, covered, &mut last_chord_label, &mut on_event);
            last_covered = covered;
        }
        if let Some(d) = duration_s {
            if elapsed >= d {
                break;
            }
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
    }

    // NoteStitcher deliberately holds back anything within the last
    // CONFIRM_MARGIN_S of "covered" time, in case more audio revises it.
    // There's no more audio coming once the loop above exits -- flush what's
    // left so it isn't silently lost.
    let final_notes = extractor.note_events();
    for n in stitcher.flush(&final_notes) {
        on_event(PipelineEvent::NoteConfirmed {
            start_s: n.start_s,
            end_s: n.end_s,
            pitch_midi: n.pitch_midi,
        });
    }
    report_new_chords(
        &final_notes,
        last_covered,
        extractor.covered_seconds(),
        &mut last_chord_label,
        &mut on_event,
    );

    Ok(())
}
