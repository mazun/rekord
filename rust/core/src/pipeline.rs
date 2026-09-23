//! Shared live-session driver: turns the extractor/stitcher/chord-matching
//! pipeline into a stream of `PipelineEvent`s so any frontend (the CLI's
//! `--live` mode, a future GUI) can drive the exact same logic without
//! duplicating it -- only what happens with each event differs.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::chord_templates::{extra_tensions, match_chord, ChordMatch};
use crate::key_estimator::{roman_numeral, KeyEstimate, KeyEstimator};
use crate::live_capture;
use crate::melody_bass::LineTracker;
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

/// Whether the key shown to the user comes from `KeyEstimator`'s own
/// rolling correlation, or has been pinned by the user to work around a
/// misdetection (K-S-style correlation is only reliable over whole
/// pieces/large segments -- a short rolling window can legitimately lock
/// onto the wrong member of a relative major/minor pair before the true
/// tonic has even sounded; see key_estimator.rs and TODO.md). `Manual`
/// always carries `Mode::Major` -- `KeyEstimate::name()` and
/// `roman_numeral()` only ever consult `relative_major_pc()`, so a manual
/// pick just needs the target relative-major pitch class, not a real
/// major/minor judgment call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyMode {
    Auto,
    Manual(KeyEstimate),
}

impl Default for KeyMode {
    fn default() -> Self {
        KeyMode::Auto
    }
}

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
    ///
    /// `melody`/`bass` say whether the note belongs to the top/bottom voice
    /// line (see `melody_bass`); both false means an inner voice.
    NoteConfirmed {
        start_s: f32,
        end_s: f32,
        pitch_midi: i32,
        velocity: f32,
        melody: bool,
        bass: bool,
    },
    /// `roman_numeral` is the chord's degree name relative to the most
    /// recently estimated key (see `key_estimator`), or `None` if no key
    /// estimate is confident yet (e.g. near session start).
    ChordChange { t: f32, label: String, roman_numeral: Option<String> },
    /// Notes currently sounding that fall outside the chord named by the
    /// last `ChordChange` -- e.g. a melodic passing tone over a held
    /// chord. Unlike `ChordChange` (gated by `CHORD_MIN_HOLD_S`'s hold-time
    /// hysteresis so melody notes can't pull the label around), this
    /// reacts immediately to the raw active-note set: flagging a
    /// transient extension *as* transient, rather than either hiding it
    /// or letting it hijack the main label, is the whole point. Always
    /// the full current set (not a delta) in `extra_tensions`'s fixed
    /// canonical order (b9, 9, #9, 11, #11, b13, 13), so the frontend can
    /// diff/render directly without re-sorting.
    TensionsChanged { tensions: Vec<String> },
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
#[derive(Clone, Copy, PartialEq)]
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

/// One checkpoint's raw reading: every currently-active pitch class (used
/// for tension detection regardless of whether a chord could be named),
/// plus the chord signal itself (`None` for the ambiguous-gap case --see
/// `chord_at`'s doc comment).
struct ActiveNow {
    active_pitch_classes: Vec<i32>,
    chord: Option<ChordAt>,
}

fn evaluate_active(note_events: &[NoteEvent], t: f32, tolerance: f32) -> ActiveNow {
    let active: Vec<i32> = note_events
        .iter()
        .filter(|&&(s, e, _p, _a)| s <= t + 1e-6 && e >= t - tolerance)
        .map(|&(_s, _e, p, _a)| p)
        .collect();
    if active.is_empty() {
        let recently_sounding = note_events
            .iter()
            .any(|&(s, e, _p, _a)| s <= t + 1e-6 && e >= t - SILENCE_HOLD_S);
        return ActiveNow {
            active_pitch_classes: Vec::new(),
            chord: if recently_sounding { None } else { Some(ChordAt::Silence) },
        };
    }
    let bass_pc = active.iter().min().map(|p| p.rem_euclid(12));
    let pitch_classes: Vec<i32> = active.iter().map(|p| p.rem_euclid(12)).collect();
    let chord = match_chord(&pitch_classes, bass_pc).map(ChordAt::Match);
    ActiveNow { active_pitch_classes: pitch_classes, chord }
}

