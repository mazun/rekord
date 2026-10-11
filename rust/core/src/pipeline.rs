//! Shared live-session driver: turns the note transcription / chord-layer
//! pipeline into a stream of `PipelineEvent`s so any frontend (the CLI's
//! `--live` mode, a future GUI) can drive the exact same logic without
//! duplicating it -- only what happens with each event differs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::Duration;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::beat_model::{self, BeatModel};
use crate::beat_tracker::{BeatGrid, BeatTracker};
use crate::chord_layers::{ActiveChange, ChordEngine, ChordLayerId, LayerChord};
use crate::chord_templates::{self, ChordVocabulary};
use crate::harmonica_decode::ConfirmedNote;
use crate::harmonica_model::{self, Device, HarmonicaModel, ModelChoice, ModelSize, Part};
use crate::harmonica_stream::NoteTranscriber;
use crate::key_estimator::{KeyEstimate, KeyEstimator};
use crate::spelling::{note_name, Accidental};
use crate::live_capture;

pub use crate::chord_layers::ChordAt;

// Extra fudge for IPC/processing overhead and jitter on top of the
// pipeline's own structural confirm delay, so the reported total_latency_s
// has a small safety margin baked in (matches the figure the piano-roll UI
// used to hardcode for this purpose).
const LATENCY_SAFETY_MARGIN_S: f32 = 0.3;

// How often a capture that is receiving no audio looks at the stop flag.
const STOP_POLL: Duration = Duration::from_millis(100);

/// Which note-transcription model to run and how. `future_frames` trades
/// confirm latency for accuracy (see `FUTURE_FRAME_CHOICES`); `allow_directml`
/// lets a caller (e.g. a CLI `--cpu` flag) force the CPU path deliberately,
/// for testing it without having to disable DirectML at the OS/driver level.
pub struct NoteModelConfig {
    pub choice: ModelChoice,
    pub future_frames: usize,
    pub allow_directml: bool,
}

/// Which model/device a session actually ended up running, after `Auto` and
/// any DirectML->CPU fallback (see `HarmonicaModel::load`) -- reported in the
/// `Started` event so a frontend can show what's really running rather than
/// just echoing back the user's request. `over_budget` is
/// `HarmonicaModel::may_exceed_budget`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct NoteModelInfo {
    pub size: ModelSize,
    pub device: Device,
    pub future_frames: usize,
    pub over_budget: bool,
}

