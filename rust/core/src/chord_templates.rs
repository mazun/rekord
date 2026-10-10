//! Rule-based chord naming: match how long each pitch class sounded in a short window
//! against chord templates (root + quality, intervals in semitones from root).

use crate::key_estimator::KeyEstimate;
use crate::spelling::{chord_label, Accidental};

pub struct ChordTemplate {
    pub quality: &'static str,
    pub intervals: &'static [i32],
}

pub const CHORD_TEMPLATES: &[ChordTemplate] = &[
    // Root + perfect 5th only, no 3rd -- a "power chord" (or a bare 4th/5th dyad read from its
    // other end). Without it a distorted electric guitar power chord was always mislabeled as a
    // full major chord (confirmed in real-world testing, 2026-09-03): a dyad leaves every
    // 3-tone template missing one tone. Its prior (`POWER_CHORD_PRIOR`) is lower than the other
    // ornate qualities' so that it still wins when the 3rd is really absent; the price is that
    // on RWC-Pop it also names some chords whose 3rd the transcription missed (majmin -.04).
    ChordTemplate { quality: "5", intervals: &[0, 7] },
    ChordTemplate { quality: "maj", intervals: &[0, 4, 7] },
    ChordTemplate { quality: "m", intervals: &[0, 3, 7] },
    ChordTemplate { quality: "dim", intervals: &[0, 3, 6] },
    ChordTemplate { quality: "aug", intervals: &[0, 4, 8] },
    ChordTemplate { quality: "sus2", intervals: &[0, 2, 7] },
    ChordTemplate { quality: "sus4", intervals: &[0, 5, 7] },
    ChordTemplate { quality: "6", intervals: &[0, 4, 7, 9] },
    ChordTemplate { quality: "m6", intervals: &[0, 3, 7, 9] },
    ChordTemplate { quality: "7", intervals: &[0, 4, 7, 10] },
    ChordTemplate { quality: "maj7", intervals: &[0, 4, 7, 11] },
    ChordTemplate { quality: "m7", intervals: &[0, 3, 7, 10] },
    ChordTemplate { quality: "m7b5", intervals: &[0, 3, 6, 10] },
    ChordTemplate { quality: "dim7", intervals: &[0, 3, 6, 9] },
    ChordTemplate { quality: "add9", intervals: &[0, 4, 7, 2] },
    ChordTemplate { quality: "madd9", intervals: &[0, 3, 7, 2] },
    ChordTemplate { quality: "9", intervals: &[0, 4, 7, 10, 2] },
    ChordTemplate { quality: "maj9", intervals: &[0, 4, 7, 11, 2] },
    ChordTemplate { quality: "m9", intervals: &[0, 3, 7, 10, 2] },
    ChordTemplate { quality: "sus4(7)", intervals: &[0, 5, 7, 10] },
    ChordTemplate { quality: "11", intervals: &[0, 4, 7, 10, 2, 5] },
    ChordTemplate { quality: "m11", intervals: &[0, 3, 7, 10, 2, 5] },
    ChordTemplate { quality: "13", intervals: &[0, 4, 7, 10, 9] },
    ChordTemplate { quality: "maj13", intervals: &[0, 4, 7, 11, 9] },
];

