//! Rolling-window musical key estimation: correlate a duration/velocity-
//! weighted 12-bin chroma histogram against 24 reference key profiles
//! (Krumhansl-Schmuckler-style), with a switching rule (accumulated evidence, see
//! `estimate_cusum`) so the displayed key
//! doesn't flicker chord-to-chord. Fed exclusively from `ConfirmedNote`s
//! (see `harmonica_decode.rs`) -- Harmonica's streaming decode emits each
//! note exactly once (see `harmonica_stream.rs`'s `RETAINED_SECONDS`), so
//! each note contributes to the histogram exactly once.

use std::collections::VecDeque;

use crate::chord_templates::ChordMatch;
use crate::harmonica_decode::ConfirmedNote;
use crate::spelling::{key_name, Accidental};

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
//
// The numbers below were re-tuned on 2026-10-09 (docs/chord-layers.md, section 11) against
// reference keys read off RWC-Pop's chord annotations (with look-ahead) and one song with
// known modulations; the first design was a flat 12 s window with the hysteresis below
// (`l=0,w=12,d=0,mp=0` in `REKORD_KEY_TUNE` brings it back). On 100 songs the new numbers
// agree with the reference 75% of the time instead of 69%, follow a modulation 1.7 s sooner
// (7.0 s) and flicker a little more (2.8 switches a minute against 2.3; the reference has 1.0).
const WINDOW_S: f32 = 8.0;
// A note counts `exp(-age / decay)` of its weight, so a modulation outweighs the old key sooner
// than a flat window lets it; 0 = the plain flat window.
const DECAY_S: f32 = 2.0;
// Subtracted from a minor key's correlation. A bare-fifth riff has no 3rd, and with the plain
// correlation it read as the minor key whose scale is the old key's (A major's chorus as A minor =
// C major) until a major 3rd finally sounded; major is also the usual mode in pop.
const MINOR_PENALTY: f32 = 0.1;
// How much a challenger must gain on the shown key (correlation x seconds) to replace it; see
// `KeyEstimator::estimate_cusum`. 0 = the hysteresis below instead.
const CUSUM_S: f32 = 1.0;
// Chord naming (not the key readout) already goes with a challenger that has gained this share of
// `CUSUM_S`, see `KeyEstimator::naming_key`.
const NAMING_DOUBT: f32 = 0.5;

// Fewer than this many confirmed notes in the window and correlating
// against 24 profiles is noise -- e.g. a single melody note "correlates"
// best with whatever key puts it on scale degree 1, meaninglessly.
const MIN_NOTES_FOR_ESTIMATE: usize = 3;

// The first design's switching rule (still there for `CUSUM_S = 0`): a candidate key must
// out-correlate the currently-displayed key by at
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
    pub fn name(&self, mode: Accidental) -> String {
        key_name(self, mode)
    }

    /// Roman-numeral degrees are always spelled against this project's
    /// chosen reference: the key's own tonic if major, or its relative
    /// major (+3 semitones) if minor -- e.g. in A minor, a C-major chord
    /// (the relative major's own tonic) reads "I", and the A-minor tonic
    /// chord itself reads "VIm", never "i" as if A were degree 1.
    pub fn relative_major_pc(&self) -> i32 {
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

/// The estimator's numbers (the constants above are the defaults). `REKORD_KEY_TUNE` overrides
/// them in a running app or the replay tool: `w` window s, `d` decay s, `mp` minor penalty,
/// `l` cumulative gain needed (0 = the margin / hold rule instead), `df` share of it at which
/// chord naming switches, `m` switch margin and `h` hold s (those two only without `l`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyParams {
    pub window_s: f32,
    pub decay_s: f32,
    pub switch_margin: f32,
    pub min_hold_s: f32,
    /// Subtracted from a minor key's correlation: a major key wins when the 3rd is too thin to
    /// tell (a bare-fifth riff), which is the usual case in pop; a minor key needs its 3rd.
    pub minor_penalty: f32,
    /// > 0: track the key by accumulating evidence instead of the margin / hold hysteresis (see
    /// `KeyEstimator::estimate_cusum`): a challenger takes over once it has gained this much
    /// (correlation x seconds) on the current key. 0 = the original hysteresis.
    pub cusum_s: f32,
    /// With `cusum_s`: the key is "in doubt" (`KeyEstimator::in_doubt`) while a challenger has
    /// gained this fraction of `cusum_s`; 0 = never.
    pub doubt: f32,
}

