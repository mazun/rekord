"""Turn repeated (whole-buffer) note-extraction passes into a stable stream
of "confirmed" note events, holding back notes near the live edge since
basic-pitch may still revise them once more audio arrives.
"""

from dataclasses import dataclass
from typing import List, Set, Tuple

from chord_poc.note_extractor import NoteEvent

# Notes ending within this many seconds of the current buffer edge are not
# yet confirmed -- they may be extended or reshaped by the next pass.
CONFIRM_MARGIN_S = 0.5


@dataclass
class ConfirmedNote:
    start_s: float
    end_s: float
    pitch_midi: int
    amplitude: float


class NoteStitcher:
    def __init__(self, confirm_margin_s: float = CONFIRM_MARGIN_S) -> None:
        self._confirm_margin_s = confirm_margin_s
        self._emitted: Set[Tuple[float, int]] = set()  # (rounded start_s, pitch)

    def update(self, buffer_duration_s: float, note_events: List[NoteEvent]) -> List[ConfirmedNote]:
        """Given the latest full note-extraction pass, return notes that just
        became confirmed (i.e. weren't returned by a previous call)."""
        confirm_before = buffer_duration_s - self._confirm_margin_s
        newly_confirmed = []
        for start_s, end_s, pitch_midi, amplitude in note_events:
            if end_s > confirm_before:
                continue  # too close to the live edge, may still change
            key = (round(start_s, 2), pitch_midi)
            if key in self._emitted:
                continue
            self._emitted.add(key)
            newly_confirmed.append(ConfirmedNote(start_s, end_s, pitch_midi, amplitude))
        newly_confirmed.sort(key=lambda n: n.start_s)
        return newly_confirmed
