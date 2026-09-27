//! Rolling-window musical key estimation: correlate a duration/velocity-
//! weighted 12-bin chroma histogram against 24 reference key profiles
//! (Krumhansl-Schmuckler-style), with hysteresis so the displayed key
//! doesn't flicker chord-to-chord. Fed exclusively from `ConfirmedNote`s
//! (see `harmonica_decode.rs`) -- Harmonica's streaming decode emits each
//! note exactly once (see `harmonica_stream.rs`'s `RETAINED_SECONDS`), so
//! each note contributes to the histogram exactly once.

use std::collections::VecDeque;

use crate::chord_templates::{ChordMatch, NOTE_NAMES};
use crate::harmonica_decode::ConfirmedNote;

// Bellman & Budge (1999) key profiles, via
// https://extras.humdrum.org/man/keycor/ (also reproduced in music21's
// analysis/discrete.py as `BellmanBudge`). Chosen over the more commonly
// cited Aarden-Essen profile: music21's own source comments that Aarden-
// Essen's *minor* weights have uncertain provenance ("we are not sure
// exactly where the minor weightings come from, and recommend only using
// these weights for major"), and the single highest-scoring published
// alternative (Temperley-Kostka-Payne, ~96.8% vs Bellman-Budge's ~94.9%
// in published correlation-accuracy comparisons) has a documented "strong
// tendency to identify the relative major as tonic in minor keys" --
// exactly the major/minor distinction this feature's key-name display
// depends on getting right. Bellman-Budge carries no such documented bias.
const BELLMAN_BUDGE_MAJOR: [f32; 12] =
    [16.80, 0.86, 12.95, 1.41, 13.49, 11.93, 1.25, 20.28, 1.80, 8.04, 0.62, 10.57];
const BELLMAN_BUDGE_MINOR: [f32; 12] =
    [18.16, 0.69, 12.99, 13.34, 1.07, 11.15, 1.38, 21.07, 7.49, 1.53, 0.92, 10.21];

// Notes older than this contribute nothing to the current estimate --
// long enough to span several chords (so one passing/chromatic tone can't
// swing the estimate) but short enough to track a real modulation within
// a reasonable number of bars. No published key-finding literature
// prescribes a figure for streaming use (the classic papers score whole
// finished pieces, not a live rolling window); this is a starting point
// to tune empirically against sample_audio, mirroring the time-based-not-
// count-based retention convention already established in
// `harmonica_stream.rs`'s `RETAINED_SECONDS`.
const WINDOW_S: f32 = 12.0;

// Fewer than this many confirmed notes in the window and correlating
// against 24 profiles is noise -- e.g. a single melody note "correlates"
// best with whatever key puts it on scale degree 1, meaninglessly.
const MIN_NOTES_FOR_ESTIMATE: usize = 3;

// A candidate key must out-correlate the currently-displayed key by at
// least this much, continuously, for at least MIN_HOLD_S, before the
// display actually switches. Relative major/minor share all seven
// diatonic pitch classes, so without both guards a single ambiguous
// chord (e.g. a secondary dominant, or a borrowed tone) would flip the
// label back and forth.
const SWITCH_MARGIN: f32 = 0.05;
const MIN_HOLD_S: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Major,
    Minor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyEstimate {
    pub tonic_pc: i32,
    pub mode: Mode,
}

impl KeyEstimate {
    /// Always shows both readings of the major/minor pair (e.g. "C Major
    /// (A Minor)"), never just one -- by this project's own choice, since
    /// the major/minor distinction carries little weight in how this
    /// feature is actually used, while the relative-major reading is what
    /// `roman_numeral()`'s degrees are already spelled against regardless
    /// of which mode was detected. Showing both unconditionally (instead
    /// of only appending "(X Minor)" when `mode == Minor`) also has the
    /// side effect of making the displayed text identical no matter which
    /// of the pair the correlator actually locked onto -- relative major
    /// and minor share all seven diatonic pitch classes, so that call is
    /// inherently the noisiest part of the estimate (see `SWITCH_MARGIN`)
    /// and this way the noise never surfaces as a visibly different label.
    pub fn name(&self) -> String {
        let major_pc = self.relative_major_pc();
        let minor_pc = (major_pc - 3).rem_euclid(12);
        format!("{} Major ({} Minor)", NOTE_NAMES[major_pc as usize], NOTE_NAMES[minor_pc as usize])
    }

