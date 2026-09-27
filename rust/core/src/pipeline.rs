//! Shared live-session driver: turns the extractor/stitcher/chord-matching
//! pipeline into a stream of `PipelineEvent`s so any frontend (the CLI's
//! `--live` mode, a future GUI) can drive the exact same logic without
//! duplicating it -- only what happens with each event differs.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::beat_model::{self, BeatModel};
use crate::beat_tracker::BeatTracker;
use crate::chord_templates::{extra_tensions, match_chord, ChordMatch};
use crate::harmonica_decode::{ConfirmedNote, NoteEvent};
use crate::harmonica_model::{self, Device, HarmonicaModel, ModelSize};
use crate::harmonica_stream::NoteTranscriber;
use crate::key_estimator::{roman_numeral, KeyEstimate, KeyEstimator};
use crate::live_capture;
use crate::melody_bass::LineTracker;

// Extra fudge for IPC/processing overhead and jitter on top of the
// pipeline's own structural confirm delay, so the reported total_latency_s
// has a small safety margin baked in (matches the figure the piano-roll UI
// used to hardcode for this purpose).
const LATENCY_SAFETY_MARGIN_S: f32 = 0.3;

/// Which note-transcription model to run and how. `future_frames` trades
/// confirm latency for accuracy (see `FUTURE_FRAME_CHOICES`); `allow_directml`
/// lets a caller (e.g. a CLI `--cpu` flag) force the small-on-CPU fallback
/// path deliberately, for testing it without having to disable DirectML at
/// the OS/driver level.
pub struct NoteModelConfig {
    pub size: ModelSize,
    pub future_frames: usize,
    pub allow_directml: bool,
}

/// Which model/device a session actually ended up running, after any
/// DirectML->CPU fallback (`HarmonicaModel::load` always falls back to
/// `Small` on CPU, never the originally requested size) -- reported in the
/// `Started` event so a frontend can show what's really running rather than
/// just echoing back the user's request.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct NoteModelInfo {
    pub size: ModelSize,
    pub device: Device,
    pub future_frames: usize,
}

/// Fixed capture chunk length. A block is processed as soon as the chunk
/// holding its last sample arrives, so this adds up to one chunk of delay
/// (it's part of `total_latency_s`); 64 ms is a quarter of a 0.256 s block.
pub const CAPTURE_HOP_S: f32 = 0.064;

/// The look-ahead choices offered to the user: 0.26 s and 0.51 s of future
/// audio. In harmonica-repro's streaming evaluation, going from 8 to 16
/// frames gains under a point of OnPOff (large .754 -> .760) and longer
/// gains nothing; the thresholds, picked at 8, are used for both.
pub const FUTURE_FRAME_CHOICES: [usize; 2] = [8, 16];

/// Default look-ahead: 0.26 s.
pub const DEFAULT_FUTURE_FRAMES: usize = 8;

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

/// A beat line on the same audio timeline as the notes; `bar` marks a downbeat.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct BeatLine {
    pub t: f32,
    pub bar: bool,
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
    /// delay for this session: the transcriber's own `max_note_delay_s`
    /// (which depends on the model's look-ahead) plus the capture hop and a
    /// small safety margin -- see `run_note_pipeline`. A frontend should
    /// anchor its display clock at this event using this figure directly,
    /// not hardcode or re-derive it. `note_model` is the model actually
    /// loaded, after any DirectML->CPU fallback (see `HarmonicaModel::load`).
    Started { total_latency_s: f32, note_model: NoteModelInfo },
    /// `velocity` is the onset probability at the note's onset frame (see
    /// `harmonica_decode::ConfirmedNote::velocity`) -- a frontend can use it
    /// to shade/intensify the note by how strongly it sounded, correctly
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
    /// Every beat line at or after `from_s`, replacing the previous `BeatLines` over that
    /// range (lines before `from_s` are unchanged and already final). Three layers of the
    /// same beats at increasing delay and confidence (see beat_tracker.rs):
    /// `predicted` extrapolates the provisional grid up to the present (no delay),
    /// `provisional` is the network's own peaks (~0.6s behind the audio), and `committed`
    /// is the DBN's decision (~4.5s behind), which never changes once sent. A beat moves
    /// from one layer to the next as the audio catches up; its time can shift slightly
    /// when it does, which the frontend animates rather than redraws.
    BeatLines {
        from_s: f32,
        committed: Vec<BeatLine>,
        provisional: Vec<BeatLine>,
        predicted: Vec<BeatLine>,
    },
}

// How far back each BeatLines snapshot reaches. Only needs to cover what's still on screen
// (the piano roll shows width / PIXELS_PER_SECOND = 16s on a 1920px-wide window, and a
// line older than that is committed and final anyway); bounds the event size.
const BEAT_LINES_WINDOW_S: f64 = 30.0;

