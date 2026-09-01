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

pub struct ChordMatch {
    pub root_pc: i32,
    pub quality: &'static str,
    pub bass_pc: Option<i32>,
}

impl ChordMatch {
    pub fn label(&self) -> String {
        let root = NOTE_NAMES[self.root_pc as usize];
        let mut name = if self.quality == "maj" {
            root.to_string()
        } else {
            format!("{root}{}", self.quality)
        };
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
