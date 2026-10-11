//! The layer that waits about 2 s: it decides each moment's chord by looking at what comes after
//! it as well, with a fixed-lag Viterbi search over the whole chord vocabulary.
//!
//! Every `hop_s` the notes of a window around that moment score every chord template at every
//! root (`score_templates`, the numbers the other layers pick the best of) and, for each, every
//! bass note. The search then finds the sequence of states that explains the scores best when
//! changing the chord costs `switch` (and changing only the bass under a chord costs `bass_switch`).
//! A passing tone, a melody note that makes a 9th for a moment, or a chord whose 3rd has not
//! sounded yet no longer takes a chord's place for long, because the notes that follow it outweigh
//! the cost of the detour. A moment is decided `delay_s` after `covered` passed it, with the path
//! that is best at that point; the layer needs the notes of a few seconds around it only.
//!
//! The 3rd of a chord that has none in the notes (a bare fifth) comes from what the song has shown
//! (the 3rd that sounded under the chord on that root before) and from the key, as in the other
//! layers -- but as a tie-break only (`key_lift`): the notes decide, and the stale key of a
//! modulation does not spread over a whole chord (docs/chord-layers.md, section 12).

use std::collections::VecDeque;

use super::evidence::{snap_to_beat_grid, window_chroma};
use super::{ChordAt, ChordLayer, LayerChange, StepCtx};
use crate::beat_tracker::BeatGrid;
use crate::chord_templates::{
    bare_fifth_triad, score_templates, ChordMatch, CHORD_TEMPLATES, TEMPLATE_COUNT,
};
use crate::harmonica_model::Part;

/// The groups of states: every (root, template), then silence. Each has 12 bass notes.
const CHORDS: usize = 12 * TEMPLATE_COUNT;
const SILENCE: usize = CHORDS;
const GROUPS: usize = CHORDS + 1;
const STATES: usize = GROUPS * 12;

/// A score no state with it can win: for the silence in a bin with notes and the chords in a bin
/// without. (Large against the scores and penalties, small enough not to lose f32 precision.)
const IMPOSSIBLE: f32 = -100.0;

/// A 3rd at least this share of a window counts as really sounding (the other layers' threshold
/// for naming a bare fifth after the key).
const STRONG_THIRD_SHARE: f32 = 0.12;
/// A 3rd this strong in a bin counts as heard under the chord decided there (for the memory).
const THIRD_HEARD_SHARE: f32 = 0.1;
/// The memory names a bare fifth's 3rd only with this many seconds of evidence on its root, of
/// which this share is for one kind.
const MEMORY_MIN_S: f32 = 1.0;
const MEMORY_MAJORITY: f32 = 0.7;

/// Where a bare fifth gets its 3rd from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming {
    /// Nowhere: it stays a power chord.
    Off,
    /// The key's diatonic triad.
    Key,
    /// The 3rd this song has mostly shown under that root, else the key's.
    KeyAndMemory,
}

/// Every number here is this project's own, like the other layers' (docs/chord-layers.md).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfirmedParams {
    /// The search moves in steps of this many seconds.
    pub hop_s: f32,
    /// A moment is scored from the notes of this many seconds around it ...
    pub window_s: f32,
    /// ... of which this share (0..1) lies after it.
    pub forward: f32,
    /// A moment is decided this long after `covered` passed it.
    pub delay_s: f32,
    /// The cost of changing to another chord.
    pub switch: f32,
    /// The cost of changing the bass only (same root and quality): C -> C/E.
    pub bass_switch: f32,
    /// The bass the state names earns this much per unit of the bass part's share on it (the
    /// root's own share is also in the chord's score) ...
    pub bass_weight: f32,
    /// ... and a bass that is not the root costs this, for naming a slash chord at all.
    pub slash_cost: f32,
    /// Silence is called where no chord note sounds within this many seconds around a moment.
    pub silence_s: f32,
    pub naming: Naming,
    /// The key's triad stands in for a bare fifth in a bin's score at most this much above the
    /// triad's own score: the key only breaks a tie between major and minor.
    pub key_lift: f32,
    /// The memory of which 3rd a root had fades with this time constant.
    pub memory_s: f32,
    /// Snap a new chord's start to the beat grid, when one is known.
    pub snap: bool,
}