// Scoring of `match_chroma`. All of these are this project's own (no published prescription;
// found by sweeping against RWC-Pop, see CLAUDE.md "Chord naming"): the score of a template is
// the share of the window's note energy inside it, minus the share outside it, minus a penalty
// for each template tone that's nearly absent (so a 7th or 9th is only named when it actually
// sounds for a while), minus a small per-tone cost, plus the share of the bass part's energy
// that sits on the root.
const OUTSIDE_WEIGHT: f32 = 0.7;
const MISSING_FLOOR: f32 = 0.06;
const MISSING_PENALTY: f32 = 3.0;
const SIZE_PENALTY: f32 = 0.1;
const BASS_ROOT_WEIGHT: f32 = 0.3;
// An augmented triad reads the same from each of its three tones, so the notes cannot say which
// is the root (the first one tried won, always the lowest pitch class). The "Blackadder chord"
// puts the bass a whole tone above the root (B♭aug/C), so an aug root with the bass there gets
// this much per share of bass: a tie-break between the three readings, not evidence for aug.
const AUG_WHOLE_TONE_BASS_WEIGHT: f32 = 0.05;
// `quality_prior` tiers: 7th chords, then sus/6th/power/dim/aug, then the 9/11/13 family.
const SEVENTH_PRIOR: f32 = 0.1;
const ORNATE_PRIOR: f32 = 0.5;
const EXTENDED_PRIOR: f32 = 0.2;
const SIXTH_PRIOR: f32 = 0.2;
const POWER_CHORD_PRIOR: f32 = 0.15;
// Lower than the rest of the ornate tier: an augmented triad (Blackadder chord, B♭aug/C) that
// sounds for under a second lost to its major triad at 0.5. On RWC-Pop 0.2 costs majmin .001.
const AUG_PRIOR: f32 = 0.2;
// A 3rd (major or minor) at least this share of the window counts as really sounding.
const STRONG_THIRD_SHARE: f32 = 0.12;

/// Extra cost of naming `quality` instead of a plain major/minor triad: the rarer or more
/// ornate a chord is, the more evidence it needs.
fn quality_prior(quality: &str) -> f32 {
    match quality {
        "maj" | "m" => 0.0,
        "7" | "maj7" | "m7" => SEVENTH_PRIOR,
        "5" => POWER_CHORD_PRIOR,
        "6" | "m6" => SIXTH_PRIOR,
        "aug" => AUG_PRIOR,
        "sus2" | "sus4" | "sus4(7)" | "dim" | "m7b5" | "dim7" => ORNATE_PRIOR,
        _ => EXTENDED_PRIOR,
    }
}
// A pitch class this far below the strongest one doesn't count towards "at least two pitch
// classes sound" (kept from the binary version, where fewer than two meant "no chord").
const PRESENT_SHARE: f32 = 0.03;
// The bass is only named (for a slash chord) when the bass part has at least this much energy.
const MIN_BASS_ENERGY: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChordMatch {
    pub root_pc: i32,
    pub quality: &'static str,
    pub bass_pc: Option<i32>,
}

impl ChordMatch {
    /// The suffix `label()` appends after the root name -- empty for a
    /// plain major triad, the quality string otherwise. Also used by
    /// `key_estimator::roman_numeral()` so a Roman-numeral label carries
    /// exactly the same quality suffix a normal chord label would show.
    pub fn quality_suffix(&self) -> &'static str {
        if self.quality == "maj" { "" } else { self.quality }
    }

    /// The chord's display name, spelled per `mode` (and, for `Accidental::Auto`, the current
    /// key); see `spelling`.
    pub fn label(&self, key: Option<&KeyEstimate>, mode: Accidental) -> String {
        chord_label(self, key, mode)
    }
}

fn pitch_class_set(pitches: &[i32]) -> u16 {
    let mut mask = 0u16;
    for &p in pitches {
        mask |= 1 << p.rem_euclid(12);
    }
    mask
}

/// A bare root + fifth (`5`) in a known key, on a root whose chord in that key's scale has a
/// definite 3rd, is that diatonic chord with its 3rd missing: no 3rd of either kind sounds at
/// least `STRONG_THIRD_SHARE` strong, so it reads as the key's own major or minor triad instead.
/// (Kept in front of the key-less result so that without a key, or on a root outside the scale,
/// a power chord is still named as one.) Without the key this guess is worth .036 majmin on
/// RWC-Pop; while the key estimate lags a modulation it names the old key's triad instead (see
/// `KeyEstimator::naming_key`). A 3rd read from the sung melody, or a major 7th taken as a major
/// 3rd, overruled the key on too few chords to matter (+.001, docs/chord-layers.md section 11).
fn diatonic_triad_for_bare_fifth(m: &mut ChordMatch, share: &[f32], key: &KeyEstimate) {
    if m.quality != "5" {
        return;
    }
    if let Some(quality) = bare_fifth_triad(m.root_pc as usize, share, key) {
        m.quality = quality;
    }
}