impl Default for KeyParams {
    fn default() -> Self {
        Self {
            window_s: WINDOW_S,
            decay_s: DECAY_S,
            switch_margin: SWITCH_MARGIN,
            min_hold_s: MIN_HOLD_S,
            minor_penalty: MINOR_PENALTY,
            cusum_s: CUSUM_S,
            doubt: NAMING_DOUBT,
        }
    }
}

impl KeyParams {
    pub fn apply_tune(&mut self, tune: &str) -> Result<(), String> {
        for kv in tune.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').ok_or_else(|| format!("expected key=value, got {kv:?}"))?;
            let num = v.parse::<f32>().map_err(|_| format!("{k} needs a number, got {v:?}"))?;
            match k {
                "w" => self.window_s = num,
                "d" => self.decay_s = num,
                "m" => self.switch_margin = num,
                "h" => self.min_hold_s = num,
                "mp" => self.minor_penalty = num,
                "l" => self.cusum_s = num,
                "df" => self.doubt = num,
                _ => return Err(format!("unknown key {k:?}")),
            }
        }
        Ok(())
    }

    /// The defaults, tuned by `REKORD_KEY_TUNE` if it is set.
    pub fn standard() -> Self {
        let mut params = Self::default();
        if let Ok(tune) = std::env::var("REKORD_KEY_TUNE") {
            match params.apply_tune(&tune) {
                Ok(()) => eprintln!("REKORD_KEY_TUNE: {params:?}"),
                Err(e) => eprintln!("REKORD_KEY_TUNE ignored: {e}"),
            }
        }
        params
    }
}

pub struct KeyEstimator {
    params: KeyParams,
    history: VecDeque<WeightedNote>,
    now_s: f32,
    displayed: Option<KeyEstimate>,
    // The best-correlated challenger to `displayed`, and when it first
    // started leading -- reset whenever the leader changes, so a
    // switch only commits once the *same* candidate has led for
    // MIN_HOLD_S straight, not just cumulatively.
    pending: Option<(KeyEstimate, f32)>,
    // `cusum_s` mode: each relative-major class's accumulated evidence (the leader is 0, the
    // others sit between `-cusum_s` and 0), the class shown, and when the evidence was last added.
    scores: [f32; 12],
    leader: Option<usize>,
    last_eval_s: Option<f32>,
}

impl KeyEstimator {
    pub fn new() -> Self {
        Self::with_params(KeyParams::standard())
    }

    pub fn with_params(params: KeyParams) -> Self {
        Self {
            params,
            history: VecDeque::new(),
            now_s: 0.0,
            displayed: None,
            pending: None,
            scores: [0.0; 12],
            leader: None,
            last_eval_s: None,
        }
    }