impl ConfirmedParams {
    pub fn standard() -> Self {
        Self {
            hop_s: 0.1,
            window_s: 0.4,
            forward: 0.3,
            delay_s: 2.0,
            switch: 2.5,
            bass_switch: 3.0,
            bass_weight: 0.2,
            slash_cost: 0.05,
            silence_s: 0.5,
            naming: Naming::KeyAndMemory,
            key_lift: 0.03,
            memory_s: 60.0,
            snap: true,
        }
    }

    /// Overrides from a comma-separated `key=value` list: `hop`, `w` window, `f` forward share,
    /// `D` delay, `p` switch, `pb` bass switch, `g` bass weight, `k` slash cost, `sil` silence,
    /// `name` (0 off, 1 key, 2 key and memory), `kl` key lift, `mem` memory time constant and
    /// `snap` (0/1). An empty string changes nothing.
    pub fn apply_tune(&mut self, tune: &str) -> Result<(), String> {
        let mut next = *self;
        for kv in tune.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| format!("expected key=value, got {kv:?}"))?;
            let num = || {
                v.parse::<f32>()
                    .map_err(|_| format!("{k} needs a number, got {v:?}"))
            };
            match k {
                "hop" => next.hop_s = num()?,
                "w" => next.window_s = num()?,
                "f" => next.forward = num()?,
                "D" => next.delay_s = num()?,
                "p" => next.switch = num()?,
                "pb" => next.bass_switch = num()?,
                "g" => next.bass_weight = num()?,
                "k" => next.slash_cost = num()?,
                "sil" => next.silence_s = num()?,
                "name" => {
                    next.naming = match num()? as i32 {
                        0 => Naming::Off,
                        1 => Naming::Key,
                        2 => Naming::KeyAndMemory,
                        other => return Err(format!("name is 0..2, got {other}")),
                    }
                }
                "kl" => next.key_lift = num()?,
                "mem" => next.memory_s = num()?,
                "snap" => next.snap = num()? != 0.0,
                _ => return Err(format!("unknown key {k:?}")),
            }
        }
        if !(next.hop_s > 0.0)
            || !(0.0..=1.0).contains(&next.forward)
            || !(next.window_s > 0.0)
            || !(next.memory_s > 0.0)
        {
            return Err("hop, w and mem must be positive, f within 0..1".into());
        }
        *self = next;
        Ok(())
    }
}

/// What the notes around one moment say.
struct Bin {
    /// The score of each (root, template); with the silence, all but one of these is `IMPOSSIBLE`.
    chord: [f32; CHORDS],
    silence: f32,
    /// The bass part's share of each pitch class (zero where there is too little bass to name).
    bass: [f32; 12],
    /// Each pitch class's share of the window (for the memory of thirds).
    share: [f32; 12],
}

/// How the best path into each state of a bin came about: the state it kept, or which
/// better-placed state it switched from.
struct Row {
    /// Per state: 0 = the same state in the bin before, 1 = the best bass of the same chord,
    /// 2 = the best state of all.
    back: Vec<u8>,
    /// Of the bin before: per chord the best bass, and the best state.
    bass_arg: Vec<u8>,
    global_arg: u16,
    share: [f32; 12],
}