/// The quality a bare fifth on `root` stands for in `key` (`"maj"` or `"m"`), if no 3rd sounds
/// (`share` = each pitch class's share of the window) and the key's scale has a definite one.
pub fn bare_fifth_triad(root: usize, share: &[f32], key: &KeyEstimate) -> Option<&'static str> {
    if share[(root + 3) % 12].max(share[(root + 4) % 12]) >= STRONG_THIRD_SHARE {
        return None;
    }
    let base = key.relative_major_pc();
    let in_scale = |pc: usize| matches!((pc as i32 - base).rem_euclid(12), 0 | 2 | 4 | 5 | 7 | 9 | 11);
    match (in_scale((root + 4) % 12), in_scale((root + 3) % 12)) {
        (true, false) => Some("maj"),
        (false, true) => Some("m"),
        _ => None,
    }
}

/// Best-scoring chord for a window's worth of notes (`key`, if known, only decides what a bare
/// fifth is, see `diatonic_triad_for_bare_fifth`): `all` is how long each pitch class sounded
/// in the accompaniment (summed overlap with the window, in seconds), `bass` the same for the
/// bass part alone (or the lowest note at each moment where there is no bass part). `None` if
/// fewer than two pitch classes are really present, or nothing sounded at all.
pub fn match_chroma(all: &[f32; 12], bass: &[f32; 12], key: Option<&KeyEstimate>) -> Option<ChordMatch> {
    match_chroma_biased(all, bass, key, None, 0.0)
}

/// `match_chroma` with a bias towards the chord already shown: `prefer` (its root and quality)
/// keeps the win unless the best template beats its score by more than `margin`. This damps
/// flicker without waiting any time, unlike a hold. `margin <= 0` is plain `match_chroma`.
pub fn match_chroma_biased(
    all: &[f32; 12],
    bass: &[f32; 12],
    key: Option<&KeyEstimate>,
    prefer: Option<(i32, &str)>,
    margin: f32,
) -> Option<ChordMatch> {
    let scored = score_templates(all, bass)?;
    let (share, bass_pc) = (&scored.share, scored.bass_pc);

    let mut best: Option<(ChordMatch, f32)> = None;
    let mut preferred: Option<(ChordMatch, f32)> = None;
    for root in 0..12usize {
        for (index, tpl) in CHORD_TEMPLATES.iter().enumerate() {
            let score = scored.scores[root * TEMPLATE_COUNT + index];
            if best.as_ref().map_or(true, |(_, best_score)| score > *best_score) {
                best = Some((ChordMatch { root_pc: root as i32, quality: tpl.quality, bass_pc }, score));
            }
            if margin > 0.0 && prefer == Some((root as i32, tpl.quality)) {
                preferred = Some((ChordMatch { root_pc: root as i32, quality: tpl.quality, bass_pc }, score));
            }
        }
    }
    if let (Some((_, best_score)), Some((_, preferred_score))) = (best.as_ref(), preferred.as_ref()) {
        if best_score - preferred_score < margin {
            best = preferred;
        }
    }
    if let (Some((m, _)), Some(key)) = (best.as_mut(), key) {
        diatonic_triad_for_bare_fifth(m, share, key);
    }
    best.map(|(m, _)| m)
}

pub const TEMPLATE_COUNT: usize = CHORD_TEMPLATES.len();

