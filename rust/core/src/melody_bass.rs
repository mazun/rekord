//! Labels each confirmed note as part of the melody (top voice) and/or bass
//! (bottom voice) line, using only the live note stream -- no extra model.
//!
//! The core is the classic "skyline" rule from symbolic-music melody
//! extraction (Uitdenbogerd & Zobel): whatever is the highest sounding
//! pitch is the melody, and, mirrored, the lowest is the bass. Everything
//! layered on top (the top/bottom-fraction test, `BASS_MAX_PITCH`, the
//! voice-leading jump gate) is this project's own adaptation for noisy,
//! transcribed-from-audio notes with no canonical prescription -- tune
//! empirically, don't treat those constants as sacred.
//!
//! Known limitation, inherent to the skyline rule: with block chords and no
//! separate melody, the chord's top note is labeled melody. The rule can't
//! tell "melody on top" from "just the highest chord tone".

use crate::harmonica_decode::{ConfirmedNote, NoteEvent};

// Resolution of the "is this note the top/bottom voice" sweep across a
// note's duration.
const GRID_S: f32 = 0.02;
// A note counts as the top (or bottom) voice if it holds that position for
// at least this fraction of its own duration -- a note that's only briefly
// on top (a neighbor starts above it shortly after) shouldn't flip label.
const MIN_EXTREME_FRACTION: f32 = 0.5;
// Only notes below this (C4 = MIDI 60) can be labeled bass. Without a
// register cutoff, a lone treble note with nothing beneath it is trivially
// the "lowest sounding" note and would be called bass.
const BASS_MAX_PITCH: i32 = 60;
// Mirror of the above: below this (C3 = MIDI 48) nothing is labeled melody.
// Same trivial-extreme problem in the other direction -- a bass note
// sounding alone is also the "highest" sounding note. Measured against
// Demucs-separated stems on a real recording (2026-09-21), about half of
// the wrongly-labeled melody notes were exactly this case.
const MELODY_MIN_PITCH: i32 = 48;
// Voice-leading gate for the melody line: a top-voice note this far (or
// further) from the previous melody note, within `MELODY_CONTINUITY_S` of
// it, is treated as not continuing that line (typically a transcription
// artifact such as a harmonic mistaken for a note, or a different voice
// entirely). Once the previous melody note is older than the continuity
// window, the gate lets any new top note start a fresh phrase.
const MELODY_CONTINUITY_S: f32 = 1.0;
const MAX_MELODY_JUMP_SEMITONES: i32 = 12;
// The transcriber's spurious high notes (harmonics/octave errors) show up
// as clearly weaker than the real notes sounding with them, and skyline is
// especially vulnerable to them because they land on top by construction.
// A note weaker than this fraction of the loudest note it overlaps neither
// counts as melody itself nor as "beating" a lower note for top-voice
// purposes. Only applied to the melody side: real bass notes are often
// quiet in the transcription, so the same cut would drop genuine ones.
// Loosely in the spirit of salience-based melody identification (Madsen &
// Widmer 2007), but a plain relative-velocity cut rather than their model.
const MIN_RELATIVE_VELOCITY: f32 = 0.6;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoteRoles {
    pub melody: bool,
    pub bass: bool,
}

pub struct LineTracker {
    // (pitch, end_s) of the most recently accepted melody note -- what the
    // voice-leading gate compares the next candidate against.
    last_melody: Option<(i32, f32)>,
}

impl LineTracker {
    pub fn new() -> Self {
        Self { last_melody: None }
    }

    /// Classify one newly confirmed note against `note_events` (the
    /// transcriber's current retained note list: recently finished notes
    /// plus still-open ones, each extended to the covered edge -- see
    /// `NoteTranscriber::note_events`). Every note that overlapped `note`
    /// while it sounded is present there by construction, whether or not it
    /// has itself finished yet, so the top/bottom test sees everything that
    /// sounded with it. Call in confirmation order.
    pub fn classify(&mut self, note: &ConfirmedNote, note_events: &[NoteEvent]) -> NoteRoles {
        let strong = strong_events(note_events);
        let note_is_strong = note.velocity
            >= MIN_RELATIVE_VELOCITY * loudest_overlapping(note.start_s, note.end_s, note_events);
        let mut melody = note.pitch_midi >= MELODY_MIN_PITCH
            && note_is_strong
            && extreme_fraction(note, &strong, true) >= MIN_EXTREME_FRACTION;
        if melody {
            if let Some((last_pitch, last_end)) = self.last_melody {
                let continuous = note.start_s - last_end <= MELODY_CONTINUITY_S;
                if continuous && (note.pitch_midi - last_pitch).abs() > MAX_MELODY_JUMP_SEMITONES {
                    melody = false;
                }
            }
        }
        if melody && self.last_melody.map_or(true, |(_, end)| note.end_s >= end) {
            self.last_melody = Some((note.pitch_midi, note.end_s));
        }

        let bass = note.pitch_midi < BASS_MAX_PITCH
            && extreme_fraction(note, note_events, false) >= MIN_EXTREME_FRACTION;

        NoteRoles { melody, bass }
    }
}

impl Default for LineTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Loudest amplitude among `events` overlapping `[start_s, end_s)`.
fn loudest_overlapping(start_s: f32, end_s: f32, events: &[NoteEvent]) -> f32 {
    events
        .iter()
        .filter(|&&(s, e, ..)| s < end_s && start_s < e)
        .map(|&(_s, _e, _p, a, _part)| a)
        .fold(0.0, f32::max)
}

/// `events` minus the notes too weak, relative to what they overlap, to be
/// trusted as top-voice material (see `MIN_RELATIVE_VELOCITY`).
fn strong_events(events: &[NoteEvent]) -> Vec<NoteEvent> {
    events
        .iter()
        .copied()
        .filter(|&(s, e, _p, a, _part)| a >= MIN_RELATIVE_VELOCITY * loudest_overlapping(s, e, events))
        .collect()
}

/// Fraction of `note`'s duration during which no other note in `events`
/// sounds above it (`top`) or below it (`!top`).
fn extreme_fraction(note: &ConfirmedNote, events: &[NoteEvent], top: bool) -> f32 {
    let duration = note.end_s - note.start_s;
    if duration <= 0.0 {
        return 0.0;
    }
    let steps = ((duration / GRID_S).ceil() as usize).max(1);
    let mut held = 0usize;
    for i in 0..steps {
        let t = note.start_s + (i as f32 + 0.5) * duration / steps as f32;
        let beaten = events.iter().any(|&(s, e, p, ..)| {
            s <= t && t < e && if top { p > note.pitch_midi } else { p < note.pitch_midi }
        });
        if !beaten {
            held += 1;
        }
    }
    held as f32 / steps as f32
}