fn beat_lines(beats: &[f64], downbeats: &[f64]) -> Vec<BeatLine> {
    // downbeats are always one of the beats' exact values (committed: the DBN's; provisional:
    // snapped to the nearest beat), except a provisional downbeat with no beat at all yet
    let mut out: Vec<BeatLine> =
        beats.iter().map(|&t| BeatLine { t: t as f32, bar: downbeats.contains(&t) }).collect();
    for &d in downbeats {
        if !beats.contains(&d) {
            out.push(BeatLine { t: d as f32, bar: true });
        }
    }
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// The beat tracker's side of a live session: runs on its own thread (Beat This! costs ~40%
/// of a core in real time, which must not delay the note pipeline) on a copy of every
/// capture chunk, and emits a `BeatLines` snapshot whenever the lines change. Beat tracking
/// failing (e.g. a missing model) only disables the lines, not the session. Public for
/// `examples/beat_replay.rs`, which drives it from a file at real-time pace.
pub fn run_beat_tracker<F: FnMut(PipelineEvent)>(rx: Receiver<Vec<f32>>, stop: &AtomicBool, on_event: &Mutex<F>) {
    let model = match BeatModel::load(1) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("beat tracking disabled: {e:#}");
            return;
        }
    };
    let mut tracker = BeatTracker::bounded(model, true);
    let mut last: Option<PipelineEvent> = None;
    let mut emit = |tracker: &BeatTracker| {
        let now = tracker.now();
        let from = now - BEAT_LINES_WINDOW_S;
        let l = tracker.lines_since(now, from);
        let event = PipelineEvent::BeatLines {
            from_s: from as f32,
            committed: beat_lines(&l.committed_beats, &l.committed_downbeats),
            provisional: beat_lines(&l.provisional_beats, &l.provisional_downbeats),
            predicted: l.predicted.iter().map(|p| BeatLine { t: p.time as f32, bar: p.is_bar }).collect(),
        };
        // from_s alone moves on every chunk; only send when a line changed
        let changed = match (&last, &event) {
            (
                Some(PipelineEvent::BeatLines { committed: c0, provisional: p0, predicted: q0, .. }),
                PipelineEvent::BeatLines { committed: c1, provisional: p1, predicted: q1, .. },
            ) => c0 != c1 || p0 != p1 || q0 != q1,
            _ => true,
        };
        if changed {
            (on_event.lock().unwrap())(event.clone());
            last = Some(event);
        }
    };
    while let Ok(mut chunk) = rx.recv() {
        // if it ever falls behind, catch up in one push rather than one event per chunk
        while let Ok(more) = rx.try_recv() {
            chunk.extend(more);
        }
        if stop.load(Ordering::Relaxed) {
            return; // stopped by the user: nobody is watching the final lines
        }
        if let Err(e) = tracker.push(&chunk) {
            eprintln!("beat tracking stopped: {e:#}");
            return;
        }
        emit(&tracker);
    }
    // the capture ended on its own (e.g. --duration): commit through the end, as the note pipeline's finish()
    if tracker.flush().is_ok() {
        emit(&tracker);
    }
}

// A brief gap with no active note (both hands lifting between chords, a
// note ending slightly early) shouldn't flash "(silence)" -- only declare
// real silence once nothing has sounded for this long. Chosen independently
// of Harmonica's own note-emission delay (see
// `NoteTranscriber::max_note_delay_s`), not derived from it -- just an
// empirically reasonable gap length.
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
/// plus the chord signal itself: Some(Silence) if nothing has sounded
/// recently, None if notes are present (or sounded within SILENCE_HOLD_S)
/// but nothing matched well enough / it's just a brief gap (ambiguous --
/// the caller should just keep showing the prior label).
struct ActiveNow {
    active_pitch_classes: Vec<i32>,
    chord: Option<ChordAt>,
}

/// Notes sounding at time t. Note ends are taken as exact: Harmonica's
/// decode closes a note on the frame its sustain drops, so there's no
/// grace period after it (basic-pitch needed 0.2 s for its ragged ends,
/// which with Harmonica only let the previous chord linger into the next
/// as a mixed chord, e.g. Am7 for C -> Am).
fn evaluate_active(note_events: &[NoteEvent], t: f32) -> ActiveNow {
    let active: Vec<i32> = note_events
        .iter()
        .filter(|&&(s, e, _p, _a)| s <= t + 1e-6 && e >= t)
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
// then silence until whatever triggers the next one. Harmonica's streaming
// blocks complete every 0.256s (BLOCK_FRAMES in harmonica_model.rs) -- far
// finer than the basic-pitch extractor's old windows (which completed in
// jumps up to ~1.6s, where this was originally found and fixed) -- but a
// solid block chord with no follow-up onset before the next chord could
// still go unconfirmed for the rest of its duration without this grid. The
// grid step just needs to be smaller than CHORD_MIN_HOLD_S; this has ample
// margin either way.
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
        let evaluated = evaluate_active(note_events, t);
        chord_display.observe(evaluated, t, current_key, &mut on_event);
    }
}