/// How well every template at every root explains one window (the numbers `match_chroma_biased`
/// picks the best of), for a caller that wants more than the winner.
pub struct TemplateScores {
    /// Each pitch class's share of the window's note energy.
    pub share: [f32; 12],
    /// Each pitch class's share of the bass part's energy (all zero without a bass part).
    pub bass_share: [f32; 12],
    /// The bass part's dominant pitch class, if it has enough energy to be named.
    pub bass_pc: Option<i32>,
    /// `scores[root * TEMPLATE_COUNT + index into CHORD_TEMPLATES]`.
    pub scores: [f32; 12 * TEMPLATE_COUNT],
}

/// The score of every (root, template) for a window (`all` / `bass`: see `match_chroma`). `None`
/// if fewer than two pitch classes are really present, or nothing sounded at all.
pub fn score_templates(all: &[f32; 12], bass: &[f32; 12]) -> Option<TemplateScores> {
    let total: f32 = all.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut share = [0.0f32; 12];
    for (s, &x) in share.iter_mut().zip(all) {
        *s = x / total;
    }
    if share.iter().filter(|&&c| c >= PRESENT_SHARE).count() < 2 {
        return None;
    }
    let bass_total: f32 = bass.iter().sum();
    let mut bass_share = [0.0f32; 12];
    if bass_total > 0.0 {
        for (s, &x) in bass_share.iter_mut().zip(bass) {
            *s = x / bass_total;
        }
    }
    let bass_pc = (bass_total >= MIN_BASS_ENERGY * total)
        .then(|| (0..12usize).max_by(|&a, &b| bass[a].partial_cmp(&bass[b]).unwrap()).unwrap() as i32);

    let mut scores = [0.0f32; 12 * TEMPLATE_COUNT];
    for root in 0..12usize {
        for (index, tpl) in CHORD_TEMPLATES.iter().enumerate() {
            let mut inside = 0.0;
            let mut missing = 0.0;
            for &i in tpl.intervals {
                let c = share[(root + i as usize) % 12];
                inside += c;
                missing += (MISSING_FLOOR - c).max(0.0);
            }
            if tpl.quality == "aug" {
                // The Blackadder chord's bass: the tone a whole tone above the root is part of the
                // chord when the bass plays it, not a stray note outside the triad.
                let above = (root + 2) % 12;
                inside += share[above] * bass_share[above];
            }
            scores[root * TEMPLATE_COUNT + index] = inside - OUTSIDE_WEIGHT * (1.0 - inside)
                - MISSING_PENALTY * missing
                - SIZE_PENALTY * tpl.intervals.len() as f32
                - quality_prior(tpl.quality)
                + BASS_ROOT_WEIGHT * bass_share[root]
                + if tpl.quality == "aug" { AUG_WHOLE_TONE_BASS_WEIGHT * bass_share[(root + 2) % 12] } else { 0.0 };
        }
    }
    Some(TemplateScores { share, bass_share, bass_pc, scores })
}

// Canonical upper-structure tension names by semitone offset from the
// chord root -- exactly the 7 slots the piano-roll UI always shows (in
// this fixed order, so a diffed/re-rendered tension list never needs
// sorting). Offsets that coincide with a triad/7th tone (0, 3, 4, 7, 10,
// 11) are deliberately absent: a note there is either already part of
// the stabilized chord label or was matched by `match_chroma`
// itself, not something this display is meant to call out.
const TENSION_NAMES: [(i32, &str); 7] =
    [(1, "b9"), (2, "9"), (3, "#9"), (5, "11"), (6, "#11"), (8, "b13"), (9, "13")];

