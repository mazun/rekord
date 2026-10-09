//! The chord estimate in layers of increasing delay (see `docs/chord-layers.md`): an instant one,
//! one that waits ~0.5 s and one that waits 2 s. Each layer is append-only -- it
//! reports the times its chord changes and how far it has decided -- and a later layer corrects an
//! earlier one only by being shown in its place (that composition happens in the frontend).
//!
//! - `evidence`: what the layers read off the notes (windowed pitch-class energy, checkpoints).
//! - `hold`: the layer that names the chord from a window and shows a different one only after it
//!   has led for a while; the instant and the settled layer are this with different numbers.
//! - `confirmed`: the layer that waits 2 s and searches the whole vocabulary for the chord
//!   sequence that the notes before and after each moment explain best.
//! - `engine`: runs the layers over the same checkpoints and emits their decisions as events.

pub mod confirmed;
pub mod engine;
pub mod evidence;
pub mod hold;

pub use confirmed::{ConfirmedLayer, ConfirmedParams, Naming};
pub use engine::{ActiveChange, ChordEngine, LayerChord};
pub use hold::{HoldLayer, HoldParams};

use crate::beat_tracker::BeatGrid;
use crate::chord_templates::ChordMatch;
use crate::harmonica_decode::NoteEvent;
use crate::key_estimator::KeyEstimate;
use crate::spelling::Accidental;

/// What's sounding at some time, if anything conclusive can be said. Kept as the matched
/// `ChordMatch` (not just its formatted label) so a caller can also derive a Roman-numeral degree
/// from `root_pc`/`quality` without reparsing a display string.
#[derive(Clone, Copy, PartialEq)]
pub enum ChordAt {
    Silence,
    Match(ChordMatch),
}

impl ChordAt {
    pub fn label(&self, key: Option<&KeyEstimate>, mode: Accidental) -> String {
        match self {
            ChordAt::Silence => "(silence)".to_string(),
            ChordAt::Match(m) => m.label(key, mode),
        }
    }
}

/// Which layer. Serialized as the lowercase name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChordLayerId {
    /// Names the chord as soon as the notes show it.
    Instant,
    /// Waits about 0.5 s: the window and hold keep a passing tone from flipping the name.
    Settled,
    /// Waits 2 s and searches for the chord sequence the notes before and after explain best.
    Confirmed,
}

/// "From `t` on, the chord is `chord`" -- until the layer's next change.
#[derive(Clone, Copy, PartialEq)]
pub struct LayerChange {
    pub t: f32,
    pub chord: ChordAt,
}

/// What a layer sees at one checkpoint.
pub struct StepCtx<'a> {
    /// `NoteTranscriber::note_events()`: every note of the last few seconds; the ones still
    /// sounding end at `covered`.
    pub notes: &'a [NoteEvent],
    /// The key estimate at this checkpoint (only used for the key-aware naming rules).
    pub key: Option<KeyEstimate>,
    pub beat_grid: Option<BeatGrid>,
}

pub trait ChordLayer {
    /// Called at every checkpoint `t` (time-ordered, `t <= covered`). Appends the changes the
    /// layer has newly decided to `out` and returns how far it has decided: from now on it only
    /// reports changes at or after that time, and no change reported earlier is revised.
    fn step(&mut self, t: f32, ctx: &StepCtx, out: &mut Vec<LayerChange>) -> f32;

    /// No more input: decides whatever is still pending, as of `covered`.
    fn finish(&mut self, covered: f32, out: &mut Vec<LayerChange>);
}
