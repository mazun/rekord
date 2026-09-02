//! Shared live-session driver: turns the extractor/stitcher/chord-matching
//! pipeline into a stream of `PipelineEvent`s so any frontend (the CLI's
//! `--live` mode, a future GUI) can drive the exact same logic without
//! duplicating it -- only what happens with each event differs.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;

use crate::chord_templates::{match_chord, ChordMatch};
use crate::key_estimator::{roman_numeral, KeyEstimator};
use crate::live_capture;
use crate::note_creation::NoteEvent;
use crate::note_stitcher::{ConfirmedNote, NoteStitcher, CONFIRM_MARGIN_S};
use crate::onnx_model::OnnxNoteModel;
use crate::streaming_extractor::{StreamingNoteExtractor, AUDIO_SAMPLE_RATE};

// A note's end_s can be a touch before the true edge of what's been
// processed (edge effect from limited context there).
const ACTIVE_EDGE_TOLERANCE_S: f32 = 0.2;

// Extra fudge for IPC/processing overhead and jitter on top of the
// pipeline's own structural confirm delay, so the reported total_latency_s
// has a small safety margin baked in (matches the figure the piano-roll UI
// used to hardcode for this purpose).
const LATENCY_SAFETY_MARGIN_S: f32 = 0.3;

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
    ///
    /// `total_latency_s` is the authoritative, already-computed confirm
    /// delay for this session (depends on the stride/hop the caller
    /// requested, clamped to whatever was actually used) -- a frontend
    /// should anchor its display clock at this event using this figure
    /// directly, not hardcode or re-derive it.
    Started { total_latency_s: f32 },
    /// `velocity` is the model's own per-pitch activation strength for this
    /// note (see `ConfirmedNote::velocity`) -- a frontend can use it to
    /// shade/intensify the note by how strongly it sounded, correctly
    /// distinguishing notes within the same chord.
    NoteConfirmed { start_s: f32, end_s: f32, pitch_midi: i32, velocity: f32 },
    /// `roman_numeral` is the chord's degree name relative to the most
    /// recently estimated key (see `key_estimator`), or `None` if no key
    /// estimate is confident yet (e.g. near session start).
    ChordChange { t: f32, label: String, roman_numeral: Option<String> },
    /// Fired independently of `ChordChange`: a key estimate can update
    /// mid-chord, and the display needs to track a modulation even across
    /// a single held chord's duration, not just at the next chord
    /// boundary.
    KeyChange { key_name: String },
}

// A brief gap with no active note (both hands lifting between chords, a
// note ending slightly early) shouldn't flash "(silence)" -- only declare
// real silence once nothing has sounded for this long. Matches the order
// of magnitude of NoteStitcher's own CONFIRM_MARGIN_S.
const SILENCE_HOLD_S: f32 = 0.5;

/// What's sounding at time t, if anything conclusive can be said. Kept as
/// the matched `ChordMatch` (not just its formatted label) so a caller can
/// also derive a Roman-numeral degree from `root_pc`/`quality` without
/// reparsing a display string.
pub enum ChordAt {
    Silence,
    Match(ChordMatch),
}

impl ChordAt {
    pub fn label(&self) -> String {
        match self {
            ChordAt::Silence => "(silence)".to_string(),
            ChordAt::Match(m) => m.label(),
        }
    }
}

/// Chord at time t: Some(Silence) if nothing has sounded recently,
/// None if notes are present (or sounded within SILENCE_HOLD_S) but
/// nothing matched well enough / it's just a brief gap (ambiguous --
/// caller should just keep showing the prior label).
pub fn chord_at(note_events: &[NoteEvent], t: f32, tolerance: f32) -> Option<ChordAt> {
    let active: Vec<i32> = note_events
        .iter()
        .filter(|&&(s, e, _p, _a)| s <= t + 1e-6 && e >= t - tolerance)
        .map(|&(_s, _e, p, _a)| p)
        .collect();
    if active.is_empty() {
        let recently_sounding = note_events
            .iter()
            .any(|&(s, e, _p, _a)| s <= t + 1e-6 && e >= t - SILENCE_HOLD_S);
        if recently_sounding {
            return None;
        }
        return Some(ChordAt::Silence);
    }
    let bass_pc = active.iter().min().map(|p| p.rem_euclid(12));
    let pitch_classes: Vec<i32> = active.iter().map(|p| p.rem_euclid(12)).collect();
    match_chord(&pitch_classes, bass_pc).map(ChordAt::Match)
}