impl NoteModelInfo {
    pub fn of(model: &HarmonicaModel, future_frames: usize) -> Self {
        Self { size: model.size(), device: model.device(), future_frames, over_budget: model.may_exceed_budget() }
    }
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
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum KeyMode {
    #[default]
    Auto,
    Manual(KeyEstimate),
}

pub fn midi_to_name(pitch: i32, mode: Accidental) -> String {
    format!("{}{}", note_name(pitch, mode), pitch.div_euclid(12) - 1)
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
    ///
    /// `chord_vocabulary` is the chord table the tensions are computed from on the frontend (see
    /// `ActiveNotes`), so that table lives in one place, `chord_templates.rs`.
    Started { total_latency_s: f32, note_model: NoteModelInfo, chord_vocabulary: ChordVocabulary },
    /// A note whose onset is decided but which is still sounding (as far as
    /// the decoder knows); `NoteConfirmed` follows with the same `start_s`,
    /// `pitch_midi` and `part` once it ends. Lets the piano roll draw the note
    /// from its onset, growing, instead of only once it is over.
    NoteStarted { start_s: f32, pitch_midi: i32, velocity: f32, part: Part },
    /// `velocity` is the onset probability at the note's onset frame (see
    /// `harmonica_decode::ConfirmedNote::velocity`) -- a frontend can use it
    /// to shade/intensify the note by how strongly it sounded, correctly
    /// distinguishing notes within the same chord.
    ///
    /// `part` is the model's own output part (vocal/bass/other).
    NoteConfirmed { start_s: f32, end_s: f32, pitch_midi: i32, velocity: f32, part: Part },
    /// What one chord layer newly decided (see `chord_layers` and docs/chord-layers.md): the chords
    /// it says begin at the given times, and `decided_s`, the time up to which it has decided
    /// everything (all `changes` are at or before it, and nothing before it is ever revised).
    /// Layers differ in how long they wait before deciding, so a later layer corrects an earlier
    /// one; the frontend shows each layer's decisions for the span only it has decided. A change
    /// with the same `t` as the layer's previous one replaces it (the chord respelled after the
    /// sharp/flat setting changed).
    ChordLayer { layer: ChordLayerId, changes: Vec<LayerChord>, decided_s: f32 },
    /// The pitch classes sounding (every part, the sung melody included) from each `t` on, sent
    /// when the set changes. The tensions over the shown chord are read from this: the notes
    /// outside the chord's own template, in the fixed canonical order (b9, 9, #9, 11, #11, b13,
    /// 13). Unlike the chord (decided only after a window and a hold), a melody note shows up as a
    /// tension at once -- flagging a transient extension *as* transient, rather than either hiding
    /// it or letting it hijack the label, is the point.
    ActiveNotes { changes: Vec<ActiveChange> },
    /// Fired independently of the chords: a key estimate can update
    /// mid-chord, and the display needs to track a modulation even across
    /// a single held chord's duration, not just at the next chord
    /// boundary. `t` is the checkpoint time of the estimate (audio time,
    /// like every other timestamp here).
    KeyChange { t: f32, key_name: String, tonic_pc: i32 },
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
/// failing (e.g. a missing model) only disables the lines, not the session. Also publishes
/// its current `BeatGrid` to `beat_grid` after every chunk (a plain write behind a brief
/// lock, never held during the actual model/DBN work) so the note-processing thread can snap
/// chord onsets to it without ever blocking on this thread's heavier computation. Public for
/// `examples/beat_replay.rs`, which drives it from a file at real-time pace.
pub fn run_beat_tracker<F: FnMut(PipelineEvent)>(
    rx: Receiver<Vec<f32>>,
    stop: &AtomicBool,
    on_event: &Mutex<F>,
    beat_grid: &Mutex<Option<BeatGrid>>,
) {
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
        *beat_grid.lock().unwrap() = tracker.beat_grid(tracker.now());
        emit(&tracker);
    }
    // the capture ended on its own (e.g. --duration): commit through the end, as the note pipeline's finish()
    if tracker.flush().is_ok() {
        emit(&tracker);
    }
}

/// Emit one `NoteConfirmed` per finished note (whatever a `NoteTranscriber`
/// `push`/`finish` returned -- each note comes out exactly once), feeding
/// each through `key_estimator` first. Shared by the per-block path and the
/// end-of-session flush so a note is only ever observed from one place.
pub fn emit_confirmed_notes(
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
            part: n.part,
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
/// so it stops within about one hop's worth of latency). `tap`, if given,
/// also sees the raw native-rate stereo audio (the GUI records it for
/// playback; see `audio_ring`).
pub fn run_live_session(
    config: NoteModelConfig,
    hop_seconds: f32,
    duration_s: Option<f32>,
    stop: Arc<AtomicBool>,
    key_mode: Arc<Mutex<KeyMode>>,
    accidental: Arc<Mutex<Accidental>>,
    tap: Option<Box<dyn live_capture::CaptureTap>>,
    on_event: impl FnMut(PipelineEvent) + Send,
) -> Result<()> {
    let on_event = Mutex::new(on_event);
    let beat_grid: Mutex<Option<BeatGrid>> = Mutex::new(None);
    let (beat_tx, beat_rx) = channel::<Vec<f32>>();
    std::thread::scope(|scope| {
        let beat_stop = &*stop;
        let on_event_ref = &on_event;
        let beat_grid_ref = &beat_grid;
        scope.spawn(move || run_beat_tracker(beat_rx, beat_stop, on_event_ref, beat_grid_ref));
        let mut emit = |event| (on_event.lock().unwrap())(event);
        let result = run_note_pipeline(config, hop_seconds, duration_s, &stop, &key_mode, &accidental, &beat_grid, tap, |chunk| {
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
    accidental: &Mutex<Accidental>,
    beat_grid: &Mutex<Option<BeatGrid>>,
    tap: Option<Box<dyn live_capture::CaptureTap>>,
    mut on_chunk: impl FnMut(&[f32]),
    mut on_event: impl FnMut(PipelineEvent),
) -> Result<()> {
    let model = HarmonicaModel::load(config.choice, config.allow_directml)?;
    let note_model = NoteModelInfo::of(&model, config.future_frames);
    let mut transcriber = NoteTranscriber::new(model, config.future_frames);
    let mut key_estimator = KeyEstimator::new();
    let mut chords = ChordEngine::standard();
    // What the user can change while the session runs, as it is right now.
    let settings_now = || (*key_mode.lock().unwrap(), *accidental.lock().unwrap(), *beat_grid.lock().unwrap());
    let mut elapsed = 0.0f32;

    let (_stream, rx) =
        live_capture::spawn_loopback_capture(hop_seconds, [harmonica_model::SAMPLE_RATE, beat_model::SAMPLE_RATE], tap)?;
    let total_latency_s = transcriber.max_note_delay_s() + hop_seconds + LATENCY_SAFETY_MARGIN_S;
    on_event(PipelineEvent::Started { total_latency_s, note_model, chord_vocabulary: chord_templates::vocabulary() });
    loop {
        // Loopback delivers no packets while nothing is playing, so the stop flag is polled
        // while waiting too -- otherwise stopping in silence would hang until sound resumed.
        let [note_chunk, beat_chunk] = match rx.recv_timeout(STOP_POLL) {
            Ok(chunks) => chunks,
            Err(RecvTimeoutError::Timeout) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        on_chunk(&beat_chunk);
        let blocks_before = transcriber.blocks_done();
        let notes = transcriber.push(&note_chunk)?;
        elapsed += hop_seconds;
        if transcriber.blocks_done() != blocks_before {
            let note_events = transcriber.note_events();
            let covered = transcriber.covered_seconds();
            emit_confirmed_notes(notes, &mut key_estimator, &mut on_event);
            for n in transcriber.take_started_notes() {
                on_event(PipelineEvent::NoteStarted { start_s: n.start_s, pitch_midi: n.pitch_midi, velocity: n.velocity, part: n.part });
            }
            let (current_key_mode, current_accidental, current_beat_grid) = settings_now();
            chords.advance(&note_events, covered, &mut key_estimator, &current_key_mode, current_accidental, current_beat_grid, &mut on_event);
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
    emit_confirmed_notes(final_notes, &mut key_estimator, &mut on_event);
    let (current_key_mode, current_accidental, current_beat_grid) = settings_now();
    chords.advance(
        &final_note_events,
        transcriber.covered_seconds(),
        &mut key_estimator,
        &current_key_mode,
        current_accidental,
        current_beat_grid,
        &mut on_event,
    );
    chords.finish(transcriber.covered_seconds(), current_accidental, &mut on_event);

    Ok(())
}
