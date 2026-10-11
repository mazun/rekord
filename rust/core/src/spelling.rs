//! How pitch classes are spelled in chord names and key names: always with
//! sharps, always with flats, or `Auto` -- whatever reads most naturally on a
//! piano score. Everything here is a pure function of (pitch class, chord
//! quality, key), so the rules are unit-tested below.
//!
//! `Auto`, for a chord root:
//! 1. dim / dim7 / m7b5 roots are sharp (C#dim7 -> D, F#m7b5, G#dim): in
//!    practice these are leading-tone or passing chords that rise a semitone.
//! 2. With a key: the root is spelled as a scale degree of the key (tonic
//!    spelled by its key signature, chromatic degrees by the same convention
//!    as `key_estimator::DEGREE_NAMES`: b2 b3 b6 b7, but #4). That gives
//!    A major C#m / F#m, D major F#m and a borrowed Bb, C major Bb / Eb / Ab.
//! 3. Without a key (session start): a fixed table -- flats, except F#, and
//!    except C#m / F#m / G#m (Dbm, Gbm and Abm have 8, 9 and 7 flats).
//!
//! The bass of a slash chord is spelled as the interval above the already
//! spelled root (letter arithmetic), so root and bass never disagree
//! (G♭/B♭, never G♭/A♯) and E/G♯, Fm/A♭, Gmaj7/F♯, C/B♭ all fall out.

use std::str::FromStr;

use crate::chord_templates::ChordMatch;
use crate::key_estimator::KeyEstimate;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accidental {
    #[default]
    Auto,
    Sharp,
    Flat,
}

impl FromStr for Accidental {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "auto" => Ok(Accidental::Auto),
            "sharp" => Ok(Accidental::Sharp),
            "flat" => Ok(Accidental::Flat),
            other => anyhow::bail!("expected auto, sharp or flat, got {other:?}"),
        }
    }
}

const SHARP_NAMES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];
const FLAT_NAMES: [&str; 12] = [
    "C", "D♭", "D", "E♭", "E", "F", "G♭", "G", "A♭", "A", "B♭", "B",
];
// Auto without a key (or for a bare note): flats except F♯.
const AUTO_NAMES: [&str; 12] = [
    "C", "D♭", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
];
// Auto, minor-type chord root without a key: the sharp side where the flat spelling is absurd.
const AUTO_MINOR_NAMES: [&str; 12] = [
    "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "G♯", "A", "B♭", "B",
];
// Major keys by tonic, in key-signature order of preference (G♭ over F♯, as the flat side).
const MAJOR_KEY_NAMES: [&str; 12] = FLAT_NAMES;
// Relative minors: fewest accidentals in the signature.
const MINOR_KEY_NAMES: [&str; 12] = AUTO_MINOR_NAMES;

/// A single plain note name in `mode`, with no chord or key context (the CLI's note lines).
pub fn note_name(pc: i32, mode: Accidental) -> &'static str {
    let table = match mode {
        Accidental::Sharp => &SHARP_NAMES,
        Accidental::Flat => &FLAT_NAMES,
        Accidental::Auto => &AUTO_NAMES,
    };
    table[pc.rem_euclid(12) as usize]
}

/// "C Major (A Minor)" for the pair whose major tonic is `relative_major_pc`.
pub fn key_pair_name(relative_major_pc: i32, mode: Accidental) -> String {
    let major = relative_major_pc.rem_euclid(12);
    let minor = (major - 3).rem_euclid(12);
    let (major_name, minor_name) = match mode {
        Accidental::Auto => (
            MAJOR_KEY_NAMES[major as usize],
            MINOR_KEY_NAMES[minor as usize],
        ),
        _ => (note_name(major, mode), note_name(minor, mode)),
    };
    format!("{major_name} Major ({minor_name} Minor)")
}

pub fn key_name(key: &KeyEstimate, mode: Accidental) -> String {
    key_pair_name(key.relative_major_pc(), mode)
}

// Letter names index 0..7 = C D E F G A B; their natural pitch classes.
const LETTERS: [char; 7] = ['C', 'D', 'E', 'F', 'G', 'A', 'B'];
const NATURAL_PC: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// A spelled note: letter index plus accidental in semitones (-1 = ♭, +1 = ♯).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spelled {
    letter: usize,
    acc: i32,
}