/// Chord at time t: Some(Silence) if nothing has sounded recently,
/// None if notes are present (or sounded within SILENCE_HOLD_S) but
/// nothing matched well enough / it's just a brief gap (ambiguous --
/// caller should just keep showing the prior label).
pub fn chord_at(note_events: &[NoteEvent], t: f32, tolerance: f32) -> Option<ChordAt> {
    evaluate_active(note_events, t, tolerance).chord
}

// A candidate chord must be the best match continuously for this long
// before it actually replaces the displayed one -- otherwise a fast
// melodic passing tone (typically shorter than this: roughly a 16th note
// at a brisk tempo) would flip the label to whatever it briefly forms
// with the held chord, then flip back, for every such note. Deliberately
// far shorter than KeyEstimator's MIN_HOLD_S (2.0s): a real chord change
// legitimately happens every beat or faster, so this only needs to
// outlast a single passing note, not a phrase. No published source
// prescribes this for real-time chord tracking (the classic template-
// matching literature scores fixed, already-segmented chords, not a live
// stream) -- start here and tune empirically against sample_audio and
// live playing, same as WINDOW_S/SWITCH_MARGIN in key_estimator.rs.
const CHORD_MIN_HOLD_S: f32 = 0.15;

/// Stabilizes the displayed chord against brief melodic interference
/// (`CHORD_MIN_HOLD_S` hold-time hysteresis, mirroring the proven
/// margin/hold pattern in `KeyEstimator`) while separately surfacing
/// whatever's sounding outside that stabilized chord as `TensionsChanged`
/// events -- so a melody note doesn't just get silently absorbed or
/// briefly hijack the label, it shows up as what it is.
pub struct ChordStabilizer {
    displayed: Option<ChordAt>,
    // The best-scoring challenger to `displayed`, and when it first took
    // the lead -- reset whenever the leader changes, so a switch only
    // commits once the *same* candidate has led continuously for
    // CHORD_MIN_HOLD_S, not just cumulatively (same reasoning as
    // KeyEstimator::pending).
    pending: Option<(ChordAt, f32)>,
    tensions: Vec<&'static str>,
}

impl ChordStabilizer {
    pub fn new() -> Self {
        Self { displayed: None, pending: None, tensions: Vec::new() }
    }

    fn commit(
        &mut self,
        candidate: ChordAt,
        t: f32,
        current_key: Option<KeyEstimate>,
        on_event: &mut impl FnMut(PipelineEvent),
    ) {
        let roman = match (&candidate, current_key) {
            (ChordAt::Match(m), Some(key)) => Some(roman_numeral(&key, m)),
            _ => None,
        };
        on_event(PipelineEvent::ChordChange { t, label: candidate.label(), roman_numeral: roman });
        self.displayed = Some(candidate);
        self.pending = None;
    }