pub struct ConfirmedLayer {
    params: ConfirmedParams,
    /// The score of the best path ending in each state, up to the newest bin (best = 0).
    dp: Vec<f32>,
    /// The bins not decided yet: `rows[i]` is bin `front + i`.
    rows: VecDeque<Row>,
    front: i64,
    /// The next bin to score.
    next_bin: i64,
    /// Provisional bins after `next_bin` (their windows reach past the newest checkpoint), kept
    /// from the last step for `finish`, which gets no notes.
    tail: Vec<Bin>,
    grid: Option<BeatGrid>,
    /// The state last reported, when it started, and how far the layer has decided.
    shown: Option<usize>,
    last_change_t: f32,
    decided_s: f32,
    /// Seconds each root was decided with a major / a minor 3rd sounding, fading.
    memory: [[f32; 2]; 12],
    index_5: usize,
    index_maj: usize,
    index_min: usize,
}

fn position(quality: &str) -> usize {
    CHORD_TEMPLATES
        .iter()
        .position(|t| t.quality == quality)
        .expect("the template exists")
}

/// Whether a quality has a major (0) or a minor (1) 3rd, if it has one.
fn third_of(quality: &str) -> Option<usize> {
    match quality {
        "maj" | "6" | "7" | "maj7" | "add9" | "9" | "maj9" | "11" | "13" | "maj13" | "aug" => {
            Some(0)
        }
        "m" | "m6" | "m7" | "m7b5" | "madd9" | "m9" | "m11" | "dim" | "dim7" => Some(1),
        _ => None,
    }
}

impl ConfirmedLayer {
    pub fn new(params: ConfirmedParams) -> Self {
        Self {
            params,
            dp: vec![0.0; STATES],
            rows: VecDeque::new(),
            front: 0,
            next_bin: 0,
            tail: Vec::new(),
            grid: None,
            shown: None,
            last_change_t: f32::NEG_INFINITY,
            decided_s: f32::NEG_INFINITY,
            memory: [[0.0; 2]; 12],
            index_5: position("5"),
            index_maj: position("maj"),
            index_min: position("m"),
        }
    }

    /// How far past a moment the layer has to see before it can score it.
    fn ahead(&self) -> f32 {
        (self.params.window_s * self.params.forward).max(self.params.silence_s / 2.0)
    }

    /// How many bins the search looks ahead of the one it decides.
    fn lag_bins(&self) -> usize {
        ((self.params.delay_s - self.ahead()) / self.params.hop_s)
            .round()
            .max(0.0) as usize
    }

