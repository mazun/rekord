"""Rule-based chord naming: match a set of currently-sounding pitch classes
against chord templates (root + quality, intervals in semitones from root).

This exists to get a tension-aware chord label out of basic-pitch's raw note
output without training any chord-recognition model. Accuracy is expected to
be well below a dedicated model (see the Phase 1 plan) -- the point here is
to validate the pipeline end-to-end cheaply before investing in one.
"""

from dataclasses import dataclass
from typing import List, Optional, Sequence

NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

# quality name -> chord tones as semitone offsets from the root.
CHORD_TEMPLATES = {
    "maj": [0, 4, 7],
    "m": [0, 3, 7],
    "dim": [0, 3, 6],
    "aug": [0, 4, 8],
    "sus2": [0, 2, 7],
    "sus4": [0, 5, 7],
    "6": [0, 4, 7, 9],
    "m6": [0, 3, 7, 9],
    "7": [0, 4, 7, 10],
    "maj7": [0, 4, 7, 11],
    "m7": [0, 3, 7, 10],
    "m7b5": [0, 3, 6, 10],
    "dim7": [0, 3, 6, 9],
    "add9": [0, 4, 7, 2],
    "madd9": [0, 3, 7, 2],
    "9": [0, 4, 7, 10, 2],
    "maj9": [0, 4, 7, 11, 2],
    "m9": [0, 3, 7, 10, 2],
    "sus4(7)": [0, 5, 7, 10],
    "11": [0, 4, 7, 10, 2, 5],
    "m11": [0, 3, 7, 10, 2, 5],
    "13": [0, 4, 7, 10, 9],
    "maj13": [0, 4, 7, 11, 9],
}

# Penalize larger templates slightly so a bare triad wins over a superset
# chord when only the triad's notes are actually sounding.
_EXTRA_PENALTY = 1.5
_MISSING_PENALTY = 0.75
_SIZE_PENALTY = 0.05
# Chords that share all their tones (e.g. G6 and Em7 are both {G,B,D,E})
# can't be told apart by pitch class alone -- prefer the reading whose root
# matches the bass note, since that's what a listener would actually call it.
_BASS_IS_ROOT_BONUS = 0.5


@dataclass
class ChordMatch:
    root_pc: int
    quality: str
    bass_pc: Optional[int]
    score: float

    @property
    def label(self) -> str:
        root = NOTE_NAMES[self.root_pc]
        name = f"{root}{self.quality}" if self.quality != "maj" else root
        if self.bass_pc is not None and self.bass_pc != self.root_pc:
            name += f"/{NOTE_NAMES[self.bass_pc]}"
        return name


def match_chord(active_pitch_classes: Sequence[int], bass_pc: Optional[int] = None) -> Optional[ChordMatch]:
    """Return the best-scoring chord for a set of active pitch classes (0-11),
    or None if too few distinct notes are active to call it a chord."""
    active = set(active_pitch_classes)
    if len(active) < 2:
        return None

    best: Optional[ChordMatch] = None
    for root in range(12):
        for quality, intervals in CHORD_TEMPLATES.items():
            template = {(root + i) % 12 for i in intervals}
            matched = template & active
            missing = template - active
            extra = active - template
            score = (
                len(matched)
                - _MISSING_PENALTY * len(missing)
                - _EXTRA_PENALTY * len(extra)
                - _SIZE_PENALTY * len(template)
            )
            if bass_pc is not None and root == bass_pc:
                score += _BASS_IS_ROOT_BONUS
            # Require the root itself and at least a third to be present.
            if root not in active:
                continue
            if best is None or score > best.score:
                best = ChordMatch(root_pc=root, quality=quality, bass_pc=bass_pc, score=score)
    return best


def midi_pitches_to_classes(pitches: List[int]) -> List[int]:
    return [p % 12 for p in pitches]
