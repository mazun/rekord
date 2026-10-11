//! The layer that names the chord from the notes of a window ending at the checkpoint and shows a
//! different chord only once it has led for a while (the "hold"): the instant layer and the
//! settled (~0.5 s) layer are this one with different numbers.
//!
//! The chord is named by `match_chroma_biased` over a window of the last `window_s` seconds
//! (weighted towards the newest sound if `decay_s > 0`): a passing tone or a half-released chord
//! no longer flips the name, and a real change wins within about half a window. A new chord must
//! lead continuously for `hold_s` before it replaces the shown one, and its start is moved back by
//! `backdate_s` (the time it needs to fill the window); the shown chord also keeps the win unless
//! another beats its score by more than `margin`.

use super::evidence::{evaluate_chord, snap_to_beat_grid};
use super::{ChordAt, ChordLayer, LayerChange, StepCtx};

/// Every number here is this project's own (no published prescription), found by sweeping on
/// RWC-Pop (docs/chord-layers.md: odd songs to tune, even songs to check; the settled layer's
/// numbers are from the earlier sweep in CLAUDE.md, "Chord naming").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoldParams {
    /// Notes of `[t - window_s, t]` name the chord.
    pub window_s: f32,
    /// With a decay time, sound in the window counts less the older it is (weight exp(-age /
    /// decay_s)), so a longer window can be used without the old chord holding on; 0 = flat.
    pub decay_s: f32,
    /// A different chord must lead this long (0 = switch at once) before it replaces the shown one.
    pub hold_s: f32,
    /// The same, when only the slash bass differs (same root and quality); never less than `hold_s`.
    pub bass_hold_s: f32,
    /// A new chord's start is moved back by this (the time it needs to fill the window).
    pub backdate_s: f32,
    /// The shown chord keeps the win unless another beats its score by more than this.
    pub margin: f32,
    /// Silence is only called after no chord note sounded for this long.
    pub silence_hold_s: f32,
    /// How much of the chord vocabulary the layer names: 0 = all of it, 1 = without slash basses,
    /// 2 = also only the triad (7ths, 6ths and 9ths shown as their triad).
    pub coarse: u8,
    /// Snap a new chord's start to the beat grid, when one is known.
    pub snap: bool,
}

impl HoldParams {
    /// The instant layer: names a chord about 0.15 s after the notes show it (against 0.55 s for
    /// the settled layer), slash chords included. That is the user's choice from trying it in the
    /// live app (2026-10-09): the first design left the slash bass out (`c = 1`) and held a new
    /// chord for 0.1 s (a delay of 0.21 s) to keep the lane steady. With the slash and a 0.05 s
    /// hold the bass flipping between the notes of one chord made 71 corrections a minute on
    /// RWC-Pop (majmin_inv .671), so a change of the bass alone has to lead for 0.4 s: 19 a
    /// minute, majmin_inv .739, the same as without the slash (docs/chord-layers.md, section 11).
    /// The chord itself is still named after 0.15 s; the slash follows 0.4 s later.
    pub fn instant() -> Self {
        Self {
            window_s: 0.4,
            decay_s: 0.1,
            hold_s: 0.05,
            bass_hold_s: 0.4,
            backdate_s: 0.1,
            margin: 0.4,
            silence_hold_s: 0.25,
            coarse: 0,
            snap: false,
        }
    }

    /// The settled layer: the chord naming that was the only one before the layers (0.5 s window,
    /// 0.25 s hold, 0.3 s backdate: an effective delay of 0.55 s), snapped to the beat grid. With
    /// `margin = 0` it is exactly that naming (checked on 100 RWC-Pop songs: the same 20990 chords
    /// at the same times). The margin of 0.2 is the one change: the shown chord keeps the win
    /// unless another beats it by that much, which on RWC-Pop raises majmin .770 -> .778 and
    /// tetrads .588 -> .597 at the same delay (docs/chord-layers.md, section 10). Longer or
    /// decaying windows, other holds and other backdates were not better.
    pub fn settled() -> Self {
        Self {
            window_s: 0.5,
            decay_s: 0.0,
            hold_s: 0.25,
            bass_hold_s: 0.25,
            backdate_s: 0.3,
            margin: 0.2,
            silence_hold_s: 0.5,
            coarse: 0,
            snap: true,
        }
    }