/// Emit one `NoteConfirmed` per finished note (whatever a `NoteTranscriber`
/// `push`/`finish` returned -- each note comes out exactly once), feeding
/// each through `key_estimator` first. Shared by the per-block path and the
/// end-of-session flush so a note is only ever observed from one place.
pub fn emit_confirmed_notes(
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

/// Runs a live WASAPI-loopback session, invoking `on_event` for every
/// confirmed note, chord change and beat-line update (the latter from a
/// second thread, hence `Send`), until `duration_s` elapses (if given),
/// `stop` is set to true, or the capture stream ends. `config` selects the
/// note-transcription model size/look-ahead and whether DirectML may be
/// used; the model actually loaded (after any fallback) is reported in the
/// `Started` event. Shared by the CLI's `--live` mode and any other
/// frontend driving the same pipeline (e.g. a GUI, which can flip `stop`
/// from a "Stop" button to end the session early -- checked once per hop,
/// so it stops within about one hop's worth of latency).
pub fn run_live_session(
    config: NoteModelConfig,
    hop_seconds: f32,
    duration_s: Option<f32>,
    stop: Arc<AtomicBool>,
    key_mode: Arc<Mutex<KeyMode>>,
    on_event: impl FnMut(PipelineEvent) + Send,
) -> Result<()> {
    let on_event = Mutex::new(on_event);
    let (beat_tx, beat_rx) = channel::<Vec<f32>>();
    std::thread::scope(|scope| {
        let beat_stop = &*stop;
        let on_event_ref = &on_event;
        scope.spawn(move || run_beat_tracker(beat_rx, beat_stop, on_event_ref));
        let mut emit = |event| (on_event.lock().unwrap())(event);
        let result = run_note_pipeline(config, hop_seconds, duration_s, &stop, &key_mode, |chunk| {
            let _ = beat_tx.send(chunk.to_vec()); // Err only if beat tracking is disabled
        }, &mut emit);
        drop(beat_tx); // lets the beat thread flush and exit; the scope joins it
        result
    })
}

fn run_note_pipeline(
    config: NoteModelConfig,
    hop_seconds: f32,
    duration_s: Option<f32>,
    stop: &AtomicBool,
    key_mode: &Mutex<KeyMode>,
    mut on_chunk: impl FnMut(&[f32]),
    mut on_event: impl FnMut(PipelineEvent),
) -> Result<()> {
    let model = HarmonicaModel::load(config.size, config.allow_directml)?;
    let note_model = NoteModelInfo { size: model.size(), device: model.device(), future_frames: config.future_frames };
    let mut transcriber = NoteTranscriber::new(model, config.future_frames);
    let mut key_estimator = KeyEstimator::new();
    let mut lines = LineTracker::new();
    let mut chord_display = ChordStabilizer::new();
    let mut last_key_name: Option<String> = None;
    let mut last_covered = 0.0f32;
    let mut elapsed = 0.0f32;

    let (_stream, rx) =
        live_capture::spawn_loopback_capture(hop_seconds, [harmonica_model::SAMPLE_RATE, beat_model::SAMPLE_RATE])?;
    let total_latency_s = transcriber.max_note_delay_s() + hop_seconds + LATENCY_SAFETY_MARGIN_S;
    on_event(PipelineEvent::Started { total_latency_s, note_model });
    while let Ok([note_chunk, beat_chunk]) = rx.recv() {
        on_chunk(&beat_chunk);
        let blocks_before = transcriber.blocks_done();
        let notes = transcriber.push(&note_chunk)?;
        elapsed += hop_seconds;
        if transcriber.blocks_done() != blocks_before {
            let note_events = transcriber.note_events();
            let covered = transcriber.covered_seconds();
            emit_confirmed_notes(notes, &note_events, &mut key_estimator, &mut lines, &mut on_event);
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

    // There's no more audio coming once the loop above exits (stop,
    // duration_s, or the capture stream itself ending) -- `finish()`
    // zero-pads and closes every still-open note (see
    // `NoteTranscriber::finish`) so nothing still sounding at session end
    // is silently dropped.
    let final_notes = transcriber.finish()?;
    let final_note_events = transcriber.note_events();
    emit_confirmed_notes(final_notes, &final_note_events, &mut key_estimator, &mut lines, &mut on_event);
    let current_key_mode = *key_mode.lock().unwrap();
    report_new_chords(
        &final_note_events,
        last_covered,
        transcriber.covered_seconds(),
        &mut chord_display,
        &mut key_estimator,
        &current_key_mode,
        &mut last_key_name,
        &mut on_event,
    );

    Ok(())
}