    /// Feed one checkpoint's active-pitch-class reading (see
    /// `evaluate_active`), applying hold-time hysteresis before switching
    /// the displayed chord and emitting `TensionsChanged` for whatever's
    /// active outside it. Call once per checkpoint, in timestamp order.
    fn observe(
        &mut self,
        evaluated: ActiveNow,
        t: f32,
        current_key: Option<KeyEstimate>,
        mut on_event: impl FnMut(PipelineEvent),
    ) {
        // A chord's notes don't release simultaneously -- confirmed via
        // real-world testing (2026-09-04): as a chord decays, an inner
        // voice (the 3rd) commonly fades out first, briefly leaving just
        // root+5th audible before the rest follows. That bare dyad is
        // definitionally the tail of the *same* chord, not a new one, so
        // (unlike an ordinary candidate) no amount of hold-time should
        // ever let it overwrite the display -- it can legitimately
        // outlast CHORD_MIN_HOLD_S by persisting for the rest of the
        // chord's decay. Only guards a same-root downgrade to "5"; a
        // different root (or an actual power chord struck from silence,
        // where `self.displayed` is still `None`) is unaffected.
        let is_decay_to_bare_fifth = matches!(
            (self.displayed, evaluated.chord),
            (Some(ChordAt::Match(cur)), Some(ChordAt::Match(cand)))
                if cand.quality == "5" && cand.root_pc == cur.root_pc && cur.quality != "5"
        );
        if let Some(candidate) = evaluated.chord {
            if !is_decay_to_bare_fifth {
                match self.displayed {
                    None => self.commit(candidate, t, current_key, &mut on_event),
                    Some(cur) if cur == candidate => self.pending = None,
                    Some(_) => match self.pending {
                        Some((cand, since)) if cand == candidate => {
                            if t - since >= CHORD_MIN_HOLD_S {
                                self.commit(candidate, t, current_key, &mut on_event);
                            }
                        }
                        _ => self.pending = Some((candidate, t)),
                    },
                }
            }
        }

        // Tensions are judged against whatever's actually displayed right
        // now -- including mid-hold, before `pending` has won. That's
        // deliberate: it's exactly what keeps a melody note that's
        // "losing" the hold-time race visible as a tension instead of
        // vanishing until (if ever) it wins outright.
        let new_tensions = match self.displayed {
            Some(ChordAt::Match(chord)) => extra_tensions(&evaluated.active_pitch_classes, &chord),
            _ => Vec::new(),
        };
        if new_tensions != self.tensions {
            on_event(PipelineEvent::TensionsChanged {
                tensions: new_tensions.iter().map(|s| s.to_string()).collect(),
            });
            self.tensions = new_tensions;
        }
    }
}

// A held chord needs a *later* checkpoint to exist at all before
// CHORD_MIN_HOLD_S's hold-time can ever be judged satisfied -- onset-only
// checkpoints sufficed before that hold-time existed (a genuine change
// was always self-evidently onset-triggered), but a clean block chord
// (every note onsetting together, then nothing else until the next
// chord's onset) generates only one tightly-clustered checkpoint and
// then silence until whatever triggers the next one. Windows complete in
// jumps up to ~1.6s (file-mode reprocessing's default extractor; live
// mode's configurable stride can also run coarser than this), so without
// this grid such a chord could go unconfirmed for its entire duration --
// confirmed via file-mode testing against sample_audio, where exactly
// this (a solid block chord with no follow-up onset before the next
// chord) silently never committed. The grid step just needs to be
// smaller than CHORD_MIN_HOLD_S; this has ample margin.
const CHORD_CHECKPOINT_GRID_S: f32 = 0.05;