    /// Overrides from a comma-separated `key=value` list (the keys of the replay tool's
    /// `REKORD_CHORD_TUNE`): `w` window, `d` decay, `h` hold, `b` backdate, `m` margin, `sil`
    /// silence hold, `c` coarse level, `snap` (0/1). An empty string changes nothing.
    pub fn apply_tune(&mut self, tune: &str) -> Result<(), String> {
        for kv in tune.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| format!("expected key=value, got {kv:?}"))?;
            let num = || {
                v.parse::<f32>()
                    .map_err(|_| format!("{k} needs a number, got {v:?}"))
            };
            match k {
                "w" => self.window_s = num()?,
                "d" => self.decay_s = num()?,
                "h" => self.hold_s = num()?,
                "bh" => self.bass_hold_s = num()?,
                "b" => self.backdate_s = num()?,
                "m" => self.margin = num()?,
                "sil" => self.silence_hold_s = num()?,
                "c" => self.coarse = num()? as u8,
                "snap" => self.snap = num()? != 0.0,
                _ => return Err(format!("unknown key {k:?}")),
            }
        }
        Ok(())
    }
}

/// The triad (or sus/power chord) a chord quality is built on.
fn triad_of(quality: &'static str) -> &'static str {
    match quality {
        "6" | "7" | "maj7" | "add9" | "9" | "maj9" | "11" | "13" | "maj13" => "maj",
        "m6" | "m7" | "madd9" | "m9" | "m11" => "m",
        "m7b5" | "dim7" => "dim",
        "sus4(7)" => "sus4",
        other => other,
    }
}

fn coarsen(chord: ChordAt, level: u8) -> ChordAt {
    match chord {
        ChordAt::Match(mut m) => {
            if level >= 1 {
                m.bass_pc = None;
            }
            if level >= 2 {
                m.quality = triad_of(m.quality);
            }
            ChordAt::Match(m)
        }
        silence => silence,
    }
}

pub struct HoldLayer {
    params: HoldParams,
    displayed: Option<ChordAt>,
    /// The (root, quality) the match actually named for `displayed`, before it was coarsened:
    /// the margin favors that template.
    displayed_raw: Option<(i32, &'static str)>,
    /// The challenger to `displayed` and when it first led (reset when the leader changes).
    pending: Option<(ChordAt, f32)>,
    /// The latest change time and the latest `decided_s` reported, which no later change may precede.
    last_change_t: f32,
    decided_s: f32,
}

impl HoldLayer {
    pub fn new(params: HoldParams) -> Self {
        Self {
            params,
            displayed: None,
            displayed_raw: None,
            pending: None,
            last_change_t: f32::NEG_INFINITY,
            decided_s: f32::NEG_INFINITY,
        }
    }

    fn commit(
        &mut self,
        chord: ChordAt,
        raw: Option<(i32, &'static str)>,
        start: f32,
        ctx: &StepCtx,
        out: &mut Vec<LayerChange>,
    ) {
        let start = if self.params.snap {
            snap_to_beat_grid(start, ctx.beat_grid)
        } else {
            start
        };
        let t = start.max(self.last_change_t).max(self.decided_s);
        out.push(LayerChange { t, chord });
        self.displayed = Some(chord);
        self.displayed_raw = raw;
        self.pending = None;
        self.last_change_t = t;
    }
}

impl ChordLayer for HoldLayer {
    fn step(&mut self, t: f32, ctx: &StepCtx, out: &mut Vec<LayerChange>) -> f32 {
        let p = self.params;
        let named = evaluate_chord(
            ctx.notes,
            t,
            ctx.key.as_ref(),
            p.window_s,
            p.decay_s,
            p.silence_hold_s,
            self.displayed_raw,
            p.margin,
        );
        let raw = match named {
            Some(ChordAt::Match(m)) => Some((m.root_pc, m.quality)),
            _ => None,
        };
        let candidate = named.map(|c| coarsen(c, p.coarse));

        // A chord's notes don't release simultaneously -- confirmed via real-world testing
        // (2026-09-04): as a chord decays, an inner voice (the 3rd) commonly fades out first,
        // briefly leaving just root+5th audible before the rest follows. That bare dyad is
        // definitionally the tail of the *same* chord, not a new one, so (unlike an ordinary
        // candidate) no amount of hold-time should ever let it overwrite the display. Only guards
        // a same-root downgrade to "5"; a different root (or an actual power chord struck from
        // silence, where nothing is displayed yet) is unaffected.
        let is_decay_to_bare_fifth = matches!(
            (self.displayed, candidate),
            (Some(ChordAt::Match(cur)), Some(ChordAt::Match(cand)))
                if cand.quality == "5" && cand.root_pc == cur.root_pc && cur.quality != "5"
        );
        if let Some(candidate) = candidate {
            if !is_decay_to_bare_fifth {
                match self.displayed {
                    None => self.commit(candidate, raw, t, ctx, out),
                    Some(cur) if cur == candidate => self.pending = None,
                    Some(cur) => {
                        // Backdated to `since`, not `t`: `since` is the earliest checkpoint at
                        // which this exact candidate already won every re-evaluation, i.e. the
                        // earliest time consistent with the notes actually sounding -- the hold is
                        // a decision gate against melodic flicker, not a claim about when the
                        // chord itself began.
                        let since = match self.pending {
                            Some((cand, since)) if cand == candidate => since,
                            _ => t,
                        };
                        self.pending = Some((candidate, since));
                        let bass_only = matches!(
                            (cur, candidate),
                            (ChordAt::Match(a), ChordAt::Match(b)) if a.root_pc == b.root_pc && a.quality == b.quality
                        );
                        let hold = if bass_only {
                            p.bass_hold_s.max(p.hold_s)
                        } else {
                            p.hold_s
                        };
                        if t - since >= hold {
                            self.commit(candidate, raw, since - p.backdate_s, ctx, out);
                        }
                    }
                }
            }
        }
        // The earliest a change can still start: the first chord starts at the checkpoint that
        // names it (nothing is shown before, so there is nothing to backdate); after that, a
        // challenger that is waiting to hold long enough starts at its `since`, any other at a
        // later checkpoint -- minus the backdate either way, but never before the latest change.
        let earliest = if self.displayed.is_none() {
            t
        } else {
            (self.pending.map_or(t, |(_, since)| since.min(t)) - p.backdate_s)
                .max(self.last_change_t)
        };
        self.decided_s = self.decided_s.max(earliest);
        self.decided_s
    }

    fn finish(&mut self, covered: f32, _out: &mut Vec<LayerChange>) {
        // A challenger that has not held long enough is dropped, as the stabilizer always did.
        self.pending = None;
        self.decided_s = self.decided_s.max(covered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord_templates::ChordMatch;
    use crate::harmonica_decode::NoteEvent;
    use crate::harmonica_model::Part;

    /// A block chord of `pitches` over `[start, end)`, one note per pitch.
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

    /// Runs the layer over C (0-2 s), Am (2-4 s), F (4-6 s) at the 50 ms checkpoint grid, the way
    /// the engine does, returning (checkpoint, change) pairs and checking the layer's contract.
    fn run(params: HoldParams) -> Vec<(f32, LayerChange)> {
        let mut notes = chord(0.0, 2.0, &[48, 52, 55, 60]);
        notes.extend(chord(2.0, 4.0, &[45, 52, 57, 60]));
        notes.extend(chord(4.0, 6.0, &[41, 53, 57, 60]));
        run_notes(params, &notes, 6.0)
    }

    fn run_notes(params: HoldParams, notes: &[NoteEvent], end_s: f32) -> Vec<(f32, LayerChange)> {
        let mut layer = HoldLayer::new(params);
        let mut announced = Vec::new();
        let mut decided = f32::NEG_INFINITY;
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
                    key: None,
                    beat_grid: None,
                },
                &mut out,
            );
            assert!(new_decided >= decided, "decided_s went back at {t}");
            for c in &out {
                assert!(
                    c.t >= decided - 1e-4,
                    "change at {} precedes the previous decided_s {decided}",
                    c.t
                );
                assert!(
                    c.t <= new_decided + 1e-4,
                    "change at {} is past decided_s {new_decided}",
                    c.t
                );
                announced.push((t, *c));
            }
            decided = new_decided;
            k += 1;
        }
        announced
    }

    #[test]
    fn names_a_simple_progression_soon_after_each_change() {
        let changes = run(HoldParams::instant());
        let named: Vec<_> = changes.iter().map(|(_, c)| name(c.chord)).collect();
        assert_eq!(named, vec![(0, "maj"), (9, "m"), (5, "maj")], "{named:?}");
        // each is announced well within 0.5 s of the notes starting
        for ((announced_at, c), onset) in changes.iter().zip([0.0f32, 2.0, 4.0]) {
            assert!(
                *announced_at - onset < 0.5,
                "{} announced at {announced_at}",
                name(c.chord).1
            );
        }
    }

    #[test]
    fn the_settled_layer_is_slower_but_names_the_same_chords() {
        let instant = run(HoldParams::instant());
        let settled = run(HoldParams::settled());
        let names =
            |v: &[(f32, LayerChange)]| v.iter().map(|(_, c)| name(c.chord)).collect::<Vec<_>>();
        assert_eq!(names(&instant), names(&settled));
        // (the first chord is named at the first checkpoint by both: nothing is shown before it)
        for ((at_instant, _), (at_settled, _)) in instant.iter().zip(&settled).skip(1) {
            assert!(at_settled > at_instant);
        }
    }

    #[test]
    fn a_change_is_never_announced_before_it_can_be_seen() {
        for params in [HoldParams::instant(), HoldParams::settled()] {
            for (announced_at, c) in run(params) {
                assert!(
                    c.t <= announced_at,
                    "change at {} announced early at {announced_at}",
                    c.t
                );
            }
        }
    }

    #[test]
    fn a_change_of_the_bass_alone_waits_longer_than_a_change_of_chord() {
        // C major above throughout; the bass part plays C for 0-2 s, then E (C/E) for 2-4 s.
        let mut notes = chord(0.0, 4.0, &[60, 64, 67]);
        notes.push((0.0, 2.0, 36, 0.8, Part::Bass));
        notes.push((2.0, 4.0, 40, 0.8, Part::Bass));
        let slash_announced = |bass_hold_s: f32| {
            let changes = run_notes(
                HoldParams {
                    bass_hold_s,
                    ..HoldParams::instant()
                },
                &notes,
                4.0,
            );
            changes
                .iter()
                .find(|(_, c)| matches!(c.chord, ChordAt::Match(m) if m.bass_pc == Some(4)))
                .map(|&(at, _)| at)
        };
        let (quick, steady) = (
            slash_announced(0.05).expect("C/E is named"),
            slash_announced(0.4).expect("C/E is named"),
        );
        assert!(
            steady - quick > 0.25,
            "a 0.4 s bass hold announces C/E at {steady}, a 0.05 s one at {quick}"
        );
        assert!(
            steady < 2.0 + 0.4 + 0.5,
            "but still within about a second of the bass note: {steady}"
        );
    }

    #[test]
    fn tune_overrides_and_rejects_nonsense() {
        let mut p = HoldParams::instant();
        p.apply_tune("h=0.15, c=1,snap=1").unwrap();
        assert_eq!((p.hold_s, p.coarse, p.snap), (0.15, 1, true));
        assert!(p.apply_tune("x=1").is_err());
        assert!(p.apply_tune("h=fast").is_err());
        assert!(p.apply_tune("").is_ok());
    }
}