impl Spelled {
    /// The note with this letter that has pitch class `pc`, or None if that needs a double
    /// accidental. C♭ F♭ E♯ B♯ are replaced by their natural enharmonics (B, E, F, C).
    fn on_letter(letter: usize, pc: i32) -> Option<Spelled> {
        let mut acc = (pc - NATURAL_PC[letter]).rem_euclid(12);
        if acc > 6 {
            acc -= 12;
        }
        let s = Spelled { letter, acc };
        if acc.abs() > 1 {
            return None;
        }
        Some(match (letter, acc) {
            (0, -1) => Spelled { letter: 6, acc: 0 }, // C♭ -> B
            (3, -1) => Spelled { letter: 2, acc: 0 }, // F♭ -> E
            (2, 1) => Spelled { letter: 3, acc: 0 },  // E♯ -> F
            (6, 1) => Spelled { letter: 0, acc: 0 },  // B♯ -> C
            _ => s,
        })
    }

    fn name(self) -> String {
        let suffix = match self.acc {
            -1 => "♭",
            1 => "♯",
            _ => "",
        };
        format!("{}{suffix}", LETTERS[self.letter])
    }
}

fn is_dim_class(quality: &str) -> bool {
    matches!(quality, "dim" | "dim7" | "m7b5")
}

fn is_minor_type(quality: &str) -> bool {
    quality.starts_with('m') && !quality.starts_with("maj")
}

/// Letter steps above the tonic for each semitone offset: unison, b2, 2, b3, 3, 4, #4, 5, b6, 6, b7, 7.
const DEGREE_STEPS: [usize; 12] = [0, 1, 1, 2, 2, 3, 3, 4, 5, 5, 6, 6];

/// Letter steps above the root for a bass `semitones` above it. A tritone is the b5 of a
/// diminished chord and the #4 (#11) of anything else; 8 semitones is the #5 of an augmented
/// chord and the b6 of anything else.
fn interval_steps(semitones: i32, quality: &str) -> usize {
    match semitones {
        0 => 0,
        1 | 2 => 1,
        3 | 4 => 2,
        5 => 3,
        6 => {
            if is_dim_class(quality) {
                4
            } else {
                3
            }
        }
        7 => 4,
        8 => {
            if quality == "aug" {
                4
            } else {
                5
            }
        }
        9 => 5,
        _ => 6,
    }
}

fn auto_root(pc: i32, quality: &str, key: Option<&KeyEstimate>) -> String {
    if is_dim_class(quality) {
        return SHARP_NAMES[pc as usize].to_string();
    }
    if let Some(key) = key {
        let tonic = key.relative_major_pc();
        let tonic_letter = MAJOR_KEY_NAMES[tonic as usize]
            .chars()
            .next()
            .and_then(|c| LETTERS.iter().position(|&l| l == c));
        let steps = DEGREE_STEPS[(pc - tonic).rem_euclid(12) as usize];
        if let Some(tonic_letter) = tonic_letter {
            if let Some(s) = Spelled::on_letter((tonic_letter + steps) % 7, pc) {
                return s.name();
            }
        }
    }
    let table = if is_minor_type(quality) {
        &AUTO_MINOR_NAMES
    } else {
        &AUTO_NAMES
    };
    table[pc as usize].to_string()
}

