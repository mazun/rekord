//! Runs every chord layer over the same checkpoints and turns what they decide into pipeline
//! events: for each layer a `ChordLayer` event per block (the chords it newly decided and how far
//! it has decided), plus the key (`KeyChange`) and which pitch classes sound (`ActiveNotes`).
//! The frontend composes the layers into the piano roll's chord lane (docs/chord-layers.md).

use super::confirmed::{ConfirmedLayer, ConfirmedParams};
use super::evidence::{active_mask, chord_checkpoints};
use super::hold::{HoldLayer, HoldParams};
use super::{ChordAt, ChordLayer, ChordLayerId, LayerChange, StepCtx};
use crate::beat_tracker::BeatGrid;
use crate::harmonica_decode::NoteEvent;
use crate::key_estimator::{roman_numeral, KeyEstimate, KeyEstimator};
use crate::pipeline::{KeyMode, PipelineEvent};
use crate::spelling::Accidental;

/// One chord a layer decided, as sent to the frontend: from `t` on (until the layer's next
/// change) the layer says `label`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct LayerChord {
    /// When the chord starts (audio time). Never earlier than what the layer had already reported
    /// as decided, and not later than the `decided_s` of the event it comes in.
    pub t: f32,
    /// The checkpoint at which the layer announced it (`>= t`, usually a little later: the layer's
    /// delay). Not needed to draw anything; for measuring the delay.
    pub emit_s: f32,
    pub label: String,
    /// The chord's degree in the key at the time, if a key was known.
    pub roman_numeral: Option<String>,
    /// `None` for silence.
    pub root_pc: Option<i32>,
    pub quality: Option<&'static str>,
}

/// The pitch classes sounding (bit `pc` set) from `t` on, every part included.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct ActiveChange {
    pub t: f32,
    pub pcs: u16,
}

struct Slot {
    id: ChordLayerId,
    layer: Box<dyn ChordLayer>,
    /// How far the layer has decided, as last accepted (what the next change may not precede).
    decided_s: f32,
    /// The `decided_s` last sent in an event.
    sent_decided_s: f32,
    /// The latest change: its time, chord and Roman numeral (to respell it when the accidental
    /// setting changes).
    last: Option<(f32, ChordAt, Option<String>)>,
    pending: Vec<LayerChord>,
}

impl Slot {
    fn chord(t: f32, emit_s: f32, chord: ChordAt, roman: Option<String>, key: Option<KeyEstimate>, accidental: Accidental) -> LayerChord {
        let (root_pc, quality) = match chord {
            ChordAt::Match(m) => (Some(m.root_pc), Some(m.quality)),
            ChordAt::Silence => (None, None),
        };
        LayerChord { t, emit_s, label: chord.label(key.as_ref(), accidental), roman_numeral: roman, root_pc, quality }
    }

    /// Takes what the layer decided at checkpoint `emit_s`. A change is held to the layer's
    /// contract here: not before what it had already reported decided or its own previous change,
    /// not after the new `decided_s` (a layer's snapping to the beat grid can step out of that).
    fn accept(&mut self, changes: Vec<LayerChange>, decided_s: f32, emit_s: f32, key: Option<KeyEstimate>, accidental: Accidental) {
        for c in changes {
            let lo = self.decided_s.max(self.last.as_ref().map_or(f32::NEG_INFINITY, |l| l.0));
            let t = c.t.min(decided_s).max(lo);
            let roman = match (&c.chord, key) {
                (ChordAt::Match(m), Some(k)) => Some(roman_numeral(&k, m)),
                _ => None,
            };
            self.pending.push(Self::chord(t, emit_s, c.chord, roman.clone(), key, accidental));
            self.last = Some((t, c.chord, roman));
        }
        self.decided_s = self.decided_s.max(decided_s);
    }
}

pub struct ChordEngine {
    slots: Vec<Slot>,
    last_accidental: Option<Accidental>,
    last_key: Option<KeyEstimate>,
    last_mask: Option<u16>,
    /// The name of the key last reported in a `KeyChange` (a respelling is a change).
    last_key_name: Option<String>,
    /// Where the previous `advance` ended: the next one starts its checkpoints after it.
    last_covered: f32,
    active: Vec<ActiveChange>,
}

impl ChordEngine {
    pub fn new(layers: Vec<(ChordLayerId, Box<dyn ChordLayer>)>) -> Self {
        let slots = layers
            .into_iter()
            .map(|(id, layer)| Slot {
                id,
                layer,
                decided_s: f32::NEG_INFINITY,
                sent_decided_s: f32::NEG_INFINITY,
                last: None,
                pending: Vec::new(),
            })
            .collect();
        Self { slots, last_accidental: None, last_key: None, last_mask: None, last_key_name: None, last_covered: 0.0, active: Vec::new() }
    }