/// Re-evaluate the chord at every note onset within the newly covered
/// span, plus a periodic grid (see `CHORD_CHECKPOINT_GRID_S`) so a held
/// chord always gets enough checkpoints to clear `CHORD_MIN_HOLD_S`.
pub fn report_new_chords(
    note_events: &[NoteEvent],
    prev_covered: f32,
    covered: f32,
    chord_display: &mut ChordStabilizer,
    key_estimator: &mut KeyEstimator,
    key_mode: &KeyMode,
    last_key_name: &mut Option<String>,
    mut on_event: impl FnMut(PipelineEvent),
) {
    let mut checkpoints: BTreeSet<i64> = note_events
        .iter()
        .map(|&(s, _e, _p, _a)| s)
        .filter(|&s| s > prev_covered && s <= covered)
        .map(|s| (s * 1000.0).round() as i64)
        .collect();
    let mut grid_t = (prev_covered / CHORD_CHECKPOINT_GRID_S).floor() * CHORD_CHECKPOINT_GRID_S;
    loop {
        grid_t += CHORD_CHECKPOINT_GRID_S;
        if grid_t > covered {
            break;
        }
        if grid_t > prev_covered {
            checkpoints.insert((grid_t * 1000.0).round() as i64);
        }
    }
    checkpoints.insert((covered * 1000.0).round() as i64);

    for t_key in checkpoints {
        let t = t_key as f32 / 1000.0;
        // Re-evaluate the key first: it can update mid-chord, and doing
        // this before the chord check lets a just-updated key inform this
        // same checkpoint's Roman-numeral label instead of lagging a step.
        // The estimator is still fed/advanced in Manual mode (its history
        // and hysteresis stay warm for whenever the user switches back to
        // Auto) -- only its output is ignored while a manual pick is in
        // effect.
        let auto_key = key_estimator.estimate(t);
        let current_key = match key_mode {
            KeyMode::Auto => auto_key,
            KeyMode::Manual(key) => Some(*key),
        };
        if let Some(key) = current_key {
            let key_name = key.name();
            if Some(&key_name) != last_key_name.as_ref() {
                on_event(PipelineEvent::KeyChange { key_name: key_name.clone() });
                *last_key_name = Some(key_name);
            }
        }
        let evaluated = evaluate_active(note_events, t, ACTIVE_EDGE_TOLERANCE_S);
        chord_display.observe(evaluated, t, current_key, &mut on_event);
    }
}

/// Emit one `NoteConfirmed` per note, feeding each through `key_estimator`
/// first -- shared by both call sites that produce a `Vec<ConfirmedNote>`
/// (the normal per-hop path and the end-of-session flush) so a note is
/// only ever observed once, from a single place.
fn emit_notes(
    notes: Vec<ConfirmedNote>,
    note_events: &[NoteEvent],
    key_estimator: &mut KeyEstimator,
    lines: &mut LineTracker,
    mut on_event: impl FnMut(PipelineEvent),
) {
    for n in notes {
        key_estimator.observe(&n);
        let roles = lines.classify(&n, note_events);
        on_event(PipelineEvent::NoteConfirmed {
            start_s: n.start_s,
            end_s: n.end_s,
            pitch_midi: n.pitch_midi,
            velocity: n.velocity,
            melody: roles.melody,
            bass: roles.bass,
        });
    }
}

pub fn emit_confirmed_notes(
    stitcher: &mut NoteStitcher,
    covered: f32,
    note_events: &[NoteEvent],
    key_estimator: &mut KeyEstimator,
    lines: &mut LineTracker,
    on_event: impl FnMut(PipelineEvent),
) {
    emit_notes(stitcher.update(covered, note_events), note_events, key_estimator, lines, on_event);
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
    key_mode: Arc<Mutex<KeyMode>>,
    mut on_event: impl FnMut(PipelineEvent),
) -> Result<()> {
    let mut model = OnnxNoteModel::load()?;
    let mut stitcher = NoteStitcher::new();
    let mut key_estimator = KeyEstimator::new();
    let mut lines = LineTracker::new();
    let mut chord_display = ChordStabilizer::new();
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
            emit_confirmed_notes(
                &mut stitcher,
                covered,
                &note_events,
                &mut key_estimator,
                &mut lines,
                &mut on_event,
            );
            let current_key_mode = *key_mode.lock().unwrap();
            report_new_chords(
                &note_events,
                last_covered,
                covered,
                &mut chord_display,
                &mut key_estimator,
                &current_key_mode,
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
    emit_notes(stitcher.flush(&final_notes), &final_notes, &mut key_estimator, &mut lines, &mut on_event);
    let current_key_mode = *key_mode.lock().unwrap();
    report_new_chords(
        &final_notes,
        last_covered,
        extractor.covered_seconds(),
        &mut chord_display,
        &mut key_estimator,
        &current_key_mode,
        &mut last_key_name,
        &mut on_event,
    );

    Ok(())
}