    /// Roman-numeral degrees are always spelled against this project's
    /// chosen reference: the key's own tonic if major, or its relative
    /// major (+3 semitones) if minor -- e.g. in A minor, a C-major chord
    /// (the relative major's own tonic) reads "I", and the A-minor tonic
    /// chord itself reads "VIm", never "i" as if A were degree 1.
    fn relative_major_pc(&self) -> i32 {
        match self.mode {
            Mode::Major => self.tonic_pc,
            Mode::Minor => (self.tonic_pc + 3).rem_euclid(12),
        }
    }
}

struct WeightedNote {
    pc: i32,
    start_s: f32,
    weight: f32,
}

pub struct KeyEstimator {
    history: VecDeque<WeightedNote>,
    displayed: Option<KeyEstimate>,
    // The best-correlated challenger to `displayed`, and when it first
    // started leading -- reset whenever the leader changes, so a
    // switch only commits once the *same* candidate has led for
    // MIN_HOLD_S straight, not just cumulatively.
    pending: Option<(KeyEstimate, f32)>,
}

impl KeyEstimator {
    pub fn new() -> Self {
        Self { history: VecDeque::new(), displayed: None, pending: None }
    }

    /// Feed one just-confirmed note. Intended to be called once per
    /// `ConfirmedNote` as `NoteTranscriber` (via `emit_confirmed_notes`)
    /// emits it.
    pub fn observe(&mut self, note: &ConfirmedNote) {
        self.history.push_back(WeightedNote {
            pc: note.pitch_midi.rem_euclid(12),
            start_s: note.start_s,
            weight: note.velocity * (note.end_s - note.start_s).max(0.05),
        });
    }

    /// Re-evaluate at time `now_s`, applying hysteresis, and return the
    /// key to display -- `None` until `MIN_NOTES_FOR_ESTIMATE` is met
    /// (e.g. near session start). Call once per checkpoint alongside
    /// chord re-evaluation, so a modulation is caught even across a
    /// stretch covered by a single held chord.
    pub fn estimate(&mut self, now_s: f32) -> Option<KeyEstimate> {
        // Must scan and drop stale entries wherever they sit, not just at
        // the front: `observe()` is called in *confirmation* order
        // (`NoteTranscriber` emits each note exactly once, the moment its
        // end frame is decided -- see harmonica_stream.rs -- which isn't
        // necessarily start_s order). A long-held bass note routinely gets
        // confirmed, and so pushed, after a shorter later-starting treble
        // note above it -- an everyday case in real piano playing, not an
        // edge case. A front-only pop loop would then leave that bass
        // note's now-stale entry stuck behind the treble note forever,
        // growing `history` without bound for the rest of the session
        // (each `estimate()` call rescanning an ever-larger list) instead
        // of staying capped at roughly WINDOW_S worth of notes.
        self.history.retain(|n| now_s - n.start_s <= WINDOW_S);
        // A sparse passage or a pause can drop the note count below
        // MIN_NOTES_FOR_ESTIMATE for a while. Without clearing `pending`
        // here too, a candidate that was partway through accumulating its
        // MIN_HOLD_S lead before the gap would still hold that stale
        // timestamp once evaluation resumes -- so if it's still leading
        // on the very first post-gap sample (an ordinary outcome, not a
        // rare coincidence, if the music continues in that direction),
        // `now_s - since` would already exceed MIN_HOLD_S and the display
        // would switch instantly, defeating the whole point of requiring
        // a *continuous* lead.
        let Some((best_key, best_score)) = self.best_correlated_key() else {
            self.pending = None;
            return None;
        };
        let displayed = match self.displayed {
            None => {
                self.pending = None;
                best_key
            }
            Some(cur) if cur == best_key => {
                self.pending = None;
                cur
            }
            Some(cur) => {
                let cur_score = self.correlation_for(cur);
                if best_score - cur_score >= SWITCH_MARGIN {
                    match self.pending {
                        Some((cand, since)) if cand == best_key && now_s - since >= MIN_HOLD_S => {
                            self.pending = None;
                            best_key
                        }
                        Some((cand, _)) if cand == best_key => cur,
                        _ => {
                            self.pending = Some((best_key, now_s));
                            cur
                        }
                    }
                } else {
                    self.pending = None;
                    cur
                }
            }
        };
        self.displayed = Some(displayed);
        self.displayed
    }