    /// The key chord naming leans on (to guess a bare fifth's 3rd): the shown key, except that
    /// with `cusum_s` and `doubt` a challenger that has gained `doubt` of what it needs to take
    /// over is believed already. Naming a chord wrongly for the seconds a modulation takes to
    /// register costs more than a key readout that moves a little early would, and the readout
    /// itself keeps waiting for the full `cusum_s`.
    pub fn naming_key(&self) -> Option<KeyEstimate> {
        let (lambda, doubt) = (self.params.cusum_s, self.params.doubt);
        if lambda <= 0.0 || doubt <= 0.0 {
            return self.displayed;
        }
        let leader = self.leader?;
        let (challenger, score) = (0..12)
            .filter(|&c| c != leader)
            .map(|c| (c, self.scores[c]))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())?;
        let class = if score > -doubt * lambda { challenger } else { leader };
        Some(KeyEstimate { tonic_pc: class as i32, mode: Mode::Major })
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
        self.now_s = now_s;
        let window_s = self.params.window_s;
        self.history.retain(|n| now_s - n.start_s <= window_s);
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
        if self.params.cusum_s > 0.0 {
            return self.estimate_cusum();
        }
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
                if best_score - cur_score >= self.params.switch_margin {
                    match self.pending {
                        Some((cand, since)) if cand == best_key && now_s - since >= self.params.min_hold_s => {
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

    /// Tracks the key as a change detector (Page's cumulative sum) over the 12 relative-major
    /// classes -- a class is a major key or the minor key three semitones below, which share a
    /// scale and so name chords the same. Each moment every class gains its correlation with the
    /// window's chroma relative to the mean over classes, times the time since the last moment,
    /// but never falls more than `cusum_s` behind the leading class: a class that keeps being
    /// the better fit overtakes the leader after gaining `cusum_s`, however far back it started,
    /// while a few ambiguous seconds gain nothing. That follows a modulation sooner than a long
    /// window can and flickers less than a short one.
    fn estimate_cusum(&mut self) -> Option<KeyEstimate> {
        if self.history.len() < MIN_NOTES_FOR_ESTIMATE {
            self.last_eval_s = None;
            return None;
        }
        let chroma = self.chroma();
        let mut fit = [0f32; 12];
        for (class, slot) in fit.iter_mut().enumerate() {
            let major = pearson(&chroma, &rotated(&BELLMAN_BUDGE_MAJOR, class as i32));
            let minor = pearson(&chroma, &rotated(&BELLMAN_BUDGE_MINOR, (class as i32 + 9) % 12)) - self.params.minor_penalty;
            *slot = major.max(minor);
        }
        let mean = fit.iter().sum::<f32>() / 12.0;
        let dt = self.last_eval_s.map_or(1.0, |t| (self.now_s - t).clamp(0.0, 1.0));
        self.last_eval_s = Some(self.now_s);
        let top = self.scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for (score, fit) in self.scores.iter_mut().zip(fit) {
            *score = score.max(top - self.params.cusum_s) + (fit - mean) * dt;
        }
        let top = self.scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for score in &mut self.scores {
            *score -= top;
        }
        let best = (0..12).max_by(|&a, &b| self.scores[a].partial_cmp(&self.scores[b]).unwrap()).unwrap();
        let leader = match self.leader {
            Some(cur) if self.scores[best] <= self.scores[cur] + 1e-6 => cur,
            _ => best,
        };
        self.leader = Some(leader);
        self.displayed = Some(KeyEstimate { tonic_pc: leader as i32, mode: Mode::Major });
        self.displayed
    }

    fn chroma(&self) -> [f32; 12] {
        let mut bins = [0f32; 12];
        let decay = self.params.decay_s;
        for n in &self.history {
            let age_weight = if decay > 0.0 { (-(self.now_s - n.start_s).max(0.0) / decay).exp() } else { 1.0 };
            bins[n.pc as usize] += n.weight * age_weight;
        }
        bins
    }

    fn correlation_for(&self, key: KeyEstimate) -> f32 {
        let profile = match key.mode {
            Mode::Major => &BELLMAN_BUDGE_MAJOR,
            Mode::Minor => &BELLMAN_BUDGE_MINOR,
        };
        pearson(&self.chroma(), &rotated(profile, key.tonic_pc)) - self.mode_penalty(key.mode)
    }

    fn mode_penalty(&self, mode: Mode) -> f32 {
        if mode == Mode::Minor {
            self.params.minor_penalty
        } else {
            0.0
        }
    }

    fn best_correlated_key(&self) -> Option<(KeyEstimate, f32)> {
        if self.history.len() < MIN_NOTES_FOR_ESTIMATE {
            return None;
        }
        let chroma = self.chroma();
        let mut best: Option<(KeyEstimate, f32)> = None;
        for tonic in 0..12i32 {
            for (mode, profile) in [(Mode::Major, &BELLMAN_BUDGE_MAJOR), (Mode::Minor, &BELLMAN_BUDGE_MINOR)] {
                let score = pearson(&chroma, &rotated(profile, tonic)) - self.mode_penalty(mode);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harmonica_model::Part;

    /// Four one-second chords on `tonic` (I, IV, V, vi of the major scale) repeated for `seconds`
    /// from `from_s`, fed to the estimator as each note ends and estimated every 0.1 s. Returns
    /// (time, shown key's relative-major pc, naming key's) for every estimate.
    fn play(est: &mut KeyEstimator, tonic: i32, from_s: f32, seconds: f32) -> Vec<(f32, i32, i32)> {
        let degrees: [[i32; 3]; 4] = [[0, 4, 7], [5, 9, 0], [7, 11, 2], [9, 0, 4]];
        let mut out = Vec::new();
        let mut pending: Vec<ConfirmedNote> = Vec::new();
        let mut k = 0;
        while k as f32 * 0.1 < seconds {
            let t = from_s + k as f32 * 0.1;
            if k % 10 == 0 {
                let chord = degrees[(k / 10) as usize % 4];
                for pc in chord {
                    pending.push(ConfirmedNote { start_s: t, end_s: t + 1.0, pitch_midi: 48 + (tonic + pc) % 12, part: Part::Other, velocity: 0.8 });
                }
            }
            let (ended, rest): (Vec<_>, Vec<_>) = pending.drain(..).partition(|n| n.end_s <= t);
            pending = rest;
            for n in &ended {
                est.observe(n);
            }
            if let Some(key) = est.estimate(t) {
                out.push((t, key.relative_major_pc(), est.naming_key().unwrap().relative_major_pc()));
            }
            k += 1;
        }
        out
    }

    #[test]
    fn follows_a_modulation_without_flicker_and_names_with_the_challenger_first() {
        let mut est = KeyEstimator::with_params(KeyParams::default());
        let first = play(&mut est, 0, 0.0, 30.0);
        assert!(first.last().is_some_and(|&(_, shown, _)| shown == 0), "C major settles as C");
        let second = play(&mut est, 9, 30.0, 30.0);
        let shown_at = |pc: i32| second.iter().find(|&&(_, shown, _)| shown == pc).map(|&(t, ..)| t);
        let named_at = |pc: i32| second.iter().find(|&&(_, _, named)| named == pc).map(|&(t, ..)| t);
        let (shown, named) = (shown_at(9).expect("A major is reached"), named_at(9).expect("naming reaches A major"));
        assert!(shown - 30.0 < 10.0, "A major takes over within 10 s of the modulation, took {}", shown - 30.0);
        assert!(named <= shown, "naming does not lag the readout ({named} vs {shown})");
        let switches = first.iter().chain(&second).collect::<Vec<_>>().windows(2).filter(|w| w[0].1 != w[1].1).count();
        assert!(switches <= 2, "the key changes about once, not {switches} times");
    }

    #[test]
    fn the_original_hysteresis_is_still_there_without_the_cumulative_rule() {
        let params = KeyParams { cusum_s: 0.0, doubt: 0.0, minor_penalty: 0.0, window_s: 12.0, decay_s: 0.0, ..KeyParams::default() };
        let mut est = KeyEstimator::with_params(params);
        let first = play(&mut est, 0, 0.0, 30.0);
        assert!(first.last().is_some_and(|&(_, shown, _)| shown == 0));
        let second = play(&mut est, 9, 30.0, 30.0);
        assert!(second.last().is_some_and(|&(_, shown, _)| shown == 9));
    }
}