/// Root name, quality, and `/bass` when the bass differs from the root.
pub fn chord_label(chord: &ChordMatch, key: Option<&KeyEstimate>, mode: Accidental) -> String {
    let quality = chord.quality_suffix();
    let root_pc = chord.root_pc.rem_euclid(12);
    let (root_name, bass_name) = match mode {
        Accidental::Auto => {
            let root = auto_root(root_pc, chord.quality, key);
            let bass = chord
                .bass_pc
                .filter(|&b| b.rem_euclid(12) != root_pc)
                .map(|b| {
                    let b = b.rem_euclid(12);
                    // Spell the bass from the spelled root, so the two agree.
                    let root_letter = root
                        .chars()
                        .next()
                        .and_then(|c| LETTERS.iter().position(|&l| l == c));
                    let from_root = root_letter.and_then(|rl| {
                        let steps = interval_steps((b - root_pc).rem_euclid(12), chord.quality);
                        Spelled::on_letter((rl + steps) % 7, b)
                    });
                    match from_root {
                        Some(s) => s.name(),
                        None => AUTO_NAMES[b as usize].to_string(),
                    }
                });
            (root, bass)
        }
        _ => (
            note_name(root_pc, mode).to_string(),
            chord
                .bass_pc
                .filter(|&b| b.rem_euclid(12) != root_pc)
                .map(|b| note_name(b, mode).to_string()),
        ),
    };
    match bass_name {
        Some(bass) => format!("{root_name}{quality}/{bass}"),
        None => format!("{root_name}{quality}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_estimator::Mode;

    fn chord(root: i32, quality: &'static str, bass: Option<i32>) -> ChordMatch {
        ChordMatch {
            root_pc: root,
            quality,
            bass_pc: bass,
        }
    }

    fn major(tonic: i32) -> KeyEstimate {
        KeyEstimate {
            tonic_pc: tonic,
            mode: Mode::Major,
        }
    }

    fn auto(c: ChordMatch, key: Option<KeyEstimate>) -> String {
        chord_label(&c, key.as_ref(), Accidental::Auto)
    }

    #[test]
    fn sharp_and_flat_modes_are_uniform() {
        let c = chord(1, "m7", Some(8));
        assert_eq!(chord_label(&c, None, Accidental::Sharp), "C♯m7/G♯");
        assert_eq!(chord_label(&c, None, Accidental::Flat), "D♭m7/A♭");
        // the key is ignored outside Auto
        assert_eq!(
            chord_label(&c, Some(&major(9)), Accidental::Flat),
            "D♭m7/A♭"
        );
    }

    #[test]
    fn dim_class_roots_are_sharp_whatever_the_key() {
        for key in [None, Some(major(0)), Some(major(3)), Some(major(2))] {
            assert_eq!(auto(chord(1, "dim7", None), key), "C♯dim7");
            assert_eq!(auto(chord(6, "m7b5", None), key), "F♯m7b5");
            assert_eq!(auto(chord(8, "dim", None), key), "G♯dim");
            assert_eq!(auto(chord(3, "dim", None), key), "D♯dim");
        }
    }

    #[test]
    fn without_a_key_roots_are_flat_except_f_sharp_and_minor_sharps() {
        assert_eq!(auto(chord(1, "maj", None), None), "D♭");
        assert_eq!(auto(chord(3, "7", None), None), "E♭7");
        assert_eq!(auto(chord(6, "maj", None), None), "F♯");
        assert_eq!(auto(chord(8, "maj7", None), None), "A♭maj7");
        assert_eq!(auto(chord(10, "maj", None), None), "B♭");
        assert_eq!(auto(chord(1, "m", None), None), "C♯m");
        assert_eq!(auto(chord(6, "m7", None), None), "F♯m7");
        assert_eq!(auto(chord(8, "m", None), None), "G♯m");
        assert_eq!(auto(chord(3, "m", None), None), "E♭m");
        assert_eq!(auto(chord(10, "m9", None), None), "B♭m9");
    }

    #[test]
    fn roots_follow_the_key() {
        // C major: borrowed chords are flat
        assert_eq!(auto(chord(10, "maj", None), Some(major(0))), "B♭");
        assert_eq!(auto(chord(3, "maj", None), Some(major(0))), "E♭");
        assert_eq!(auto(chord(8, "maj", None), Some(major(0))), "A♭");
        // A major: C♯m, F♯m
        assert_eq!(auto(chord(1, "m", None), Some(major(9))), "C♯m");
        assert_eq!(auto(chord(6, "m7", None), Some(major(9))), "F♯m7");
        assert_eq!(auto(chord(8, "7", None), Some(major(4))), "G♯7"); // E major: V/vi
                                                                      // D major: F♯m, with a borrowed B♭
        assert_eq!(auto(chord(6, "m", None), Some(major(2))), "F♯m");
        assert_eq!(auto(chord(10, "maj", None), Some(major(2))), "B♭");
        // B♭ major / E♭ major are flat
        assert_eq!(auto(chord(3, "maj", None), Some(major(10))), "E♭");
        assert_eq!(auto(chord(8, "maj", None), Some(major(3))), "A♭");
        // an A-minor song is keyed as its relative major C
        let a_minor = KeyEstimate {
            tonic_pc: 9,
            mode: Mode::Minor,
        };
        assert_eq!(auto(chord(4, "7", Some(8)), Some(a_minor)), "E7/G♯");
    }

    #[test]
    fn never_spells_c_flat_or_e_sharp() {
        // B major: #IV is F, not E♯; G♭ major: IV is B, not C♭
        assert_eq!(auto(chord(5, "maj", None), Some(major(11))), "F");
        assert_eq!(auto(chord(11, "maj", None), Some(major(6))), "B");
        for key in 0..12 {
            for pc in 0..12 {
                let label = auto(chord(pc, "maj", None), Some(major(key)));
                for bad in ["C♭", "F♭", "E♯", "B♯", "𝄪", "𝄫"] {
                    assert!(!label.contains(bad), "{label} in key {key}");
                }
            }
        }
    }

    #[test]
    fn bass_is_spelled_from_the_root() {
        let none = None;
        // third in the bass: major chords sharp, minor chords flat
        assert_eq!(auto(chord(4, "maj", Some(8)), none), "E/G♯");
        assert_eq!(auto(chord(2, "maj", Some(6)), none), "D/F♯");
        assert_eq!(auto(chord(9, "maj", Some(1)), none), "A/C♯");
        assert_eq!(auto(chord(11, "maj", Some(3)), none), "B/D♯");
        assert_eq!(auto(chord(5, "m", Some(8)), none), "Fm/A♭");
        assert_eq!(auto(chord(7, "m", Some(10)), none), "Gm/B♭");
        assert_eq!(auto(chord(0, "m", Some(3)), none), "Cm/E♭");
        // fifth
        assert_eq!(auto(chord(11, "maj", Some(6)), none), "B/F♯");
        // seventh: major 7th sharp, minor/dominant 7th flat
        assert_eq!(auto(chord(7, "maj7", Some(6)), none), "Gmaj7/F♯");
        assert_eq!(auto(chord(2, "maj7", Some(1)), none), "Dmaj7/C♯");
        assert_eq!(auto(chord(0, "maj", Some(10)), none), "C/B♭");
        assert_eq!(auto(chord(5, "7", Some(3)), none), "F7/E♭");
        // a G♭-spelled root keeps a flat third; an F♯-spelled root a sharp one
        assert_eq!(auto(chord(6, "maj", Some(10)), Some(major(6))), "G♭/B♭");
        assert_eq!(auto(chord(6, "maj", Some(10)), None), "F♯/A♯");
    }

    #[test]
    fn bass_equal_to_root_is_not_shown() {
        assert_eq!(auto(chord(4, "m", Some(4)), None), "Em");
    }

    #[test]
    fn key_names() {
        let expected = [
            "C Major (A Minor)",
            "D♭ Major (B♭ Minor)",
            "D Major (B Minor)",
            "E♭ Major (C Minor)",
            "E Major (C♯ Minor)",
            "F Major (D Minor)",
            "G♭ Major (E♭ Minor)",
            "G Major (E Minor)",
            "A♭ Major (F Minor)",
            "A Major (F♯ Minor)",
            "B♭ Major (G Minor)",
            "B Major (G♯ Minor)",
        ];
        for (pc, want) in expected.iter().enumerate() {
            assert_eq!(key_pair_name(pc as i32, Accidental::Auto), *want);
        }
        assert_eq!(key_pair_name(4, Accidental::Sharp), "E Major (C♯ Minor)");
        assert_eq!(key_pair_name(1, Accidental::Sharp), "C♯ Major (A♯ Minor)");
        assert_eq!(key_pair_name(1, Accidental::Flat), "D♭ Major (B♭ Minor)");
        assert_eq!(key_pair_name(9, Accidental::Flat), "A Major (G♭ Minor)");
    }
}