    fn chroma(&self) -> [f32; 12] {
        let mut bins = [0f32; 12];
        for n in &self.history {
            bins[n.pc as usize] += n.weight;
        }
        bins
    }

    fn correlation_for(&self, key: KeyEstimate) -> f32 {
        let profile = match key.mode {
            Mode::Major => &BELLMAN_BUDGE_MAJOR,
            Mode::Minor => &BELLMAN_BUDGE_MINOR,
        };
        pearson(&self.chroma(), &rotated(profile, key.tonic_pc))
    }

    fn best_correlated_key(&self) -> Option<(KeyEstimate, f32)> {
        if self.history.len() < MIN_NOTES_FOR_ESTIMATE {
            return None;
        }
        let chroma = self.chroma();
        let mut best: Option<(KeyEstimate, f32)> = None;
        for tonic in 0..12i32 {
            for (mode, profile) in [(Mode::Major, &BELLMAN_BUDGE_MAJOR), (Mode::Minor, &BELLMAN_BUDGE_MINOR)] {
                let score = pearson(&chroma, &rotated(profile, tonic));
                let candidate = KeyEstimate { tonic_pc: tonic, mode };
                if best.map_or(true, |(_, s)| score > s) {
                    best = Some((candidate, score));
                }
            }
        }
        best
    }
}

fn rotated(profile: &[f32; 12], tonic_pc: i32) -> [f32; 12] {
    let mut out = [0f32; 12];
    for (pc, slot) in out.iter_mut().enumerate() {
        *slot = profile[(pc as i32 - tonic_pc).rem_euclid(12) as usize];
    }
    out
}

fn pearson(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let mean_a = a.iter().sum::<f32>() / 12.0;
    let mean_b = b.iter().sum::<f32>() / 12.0;
    let (mut cov, mut var_a, mut var_b) = (0f32, 0f32, 0f32);
    for i in 0..12 {
        let (da, db) = (a[i] - mean_a, b[i] - mean_b);
        cov += da * db;
        var_a += da * da;
        var_b += db * db;
    }
    if var_a <= f32::EPSILON || var_b <= f32::EPSILON {
        return 0.0;
    }
    cov / (var_a.sqrt() * var_b.sqrt())
}

// Degree-numeral names by semitone offset from the reference major tonic.
// Chromatic (non-diatonic) roots use the flat-mediant convention common
// in popular-music analysis (bIII, bVI, bVII), except the tritone (#IV),
// far more often reached as a secondary-dominant root (V/V) than named
// "bV".
const DEGREE_NAMES: [&str; 12] =
    ["I", "bII", "II", "bIII", "III", "IV", "#IV", "V", "bVI", "VI", "bVII", "VII"];

/// Roman-numeral degree name for `chord`'s root relative to `key`'s
/// governing major scale, carrying the same quality suffix a normal chord
/// label would show (e.g. a vi-position m7 chord reads "VIm7", exactly as
/// `ChordMatch::label()` shows "Am7" for that chord in root position) --
/// no separate case/symbol scheme, so anything the chord matcher can
/// name, this can label too.
pub fn roman_numeral(key: &KeyEstimate, chord: &ChordMatch) -> String {
    let degree = (chord.root_pc - key.relative_major_pc()).rem_euclid(12) as usize;
    format!("{}{}", DEGREE_NAMES[degree], chord.quality_suffix())
}
