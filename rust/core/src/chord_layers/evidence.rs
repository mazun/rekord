//! What the chord layers read off the notes: the pitch-class energy of a window ending at a
//! checkpoint (`window_chroma`), the chord it names (`evaluate_chord`), which pitch classes sound
//! (`active_mask`), and when the checkpoints are (`chord_checkpoints`).

use std::collections::BTreeSet;

use super::ChordAt;
use crate::beat_tracker::BeatGrid;
use crate::chord_templates::match_chroma_biased;
use crate::harmonica_decode::NoteEvent;
use crate::harmonica_model::Part;
use crate::key_estimator::KeyEstimate;

// Sub-steps over the window when there is no bass part and the bass is taken as the lowest
// sounding note at each of them.
const BASS_FALLBACK_STEPS: usize = 10;

/// How long each pitch class sounded in `[t - window_s, t]` among the non-vocal notes, and
/// the same for the bass: the bass part's notes if it has any in the window, otherwise (single-part
/// models) the lowest sounding note at evenly spaced moments.
///
/// With `decay_s > 0` a stretch of sound counts its length weighted by `exp(-age / decay_s)`, so
/// the newest sound counts most; with 0 it counts its plain length.
pub(crate) fn window_chroma(note_events: &[NoteEvent], t: f32, window_s: f32, decay_s: f32) -> ([f32; 12], [f32; 12]) {
    let t0 = t - window_s;
    let weight = |lo: f32, hi: f32| {
        if decay_s > 0.0 {
            decay_s * (((hi - t) / decay_s).exp() - ((lo - t) / decay_s).exp())
        } else {
            hi - lo
        }
    };
    let mut all = [0.0f32; 12];
    let mut bass = [0.0f32; 12];
    let mut has_bass_part = false;
    for &(s, e, p, _a, part) in note_events {
        if part == Part::Vocal {
            continue;
        }
        let (lo, hi) = (s.max(t0), e.min(t));
        if hi - lo <= 0.0 {
            continue;
        }
        let overlap = weight(lo, hi);
        let pc = p.rem_euclid(12) as usize;
        all[pc] += overlap;
        if part == Part::Bass {
            bass[pc] += overlap;
            has_bass_part = true;
        }
    }
    if !has_bass_part {
        let step = window_s / BASS_FALLBACK_STEPS as f32;
        for k in 0..BASS_FALLBACK_STEPS {
            let tau = t0 + (k as f32 + 0.5) * step;
            let lowest = note_events
                .iter()
                .filter(|&&(s, e, _p, _a, part)| part != Part::Vocal && s <= tau && e >= tau)
                .map(|&(_s, _e, p, ..)| p)
                .min();
            if let Some(p) = lowest {
                bass[p.rem_euclid(12) as usize] += if decay_s > 0.0 { weight(tau - step / 2.0, tau + step / 2.0) } else { step };
            }
        }
    }
    (all, bass)
}

/// The chord the notes name at time `t`, if anything conclusive can be said: `Some(Silence)` if
/// no chord note has sounded for `silence_hold_s`, `None` if notes are present (or sounded just
/// now) but nothing matched well enough / it's just a brief gap (ambiguous -- the caller should
/// keep what it shows). Note ends are taken as exact: Harmonica's decode closes a note on the
/// frame its sustain drops, so there's no grace period after it (basic-pitch needed 0.2 s for its
/// ragged ends, which with Harmonica only let the previous chord linger into the next as a mixed
/// chord, e.g. Am7 for C -> Am).
///
/// With the Mix model the chord is named from the accompaniment only (bass and other): the sung
/// melody is mostly non-chord tones, so it's kept out of the match (it still shows as a tension
/// over the displayed chord, see `active_mask`). Vocal-only moments therefore read as silence
/// for the chord lane. Single-part models have no vocal part, so they behave as before.
///
/// `prefer`/`margin`: see `match_chroma_biased`.
pub(crate) fn evaluate_chord(
    note_events: &[NoteEvent],
    t: f32,
    key: Option<&KeyEstimate>,
    window_s: f32,
    decay_s: f32,
    silence_hold_s: f32,
    prefer: Option<(i32, &str)>,
    margin: f32,
) -> Option<ChordAt> {
    let sounding = |s: f32, e: f32| s <= t + 1e-6 && e >= t;
    let any_chord_note = note_events.iter().any(|&(s, e, _p, _a, part)| sounding(s, e) && part != Part::Vocal);
    if !any_chord_note {
        let recently_sounding = note_events
            .iter()
            .any(|&(s, e, _p, _a, part)| part != Part::Vocal && s <= t + 1e-6 && e >= t - silence_hold_s);
        return if recently_sounding { None } else { Some(ChordAt::Silence) };
    }
    let (all, bass) = window_chroma(note_events, t, window_s, decay_s);
    match_chroma_biased(&all, &bass, key, prefer, margin).map(ChordAt::Match)
}

