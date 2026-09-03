//! Rule-based chord naming: match a set of currently-sounding pitch classes
//! against chord templates (root + quality, intervals in semitones from
//! root). Ported from chord_poc/chord_templates.py -- keep in sync.

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub struct ChordTemplate {
    pub quality: &'static str,
    pub intervals: &'static [i32],
}

pub const CHORD_TEMPLATES: &[ChordTemplate] = &[
    // Root + perfect 5th only, no 3rd -- a "power chord" (or a bare
    // 4th/5th dyad read from its other end). Every other template here
    // has at least 3 tones, so against a genuine 2-pitch-class active set
    // they all tie at the same "missing one tone" score regardless of
    // which 3rd they guess -- this scores strictly higher (a perfect,
    // 0-missing/0-extra fit) whenever exactly this dyad is what's
    // sounding, without ever being able to win when a 3rd is actually
    // present too (see chord_templates tests / CLAUDE.md). Confirmed via
    // real-world testing (2026-09-03): without this, a distorted electric
    // guitar power chord was always mislabeled as a full major chord
    // (the array-order tie-break happened to fall on "maj").
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

const EXTRA_PENALTY: f32 = 1.5;
const MISSING_PENALTY: f32 = 0.75;
const SIZE_PENALTY: f32 = 0.05;
// Chords that share all their tones (e.g. G6 and Em7 are both {G,B,D,E})
// can't be told apart by pitch class alone -- prefer the reading whose root
// matches the bass note, since that's what a listener would actually call it.
const BASS_IS_ROOT_BONUS: f32 = 0.5;

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

    pub fn label(&self) -> String {
        let mut name = format!("{}{}", NOTE_NAMES[self.root_pc as usize], self.quality_suffix());
        if let Some(bass_pc) = self.bass_pc {
            if bass_pc != self.root_pc {
                name.push('/');
                name.push_str(NOTE_NAMES[bass_pc as usize]);
            }
        }
        name
    }
}

fn pitch_class_set(pitches: &[i32]) -> u16 {
    let mut mask = 0u16;
    for &p in pitches {
        mask |= 1 << p.rem_euclid(12);
    }
    mask
}

/// Best-scoring chord for a set of active pitch classes (0-11), or None if
/// too few distinct notes are active to call it a chord.
pub fn match_chord(active_pitch_classes: &[i32], bass_pc: Option<i32>) -> Option<ChordMatch> {
    let active = pitch_class_set(active_pitch_classes);
    if active.count_ones() < 2 {
        return None;
    }

    let mut best: Option<(ChordMatch, f32)> = None;
    for root in 0..12i32 {
        if active & (1 << root) == 0 {
            continue; // root itself must be present
        }
        for tpl in CHORD_TEMPLATES {
            let mut template_mask = 0u16;
            for &i in tpl.intervals {
                template_mask |= 1 << (root + i).rem_euclid(12);
            }
            let matched = (template_mask & active).count_ones();
            let missing = (template_mask & !active).count_ones();
            let extra = (active & !template_mask).count_ones();
            let mut score = matched as f32
                - MISSING_PENALTY * missing as f32
                - EXTRA_PENALTY * extra as f32
                - SIZE_PENALTY * tpl.intervals.len() as f32;
            if bass_pc == Some(root) {
                score += BASS_IS_ROOT_BONUS;
            }
            if best.as_ref().map_or(true, |(_, best_score)| score > *best_score) {
                best = Some((
                    ChordMatch { root_pc: root, quality: tpl.quality, bass_pc },
                    score,
                ));
            }
        }
    }
    best.map(|(m, _)| m)
}

// Canonical upper-structure tension names by semitone offset from the
// chord root -- exactly the 7 slots the piano-roll UI always shows (in
// this fixed order, so a diffed/re-rendered tension list never needs
// sorting). Offsets that coincide with a triad/7th tone (0, 3, 4, 7, 10,
// 11) are deliberately absent: a note there is either already part of
// the stabilized chord label or was penalized/matched by `match_chord`
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