    /// The 3rd this song has mostly shown under the chord on `root` (`"maj"` / `"m"`), if enough.
    fn remembered_third(&self, root: usize) -> Option<&'static str> {
        let [major, minor] = self.memory[root];
        let total = major + minor;
        (total >= MEMORY_MIN_S && major.max(minor) >= MEMORY_MAJORITY * total).then_some({
            if major >= minor {
                "maj"
            } else {
                "m"
            }
        })
    }

    /// The bin at moment `g`, from the notes up to `t` (the newest checkpoint; the window is cut there).
    fn score_bin(&self, ctx: &StepCtx, g: f32, t: f32) -> Bin {
        let p = &self.params;
        let hi = (g + p.window_s * p.forward).min(t);
        let lo = g - p.window_s * (1.0 - p.forward);
        let (near_lo, near_hi) = (g - p.silence_s / 2.0, (g + p.silence_s / 2.0).min(t));
        let heard = ctx.notes.iter().any(|&(s, e, _p, _a, part)| {
            part != Part::Vocal && s <= near_hi + 1e-6 && e >= near_lo
        });
        let mut bin = Bin {
            chord: [IMPOSSIBLE; CHORDS],
            silence: IMPOSSIBLE,
            bass: [0.0; 12],
            share: [0.0; 12],
        };
        if !heard {
            bin.silence = 0.0;
            return bin;
        }
        let (all, bass) = window_chroma(ctx.notes, hi, (hi - lo).max(1e-3), 0.0);
        let Some(mut scored) = score_templates(&all, &bass) else {
            // Notes sound, but not enough to say a chord: no evidence either way.
            bin.chord = [0.0; CHORDS];
            return bin;
        };
        if p.naming != Naming::Off {
            for root in 0..12 {
                if scored.share[(root + 3) % 12].max(scored.share[(root + 4) % 12])
                    >= STRONG_THIRD_SHARE
                {
                    continue;
                }
                let remembered = if p.naming == Naming::KeyAndMemory {
                    self.remembered_third(root)
                } else {
                    None
                };
                let quality = remembered.or_else(|| {
                    ctx.key
                        .as_ref()
                        .and_then(|key| bare_fifth_triad(root, &scored.share, key))
                });
                if let Some(quality) = quality {
                    let triad = root * TEMPLATE_COUNT
                        + if quality == "maj" {
                            self.index_maj
                        } else {
                            self.index_min
                        };
                    let fifth = root * TEMPLATE_COUNT + self.index_5;
                    scored.scores[triad] = scored.scores[triad]
                        .max(scored.scores[fifth].min(scored.scores[triad] + p.key_lift));
                    scored.scores[fifth] = IMPOSSIBLE;
                }
            }
        }
        bin.chord = scored.scores;
        bin.share = scored.share;
        if scored.bass_pc.is_some() {
            bin.bass = scored.bass_share;
        }
        bin
    }

    /// Adds a bin to the search.
    fn push(&mut self, bin: &Bin) {
        let p = self.params;
        // The best of the bins before, by chord and overall.
        let mut bass_arg = vec![0u8; GROUPS];
        let mut group_best = [f32::NEG_INFINITY; GROUPS];
        let mut global = (f32::NEG_INFINITY, 0u16);
        for g in 0..GROUPS {
            for b in 0..12 {
                let v = self.dp[g * 12 + b];
                if v > group_best[g] {
                    group_best[g] = v;
                    bass_arg[g] = b as u8;
                }
            }
            if group_best[g] > global.0 {
                global = (group_best[g], (g * 12 + bass_arg[g] as usize) as u16);
            }
        }

        let mut back = vec![0u8; STATES];
        let mut next = vec![0.0f32; STATES];
        let mut top = f32::NEG_INFINITY;
        let by_chord = global.0 - p.switch;
        for g in 0..GROUPS {
            let root = g / TEMPLATE_COUNT;
            let by_bass = group_best[g] - p.bass_switch;
            for b in 0..12 {
                let s = g * 12 + b;
                let (mut best, mut how) = (self.dp[s], 0u8);
                for (value, code) in [(by_bass, 1u8), (by_chord, 2)] {
                    if value > best {
                        (best, how) = (value, code);
                    }
                }
                let here = if g == SILENCE {
                    bin.silence + if b == 0 { 0.0 } else { IMPOSSIBLE }
                } else {
                    bin.chord[g] + p.bass_weight * bin.bass[b]
                        - if b == root { 0.0 } else { p.slash_cost }
                };
                back[s] = how;
                next[s] = best + here;
                top = top.max(next[s]);
            }
        }
        for v in &mut next {
            *v -= top;
        }
        self.dp = next;
        self.rows.push_back(Row {
            back,
            bass_arg,
            global_arg: global.1,
            share: bin.share,
        });
    }

    /// The state of the bin `rows[to]` on the best path ending in the newest bin.
    fn trace(&self, to: usize) -> usize {
        let mut state = (0..STATES).fold(
            0,
            |best, s| if self.dp[s] > self.dp[best] { s } else { best },
        );
        for i in (to + 1..self.rows.len()).rev() {
            let row = &self.rows[i];
            state = match row.back[state] {
                0 => state,
                1 => (state / 12) * 12 + row.bass_arg[state / 12] as usize,
                _ => row.global_arg as usize,
            };
        }
        state
    }

    /// Reports the chord of the oldest undecided bin (the state `state`) and drops the bin.
    fn decide(&mut self, state: usize, out: &mut Vec<LayerChange>) {
        let p = self.params;
        let row = self.rows.pop_front().expect("a bin to decide");
        let (g, b) = (state / 12, state % 12);

        // What this song shows about the 3rd of each root: the chord decided with a 3rd sounding.
        let decay = (-p.hop_s / p.memory_s).exp();
        for m in self.memory.iter_mut().flatten() {
            *m *= decay;
        }
        if g < CHORDS {
            let (root, index) = (g / TEMPLATE_COUNT, g % TEMPLATE_COUNT);
            match third_of(CHORD_TEMPLATES[index].quality) {
                Some(0) if row.share[(root + 4) % 12] >= THIRD_HEARD_SHARE => {
                    self.memory[root][0] += p.hop_s
                }
                Some(1) if row.share[(root + 3) % 12] >= THIRD_HEARD_SHARE => {
                    self.memory[root][1] += p.hop_s
                }
                _ => {}
            }
        }

        let bin_start = self.front as f32 * p.hop_s - p.hop_s / 2.0;
        let bin_end = bin_start + p.hop_s;
        if self.shown != Some(state) {
            let chord = if g == SILENCE {
                ChordAt::Silence
            } else {
                let root = (g / TEMPLATE_COUNT) as i32;
                ChordAt::Match(ChordMatch {
                    root_pc: root,
                    quality: CHORD_TEMPLATES[g % TEMPLATE_COUNT].quality,
                    bass_pc: (b as i32 != root).then_some(b as i32),
                })
            };
            let start = if p.snap {
                snap_to_beat_grid(bin_start, self.grid)
            } else {
                bin_start
            };
            let t = start
                .max(self.last_change_t)
                .max(self.decided_s)
                .min(bin_end)
                .max(0.0);
            out.push(LayerChange { t, chord });
            self.shown = Some(state);
            self.last_change_t = t;
        }
        self.decided_s = self.decided_s.max(bin_end);
        self.front += 1;
    }
}