/// Windows complete in ~1.6s jumps, so a single "what's active now" check
/// can skip right over a chord shorter than that. Re-evaluate the chord at
/// every note onset within the newly covered span instead.
pub fn report_new_chords(
    note_events: &[NoteEvent],
    prev_covered: f32,
    covered: f32,
    last_label: &mut Option<String>,
    key_estimator: &mut KeyEstimator,
    last_key_name: &mut Option<String>,
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
        // Re-evaluate the key first: it can update mid-chord, and doing
        // this before the chord check lets a just-updated key inform this
        // same checkpoint's Roman-numeral label instead of lagging a step.
        let current_key = key_estimator.estimate(t);
        if let Some(key) = current_key {
            let key_name = key.name();
            if Some(&key_name) != last_key_name.as_ref() {
                on_event(PipelineEvent::KeyChange { key_name: key_name.clone() });
                *last_key_name = Some(key_name);
            }
        }
        if let Some(chord) = chord_at(note_events, t, ACTIVE_EDGE_TOLERANCE_S) {
            let label = chord.label();
            if Some(&label) != last_label.as_ref() {
                let roman = match (&chord, current_key) {
                    (ChordAt::Match(m), Some(key)) => Some(roman_numeral(&key, m)),
                    _ => None,
                };
                on_event(PipelineEvent::ChordChange { t, label: label.clone(), roman_numeral: roman });
                *last_label = Some(label);
            }
        }
    }
}

/// Emit one `NoteConfirmed` per note, feeding each through `key_estimator`
/// first -- shared by both call sites that produce a `Vec<ConfirmedNote>`
/// (the normal per-hop path and the end-of-session flush) so a note is
/// only ever observed once, from a single place.
fn emit_notes(
    notes: Vec<ConfirmedNote>,
    key_estimator: &mut KeyEstimator,
    mut on_event: impl FnMut(PipelineEvent),
) {
    for n in notes {
        key_estimator.observe(&n);
        on_event(PipelineEvent::NoteConfirmed {
            start_s: n.start_s,
            end_s: n.end_s,
            pitch_midi: n.pitch_midi,
            velocity: n.velocity,
        });
    }
}

pub fn emit_confirmed_notes(
    stitcher: &mut NoteStitcher,
    covered: f32,
    note_events: &[NoteEvent],
    key_estimator: &mut KeyEstimator,
    on_event: impl FnMut(PipelineEvent),
) {
    emit_notes(stitcher.update(covered, note_events), key_estimator, on_event);
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
    stride_seconds: f32,
    duration_s: Option<f32>,
    stop: Arc<AtomicBool>,
    mut on_event: impl FnMut(PipelineEvent),
) -> Result<()> {
    let mut model = OnnxNoteModel::load()?;
    let mut stitcher = NoteStitcher::new();
    let mut key_estimator = KeyEstimator::new();
    let mut last_chord_label: Option<String> = None;
    let mut last_key_name: Option<String> = None;
    let mut last_covered = 0.0f32;
    let mut elapsed = 0.0f32;

    let (_stream, rx) = live_capture::spawn_loopback_capture(hop_seconds)?;
    let stride_samples = (stride_seconds * AUDIO_SAMPLE_RATE as f32).round() as usize;
    let mut extractor = StreamingNoteExtractor::with_stride(&mut model, stride_samples);
    // Read back the actually-clamped stride so the reported latency is
    // authoritative even if the caller asked for something out of range.
    let effective_stride_s = extractor.stride_samples() as f32 / AUDIO_SAMPLE_RATE as f32;
    let total_latency_s = effective_stride_s + hop_seconds + CONFIRM_MARGIN_S + LATENCY_SAFETY_MARGIN_S;
    on_event(PipelineEvent::Started { total_latency_s });
    while let Ok(chunk) = rx.recv() {
        let completed = extractor.push(&chunk)?;
        elapsed += hop_seconds;
        if completed > 0 {
            let note_events = extractor.note_events();
            let covered = extractor.covered_seconds();
            emit_confirmed_notes(&mut stitcher, covered, &note_events, &mut key_estimator, &mut on_event);
            report_new_chords(
                &note_events,
                last_covered,
                covered,
                &mut last_chord_label,
                &mut key_estimator,
                &mut last_key_name,
                &mut on_event,
            );
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
    emit_notes(stitcher.flush(&final_notes), &mut key_estimator, &mut on_event);
    report_new_chords(
        &final_notes,
        last_covered,
        extractor.covered_seconds(),
        &mut last_chord_label,
        &mut key_estimator,
        &mut last_key_name,
        &mut on_event,
    );

    Ok(())
}