/// Pitch classes in `active_pitch_classes` that fall outside `chord`'s own
/// template -- e.g. a melodic passing tone sounding over an already-
/// stabilized chord. Returns tension names in `TENSION_NAMES` order
/// (independent of note order/onset order), so callers can compare two
/// results with plain `==` to detect a real change. Any active pitch
/// class that doesn't land on one of the 7 named offsets (e.g. it's
/// already part of `chord`'s own template) is silently omitted, not an
/// error -- this only ever reports the offsets the UI has a slot for.
pub fn extra_tensions(active_pitch_classes: &[i32], chord: &ChordMatch) -> Vec<&'static str> {
    let template = CHORD_TEMPLATES
        .iter()
        .find(|tpl| tpl.quality == chord.quality)
        .expect("ChordMatch::quality always names an existing CHORD_TEMPLATES entry");
    let mut template_mask = 0u16;
    for &i in template.intervals {
        template_mask |= 1 << (chord.root_pc + i).rem_euclid(12);
    }
    let extra_mask = pitch_class_set(active_pitch_classes) & !template_mask;
    TENSION_NAMES
        .iter()
        .filter(|&&(interval, _)| {
            let pc = (chord.root_pc + interval).rem_euclid(12);
            extra_mask & (1 << pc) != 0
        })
        .map(|&(_, name)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aug_score(roots_bass: i32) -> Vec<(i32, f32)> {
        // G♭ B♭ D with a C (the bass) sounding.
        let mut all = [0.0f32; 12];
        for (pc, w) in [(6, 0.43), (10, 0.33), (2, 0.14), (0, 0.10)] {
            all[pc] = w;
        }
        let mut bass = [0.0f32; 12];
        bass[roots_bass as usize] = 1.0;
        let scored = score_templates(&all, &bass).unwrap();
        let aug = CHORD_TEMPLATES.iter().position(|t| t.quality == "aug").unwrap();
        [2, 6, 10].iter().map(|&r| (r, scored.scores[r as usize * TEMPLATE_COUNT + aug])).collect()
    }

    #[test]
    fn aug_root_is_the_tone_a_whole_tone_below_the_bass() {
        let scores = aug_score(0);
        let best = scores.iter().copied().fold((0, f32::NEG_INFINITY), |b, s| if s.1 > b.1 { s } else { b });
        assert_eq!(best.0, 10, "B♭aug/C expected, got {scores:?}");
    }

    #[test]
    fn aug_roots_tie_when_the_bass_is_not_a_whole_tone_above_one() {
        // Bass on G♭ itself (root position): the root bonus decides, not the whole-tone one.
        let scores = aug_score(6);
        assert!(scores[1].1 > scores[0].1 && scores[1].1 > scores[2].1, "{scores:?}");
    }

    #[test]
    fn the_whole_tone_bass_is_part_of_the_aug_chord_not_a_seventh() {
        // E♭ G B over an F bass (shares from a recording): the Blackadder chord E♭aug/F, which a G7
        // without its 5th and with a stray E♭ also fits; the F is the chord's bass, not a 7th.
        let mut all = [0.0f32; 12];
        for (pc, w) in [(3, 0.17), (5, 0.33), (7, 0.17), (11, 0.33)] {
            all[pc] = w;
        }
        let mut bass = [0.0f32; 12];
        bass[5] = 1.0;
        let m = match_chroma(&all, &bass, None).unwrap();
        assert_eq!((m.root_pc, m.quality, m.bass_pc), (3, "aug", Some(5)));
    }
}

/// The chord table as data: what the frontend needs to name the tensions over a shown chord
/// (`extra_tensions`' computation) without keeping a copy of it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ChordVocabulary {
    pub templates: Vec<TemplateInfo>,
    pub tensions: Vec<TensionInfo>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TemplateInfo {
    pub quality: &'static str,
    /// Semitones above the root.
    pub intervals: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TensionInfo {
    /// Semitones above the root.
    pub interval: i32,
    pub name: &'static str,
}

pub fn vocabulary() -> ChordVocabulary {
    ChordVocabulary {
        templates: CHORD_TEMPLATES.iter().map(|t| TemplateInfo { quality: t.quality, intervals: t.intervals.to_vec() }).collect(),
        tensions: TENSION_NAMES.iter().map(|&(interval, name)| TensionInfo { interval, name }).collect(),
    }
}