impl ChordLayer for ConfirmedLayer {
    fn step(&mut self, t: f32, ctx: &StepCtx, out: &mut Vec<LayerChange>) -> f32 {
        let p = self.params;
        self.grid = ctx.beat_grid;
        while self.next_bin as f32 * p.hop_s + self.ahead() <= t + 1e-4 {
            let bin = self.score_bin(ctx, self.next_bin as f32 * p.hop_s, t);
            self.push(&bin);
            self.next_bin += 1;
            while self.rows.len() > self.lag_bins() {
                let state = self.trace(0);
                self.decide(state, out);
            }
        }
        // The moments whose windows reach past `t`, scored with what there is so far.
        self.tail.clear();
        let mut bin = self.next_bin;
        while bin as f32 * p.hop_s <= t + 1e-4 {
            let scored = self.score_bin(ctx, bin as f32 * p.hop_s, t);
            self.tail.push(scored);
            bin += 1;
        }
        self.decided_s
    }

    fn finish(&mut self, covered: f32, out: &mut Vec<LayerChange>) {
        for bin in std::mem::take(&mut self.tail) {
            self.push(&bin);
            self.next_bin += 1;
        }
        // Whatever is undecided is decided now, along the best path to the last bin.
        let mut path = Vec::with_capacity(self.rows.len());
        for i in 0..self.rows.len() {
            path.push(self.trace(i));
        }
        for state in path {
            self.decide(state, out);
        }
        self.decided_s = self.decided_s.max(covered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harmonica_decode::NoteEvent;
    use crate::key_estimator::{KeyEstimate, Mode};

    fn chord(start: f32, end: f32, pitches: &[i32]) -> Vec<NoteEvent> {
        pitches
            .iter()
            .map(|&p| (start, end, p, 0.8, Part::Other))
            .collect()
    }

    fn name(c: ChordAt) -> (i32, &'static str) {
        match c {
            ChordAt::Match(ChordMatch {
                root_pc, quality, ..
            }) => (root_pc, quality),
            ChordAt::Silence => (-1, "silence"),
        }
    }

    /// Runs the layer over `notes` at the 50 ms checkpoint grid (what the notes show at each
    /// checkpoint is cut there), checking its contract; returns (checkpoint, change) pairs.
    fn run(params: ConfirmedParams, notes: &[NoteEvent], end_s: f32) -> Vec<(f32, LayerChange)> {
        run_in_key(params, notes, end_s, None)
    }

    fn run_in_key(
        params: ConfirmedParams,
        notes: &[NoteEvent],
        end_s: f32,
        key: Option<KeyEstimate>,
    ) -> Vec<(f32, LayerChange)> {
        let mut layer = ConfirmedLayer::new(params);
        let mut announced = Vec::new();
        let mut decided = f32::NEG_INFINITY;
        let check = |at: f32,
                     out: &[LayerChange],
                     new_decided: f32,
                     decided: &mut f32,
                     announced: &mut Vec<(f32, LayerChange)>| {
            assert!(new_decided >= *decided, "decided_s went back at {at}");
            for c in out {
                assert!(
                    c.t >= *decided - 1e-4,
                    "change at {} precedes the previous decided_s {decided}",
                    c.t
                );
                assert!(
                    c.t <= new_decided + 1e-4,
                    "change at {} is past decided_s {new_decided}",
                    c.t
                );
                announced.push((at, *c));
            }
            *decided = new_decided;
        };
        let mut k = 1;
        loop {
            let t = k as f32 * 0.05;
            if t > end_s {
                break;
            }
            let visible: Vec<NoteEvent> = notes
                .iter()
                .filter(|n| n.0 <= t)
                .map(|&(s, e, p, v, part)| (s, e.min(t), p, v, part))
                .collect();
            let mut out = Vec::new();
            let new_decided = layer.step(
                t,
                &StepCtx {
                    notes: &visible,
                    key,
                    beat_grid: None,
                },
                &mut out,
            );
            check(t, &out, new_decided, &mut decided, &mut announced);
            k += 1;
        }
        let mut out = Vec::new();
        layer.finish(end_s, &mut out);
        check(end_s, &out, end_s, &mut decided, &mut announced);
        announced
    }

    fn progression() -> Vec<NoteEvent> {
        let mut notes = chord(0.0, 4.0, &[48, 52, 55, 60]);
        notes.extend(chord(4.0, 8.0, &[45, 52, 57, 60]));
        notes.extend(chord(8.0, 12.0, &[41, 53, 57, 60]));
        notes
    }

    #[test]
    fn names_a_progression_about_two_seconds_after_each_change() {
        let params = ConfirmedParams {
            snap: false,
            ..ConfirmedParams::standard()
        };
        let changes = run(params, &progression(), 14.0);
        let named: Vec<_> = changes.iter().map(|(_, c)| name(c.chord)).collect();
        assert_eq!(
            named,
            vec![(0, "maj"), (9, "m"), (5, "maj"), (-1, "silence")],
            "{named:?}"
        );
        for ((announced_at, c), onset) in changes.iter().zip([0.0f32, 4.0, 8.0, 12.0]) {
            assert!(
                (c.t - onset).abs() < 0.3,
                "{} starts at {} for an onset at {onset}",
                name(c.chord).1,
                c.t
            );
            assert!(
                *announced_at - onset > 1.5 && *announced_at - onset < 2.8,
                "{} announced at {announced_at}",
                name(c.chord).1
            );
        }
    }

    #[test]
    fn a_passing_note_does_not_make_a_chord_of_its_own() {
        // C major for 6 s, with a D (a 9th) ringing for 0.4 s in the middle: the instant layers
        // would show a Cadd9 for a moment, this one does not.
        let mut notes = chord(0.0, 6.0, &[48, 52, 55]);
        notes.extend(chord(3.0, 3.4, &[62]));
        let changes = run(ConfirmedParams::standard(), &notes, 8.0);
        let named: Vec<_> = changes.iter().map(|(_, c)| name(c.chord)).collect();
        assert_eq!(named, vec![(0, "maj"), (-1, "silence")], "{named:?}");
    }

    #[test]
    fn a_chord_that_only_gains_its_third_late_is_named_by_it_throughout() {
        // A bare fifth on A for 0.8 s, then the full A major to 4 s: the fifth is no chord of its own.
        let mut notes = chord(0.0, 4.0, &[45, 52]);
        notes.extend(chord(0.8, 4.0, &[61]));
        for naming in [Naming::Off, Naming::Key, Naming::KeyAndMemory] {
            let changes = run(
                ConfirmedParams {
                    naming,
                    ..ConfirmedParams::standard()
                },
                &notes,
                6.0,
            );
            let named: Vec<_> = changes.iter().map(|(_, c)| name(c.chord)).collect();
            assert_eq!(
                named,
                vec![(9, "maj"), (-1, "silence")],
                "{naming:?}: {named:?}"
            );
        }
    }

    #[test]
    fn a_slash_bass_is_named_and_a_stray_bass_note_is_not() {
        let mut notes = chord(0.0, 6.0, &[60, 64, 67]);
        notes.push((0.0, 3.0, 36, 0.8, Part::Bass));
        notes.push((3.0, 6.0, 40, 0.8, Part::Bass));
        let changes = run(ConfirmedParams::standard(), &notes, 8.0);
        let basses: Vec<_> = changes
            .iter()
            .filter_map(|(_, c)| match c.chord {
                ChordAt::Match(m) => Some((m.root_pc, m.quality, m.bass_pc)),
                _ => None,
            })
            .collect();
        assert_eq!(
            basses,
            vec![(0, "maj", None), (0, "maj", Some(4))],
            "{basses:?}"
        );
    }

    #[test]
    fn a_bare_fifth_gets_the_key_s_third_or_the_one_the_song_showed() {
        // D major's chord, heard with its F♯ for 4 s, then G for 3 s, then a bare D-A for 4 s. In
        // B♭ major (where D is minor) the key alone names the bare fifth Dm; the memory of the D
        // heard at the start says D.
        let mut notes = chord(0.0, 4.0, &[50, 57, 66]);
        notes.extend(chord(4.0, 7.0, &[43, 55, 59, 62]));
        notes.extend(chord(7.0, 11.0, &[50, 57]));
        let b_flat_major = Some(KeyEstimate {
            tonic_pc: 10,
            mode: Mode::Major,
        });
        let last = |naming| {
            let changes = run_in_key(
                ConfirmedParams {
                    naming,
                    snap: false,
                    ..ConfirmedParams::standard()
                },
                &notes,
                14.0,
                b_flat_major,
            );
            changes
                .iter()
                .filter_map(|(_, c)| match c.chord {
                    ChordAt::Match(m) => Some((m.root_pc, m.quality)),
                    _ => None,
                })
                .next_back()
        };
        assert_eq!(
            last(Naming::Key),
            Some((2, "m")),
            "the key alone names it Dm"
        );
        assert_eq!(
            last(Naming::KeyAndMemory),
            Some((2, "maj")),
            "the memory names it D"
        );
    }

    #[test]
    fn tune_overrides_and_rejects_nonsense() {
        let mut p = ConfirmedParams::standard();
        p.apply_tune("p=4, D=1.5,name=0").unwrap();
        assert_eq!((p.switch, p.delay_s, p.naming), (4.0, 1.5, Naming::Off));
        assert!(p.apply_tune("x=1").is_err());
        assert!(p.apply_tune("p=slow").is_err());
        assert!(p.apply_tune("f=2").is_err());
        assert!(p.apply_tune("name=7").is_err());
        assert!(p.apply_tune("").is_ok());
    }
}