    fn hold_layers(instant: HoldParams, settled: HoldParams) -> Vec<(ChordLayerId, Box<dyn ChordLayer>)> {
        vec![
            (ChordLayerId::Instant, Box::new(HoldLayer::new(instant))),
            (ChordLayerId::Settled, Box::new(HoldLayer::new(settled))),
        ]
    }

    /// The instant layer and the settled one.
    pub fn with_params(instant: HoldParams, settled: HoldParams) -> Self {
        Self::new(Self::hold_layers(instant, settled))
    }

    /// The two layers and the one that waits 2 s as well.
    pub fn with_confirmed(instant: HoldParams, settled: HoldParams, confirmed: ConfirmedParams) -> Self {
        let mut layers = Self::hold_layers(instant, settled);
        layers.push((ChordLayerId::Confirmed, Box::new(ConfirmedLayer::new(confirmed))));
        Self::new(layers)
    }

    /// The layers a session runs: all three, with the default numbers. For trying other numbers in
    /// the running app, the environment variables `REKORD_CHORD_TUNE_INSTANT` /
    /// `REKORD_CHORD_TUNE_SETTLED` hold a `HoldParams::apply_tune` list each (e.g. `h=0.05,c=0`) and
    /// `REKORD_CHORD_TUNE_CONFIRMED` a `ConfirmedParams::apply_tune` list (e.g. `D=1.5`), or `off`
    /// to run without the 2 s layer.
    pub fn standard() -> Self {
        fn tuned<P: Copy + std::fmt::Debug>(mut params: P, var: &str, apply: fn(&mut P, &str) -> Result<(), String>) -> P {
            if let Ok(tune) = std::env::var(var) {
                match apply(&mut params, &tune) {
                    Ok(()) => eprintln!("{var}: {params:?}"),
                    Err(e) => eprintln!("{var} ignored: {e}"),
                }
            }
            params
        }
        let instant = tuned(HoldParams::instant(), "REKORD_CHORD_TUNE_INSTANT", HoldParams::apply_tune);
        let settled = tuned(HoldParams::settled(), "REKORD_CHORD_TUNE_SETTLED", HoldParams::apply_tune);
        if std::env::var("REKORD_CHORD_TUNE_CONFIRMED").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off")) {
            eprintln!("REKORD_CHORD_TUNE_CONFIRMED: off (no 2 s layer)");
            return Self::with_params(instant, settled);
        }
        Self::with_confirmed(instant, settled, tuned(ConfirmedParams::standard(), "REKORD_CHORD_TUNE_CONFIRMED", ConfirmedParams::apply_tune))
    }

    /// Runs all layers over the checkpoints after the previous call's `covered` up to `covered`
    /// (see `chord_checkpoints`) and reports the key and every layer's new decisions.
    /// `note_events` is what `NoteTranscriber::note_events()` returned at `covered`; `beat_grid`
    /// is the beat tracker's current grid, if any is known yet (`None` in file mode, which has no
    /// beat tracker).
    pub fn advance(
        &mut self,
        note_events: &[NoteEvent],
        covered: f32,
        key_estimator: &mut KeyEstimator,
        key_mode: &KeyMode,
        accidental: Accidental,
        beat_grid: Option<BeatGrid>,
        mut on_event: impl FnMut(PipelineEvent),
    ) {
        let prev_covered = std::mem::replace(&mut self.last_covered, covered);
        for t in chord_checkpoints(note_events, prev_covered, covered) {
            // Re-evaluate the key first: it can update mid-chord, and doing this before the chords
            // lets a just-updated key inform this same checkpoint's labels instead of lagging a
            // step. The estimator is still fed/advanced in Manual mode (its history and
            // hysteresis stay warm for whenever the user switches back to Auto) -- only its
            // output is ignored while a manual pick is in effect.
            let auto_key = key_estimator.estimate(t);
            let current_key = match key_mode {
                KeyMode::Auto => auto_key,
                KeyMode::Manual(key) => Some(*key),
            };
            if let Some(key) = current_key {
                let key_name = key.name(accidental);
                if Some(&key_name) != self.last_key_name.as_ref() {
                    on_event(PipelineEvent::KeyChange { t, key_name: key_name.clone(), tonic_pc: key.relative_major_pc() });
                    self.last_key_name = Some(key_name);
                }
            }
            self.last_key = current_key;

            // The sharp/flat setting changed: the chord each layer ends on is announced again in
            // the new spelling, at its own time (the frontend replaces the entry); chords already
            // scrolled into the past keep the spelling they had.
            if self.last_accidental.is_some_and(|a| a != accidental) {
                for slot in &mut self.slots {
                    if let Some((lt, chord, roman)) = slot.last.clone() {
                        slot.pending.push(Slot::chord(lt, t, chord, roman, current_key, accidental));
                    }
                }
            }
            self.last_accidental = Some(accidental);

            let mask = active_mask(note_events, t);
            if self.last_mask != Some(mask) {
                self.active.push(ActiveChange { t, pcs: mask });
                self.last_mask = Some(mask);
            }

            // Naming a bare fifth after the key is a guess about the 3rd, and the estimator knows
            // sooner than it shows that the key is changing.
            let naming_key = if matches!(key_mode, KeyMode::Auto) { key_estimator.naming_key().or(current_key) } else { current_key };
            let ctx = StepCtx { notes: note_events, key: naming_key, beat_grid };
            for slot in &mut self.slots {
                let mut out = Vec::new();
                let decided = slot.layer.step(t, &ctx, &mut out);
                slot.accept(out, decided, t, current_key, accidental);
            }
        }
        self.flush(&mut on_event);
    }

    /// No more input (capture stopped or the file ended): every layer decides what is still
    /// pending as of `covered`, so a stopped roll is entirely settled.
    pub fn finish(&mut self, covered: f32, accidental: Accidental, mut on_event: impl FnMut(PipelineEvent)) {
        let key = self.last_key;
        for slot in &mut self.slots {
            let mut out = Vec::new();
            slot.layer.finish(covered, &mut out);
            slot.accept(out, covered, covered, key, accidental);
        }
        self.flush(&mut on_event);
    }

    fn flush(&mut self, on_event: &mut impl FnMut(PipelineEvent)) {
        if !self.active.is_empty() {
            on_event(PipelineEvent::ActiveNotes { changes: std::mem::take(&mut self.active) });
        }
        for slot in &mut self.slots {
            if slot.decided_s.is_finite() && (!slot.pending.is_empty() || slot.decided_s > slot.sent_decided_s) {
                on_event(PipelineEvent::ChordLayer {
                    layer: slot.id,
                    changes: std::mem::take(&mut slot.pending),
                    decided_s: slot.decided_s,
                });
                slot.sent_decided_s = slot.decided_s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harmonica_model::Part;

    fn chord(start: f32, end: f32, pitches: &[i32]) -> Vec<NoteEvent> {
        pitches.iter().map(|&p| (start, end, p, 0.8, Part::Other)).collect()
    }

    /// Runs the engine over C (0-2 s) / Am (2-4 s) / F (4-6 s) and then silence, in 0.256 s
    /// blocks as the file mode does, and returns every event.
    fn run() -> Vec<PipelineEvent> {
        let mut notes = chord(0.0, 2.0, &[48, 52, 55, 60]);
        notes.extend(chord(2.0, 4.0, &[45, 52, 57, 60]));
        notes.extend(chord(4.0, 6.0, &[41, 53, 57, 60]));
        run_engine(ChordEngine::with_params(HoldParams::instant(), HoldParams::settled()), &notes, 6.5)
    }

    fn run_engine(mut engine: ChordEngine, notes: &[NoteEvent], end_s: f32) -> Vec<PipelineEvent> {
        let mut key = KeyEstimator::new();
        let mut events = Vec::new();
        let mut k = 1;
        loop {
            let covered = (k as f32 * 0.256).min(end_s);
            let visible: Vec<NoteEvent> = notes.iter().filter(|n| n.0 <= covered).map(|&(s, e, p, v, part)| (s, e.min(covered), p, v, part)).collect();
            engine.advance(&visible, covered, &mut key, &KeyMode::Auto, Accidental::Sharp, None, |e| events.push(e));
            if covered >= end_s {
                engine.finish(covered, Accidental::Sharp, |e| events.push(e));
                break;
            }
            k += 1;
        }
        events
    }

    fn layer_events(events: &[PipelineEvent], id: ChordLayerId) -> Vec<(&[LayerChord], f32)> {
        events
            .iter()
            .filter_map(|e| match e {
                PipelineEvent::ChordLayer { layer, changes, decided_s } if *layer == id => Some((changes.as_slice(), *decided_s)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn layers_keep_their_contract_in_the_events() {
        let events = run();
        for id in [ChordLayerId::Instant, ChordLayerId::Settled] {
            let mut decided = f32::NEG_INFINITY;
            let mut last_t = f32::NEG_INFINITY;
            for (changes, new_decided) in layer_events(&events, id) {
                assert!(new_decided >= decided, "{id:?}: decided_s went back");
                for c in changes {
                    assert!(c.t >= decided - 1e-4 && c.t >= last_t - 1e-4, "{id:?}: {} at {} precedes decided {decided} / last {last_t}", c.label, c.t);
                    assert!(c.t <= new_decided + 1e-4, "{id:?}: {} at {} is past decided {new_decided}", c.label, c.t);
                    assert!(c.emit_s >= c.t, "{id:?}: announced before it started");
                    last_t = c.t;
                }
                decided = new_decided;
            }
            assert!(decided >= 6.5 - 1e-4, "{id:?}: not settled at the end ({decided})");
        }
    }

    #[test]
    fn the_instant_layer_names_each_chord_before_the_settled_one() {
        let events = run();
        let labels = |id| -> Vec<(String, f32)> {
            layer_events(&events, id).into_iter().flat_map(|(cs, _)| cs.iter().map(|c| (c.label.clone(), c.emit_s))).collect()
        };
        let (instant, settled) = (labels(ChordLayerId::Instant), labels(ChordLayerId::Settled));
        let names = |v: &[(String, f32)]| v.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>();
        assert_eq!(names(&instant), ["C", "Am", "F", "--"]);
        // (the settled layer needs 0.5 s of silence plus its 0.25 s hold: the 0.5 s tail is too short)
        assert_eq!(names(&settled), ["C", "Am", "F"]);
        for ((_, a), (_, b)) in instant.iter().zip(&settled).skip(1) {
            assert!(a < b);
        }
    }

    #[test]
    fn the_confirmed_layer_decides_last_and_agrees_on_a_clean_progression() {
        let mut notes = chord(0.0, 4.0, &[48, 52, 55, 60]);
        notes.extend(chord(4.0, 8.0, &[45, 52, 57, 60]));
        notes.extend(chord(8.0, 12.0, &[41, 53, 57, 60]));
        let engine = ChordEngine::with_confirmed(HoldParams::instant(), HoldParams::settled(), ConfirmedParams::standard());
        let events = run_engine(engine, &notes, 14.0);
        let labels = |id| -> Vec<(String, f32)> {
            layer_events(&events, id).into_iter().flat_map(|(cs, _)| cs.iter().map(|c| (c.label.clone(), c.emit_s))).collect()
        };
        let (settled, confirmed) = (labels(ChordLayerId::Settled), labels(ChordLayerId::Confirmed));
        let names = |v: &[(String, f32)]| v.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>();
        assert_eq!(names(&confirmed), ["C", "Am", "F", "--"]);
        assert_eq!(&names(&settled)[..3], ["C", "Am", "F"]);
        for ((_, at_settled), (_, at_confirmed)) in settled.iter().zip(&confirmed) {
            assert!(at_confirmed > at_settled);
        }
        // the layer's contract holds in the events, and it is settled at the end
        let mut decided = f32::NEG_INFINITY;
        for (changes, new_decided) in layer_events(&events, ChordLayerId::Confirmed) {
            assert!(new_decided >= decided);
            assert!(changes.iter().all(|c| c.t >= decided - 1e-4 && c.t <= new_decided + 1e-4));
            decided = new_decided;
        }
        assert!(decided >= 14.0 - 1e-4);
    }

    #[test]
    fn a_new_spelling_replaces_the_last_chord_of_each_layer_in_place() {
        // A black-key chord, then the setting switches to flats: the chord is announced again in
        // the new spelling with the same start time (the frontend replaces the entry).
        let mut notes = chord(0.0, 4.0, &[46, 50, 53]); // B-flat major
        notes.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut engine = ChordEngine::with_params(HoldParams::instant(), HoldParams::settled());
        let mut key = KeyEstimator::new();
        let mut events = Vec::new();
        for (i, accidental) in [(1, Accidental::Sharp), (2, Accidental::Sharp), (3, Accidental::Sharp), (4, Accidental::Flat)] {
            let covered = i as f32;
            let visible: Vec<NoteEvent> = notes.iter().map(|&(s, e, p, v, part)| (s, e.min(covered), p, v, part)).collect();
            engine.advance(&visible, covered, &mut key, &KeyMode::Auto, accidental, None, |e| events.push(e));
        }
        let instant: Vec<&LayerChord> = layer_events(&events, ChordLayerId::Instant).into_iter().flat_map(|(cs, _)| cs.iter()).collect();
        assert_eq!(instant.len(), 2, "{instant:?}");
        assert_eq!(instant[0].label, "A♯");
        assert_eq!(instant[1].label, "B♭");
        assert_eq!(instant[0].t, instant[1].t);
    }
}