/// The pitch classes sounding at time `t` (bit `pc` set), every part included: the notes the
/// tensions over the shown chord are read from.
pub(crate) fn active_mask(note_events: &[NoteEvent], t: f32) -> u16 {
    let mut mask = 0u16;
    for &(s, e, p, ..) in note_events {
        if s <= t + 1e-6 && e >= t {
            mask |= 1 << p.rem_euclid(12);
        }
    }
    mask
}

// A held chord needs a *later* checkpoint to exist at all before a layer's hold-time can ever be
// judged satisfied -- onset-only checkpoints suffice for a chord change, but a clean block chord
// (every note onsetting together, then nothing else until the next chord's onset) generates only
// one tightly-clustered checkpoint and then silence until whatever triggers the next one.
// Harmonica's streaming blocks complete every 0.256s, so without this grid a solid block chord
// with no follow-up onset could go unconfirmed for the rest of its duration. The grid step just
// needs to be smaller than the shortest hold; this has ample margin either way.
pub const CHORD_CHECKPOINT_GRID_S: f32 = 0.05;

/// The times to re-evaluate the chords in `(prev_covered, covered]`: every note onset within the
/// newly covered span, a periodic grid (see `CHORD_CHECKPOINT_GRID_S`), and `covered` itself.
pub fn chord_checkpoints(note_events: &[NoteEvent], prev_covered: f32, covered: f32) -> Vec<f32> {
    let mut checkpoints: BTreeSet<i64> = note_events
        .iter()
        .map(|&(s, ..)| s)
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
    checkpoints.into_iter().map(|k| k as f32 / 1000.0).collect()
}

// A committed chord's (backdated) onset is snapped to the nearest point on this subdivision of
// the current `BeatGrid` (see `BeatTracker::beat_grid`), once one is known -- fine enough (a
// sixteenth of a beat) that a legitimate off-beat onset (an eighth-note anticipation, a
// syncopated pickup) still lands on its own grid line rather than being dragged all the way to
// the nearest beat, but coarse enough to absorb the residual onset-detection/hold-time slop.
// No published source prescribes this -- start here and tune empirically against live playing.
const CHORD_SNAP_SUBDIVISIONS_PER_BEAT: f64 = 4.0; // sixteenth notes

/// Nearest point on `grid`'s subdivision lattice to `t`, or `t` unchanged if no grid is known yet
/// (session start, or beat tracking disabled/unavailable -- matches file mode, which never has
/// one). Snapped once, at commit time, and never revised afterward even if the grid estimate is
/// later corrected: such corrections are rare in practice (confirmed live, 2026-09-27).
pub fn snap_to_beat_grid(t: f32, grid: Option<BeatGrid>) -> f32 {
    let Some(grid) = grid else { return t };
    if !(grid.period_s > 0.0) {
        return t; // guards against a degenerate/zero period rather than trusting it blindly
    }
    let step = grid.period_s / CHORD_SNAP_SUBDIVISIONS_PER_BEAT;
    let k = ((t as f64 - grid.anchor_s) / step).round();
    (grid.anchor_s + k * step) as f32
}
