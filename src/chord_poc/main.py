"""Phase 1 CLI: run basic-pitch note extraction + chord matching over either
a file (simulating a live stream) or actual WASAPI loopback audio, printing
newly confirmed notes and chord changes as they're detected.

Usage:
    python -m chord_poc.main <audio_file> [--hop 0.5] [--plot out.png]
    python -m chord_poc.main --live [--hop 0.5] [--duration 20]
"""

import argparse
import sys
from typing import List, Optional

from chord_poc.audio_io import load_audio, stream_new_chunks
from chord_poc.chord_templates import match_chord, midi_pitches_to_classes
from chord_poc.note_extractor import AUDIO_N_SAMPLES, AUDIO_SAMPLE_RATE, NoteEvent, extract_notes, load_model
from chord_poc.note_stitcher import NoteStitcher
from chord_poc.streaming_extractor import StreamingNoteExtractor

MIDI_NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

# A note's end_s can be a touch before the true edge of what's been
# processed (edge effect from limited context there). Without this
# tolerance, notes right at the boundary are missed as "active".
ACTIVE_EDGE_TOLERANCE_S = 0.2


def midi_to_name(pitch: int) -> str:
    return f"{MIDI_NOTE_NAMES[pitch % 12]}{pitch // 12 - 1}"


def _chord_label_at(note_events: List[NoteEvent], t: float, tolerance: float) -> Optional[str]:
    """Chord label for whatever's sounding at time t, "(silence)" if
    nothing is, or None if notes are present but no template matched well
    enough (ambiguous -- caller should just keep showing the prior label)."""
    active = [p for s, e, p, _a in note_events if s <= t + 1e-6 and e >= t - tolerance]
    if not active:
        return "(silence)"
    bass_pc = min(active) % 12
    chord = match_chord(midi_pitches_to_classes(active), bass_pc=bass_pc)
    return chord.label if chord else None


class ChordStreamPipeline:
    """Drives StreamingNoteExtractor + NoteStitcher + chord matching from a
    sequence of incoming audio chunks, regardless of whether they come from
    a file (simulated streaming) or a live WASAPI loopback capture."""

    def __init__(self, model) -> None:
        self.model = model
        self.extractor = StreamingNoteExtractor(model)
        self.stitcher = NoteStitcher()
        self.all_notes = []  # (start_s, end_s, pitch) -- for optional plotting
        self.last_chord_label: Optional[str] = None
        self.last_covered = 0.0

    def _drain_confirmed_notes(self) -> None:
        for n in self.stitcher.update(self.extractor.covered_seconds, self.extractor.note_events()):
            print(f"[{n.start_s:6.2f}s -> {n.end_s:6.2f}s] note {midi_to_name(n.pitch_midi)}")
            self.all_notes.append((n.start_s, n.end_s, n.pitch_midi))

    def _report_new_chords(self, note_events: List[NoteEvent], covered: float) -> None:
        """Windows complete in ~1.6s jumps, so a single "what's active now"
        check can skip right over a chord shorter than that. Instead,
        re-evaluate the chord at every note onset within the newly covered
        span (those are exactly the points where the chord could plausibly
        have changed), in chronological order."""
        checkpoints = sorted({s for s, _e, _p, _a in note_events if self.last_covered < s <= covered})
        checkpoints.append(covered)
        for t in checkpoints:
            label = _chord_label_at(note_events, t, ACTIVE_EDGE_TOLERANCE_S)
            if label is not None and label != self.last_chord_label:
                print(f"  t={t:6.2f}s  chord: {label}")
                self.last_chord_label = label
        self.last_covered = covered

    def push(self, chunk) -> None:
        if self.extractor.push(chunk):
            self._drain_confirmed_notes()
            self._report_new_chords(self.extractor.note_events(), self.extractor.covered_seconds)

    def finalize_from_file_tail(self, audio, duration: float) -> None:
        """The tail shorter than one window (~1.6s) never completes on its
        own. Rather than zero-pad it (which showed the model a window
        that's mostly synthetic silence and suppressed real notes right at
        the boundary -- verified empirically), re-run the offline
        extractor on the real last AUDIO_N_SAMPLES of audio, which needs no
        padding at all as long as the stream is at least one window long.
        Only meaningful for a finite file -- a live capture has no tail."""
        if self.last_covered >= duration or len(audio) == 0:
            return
        clip_start = max(0, len(audio) - AUDIO_N_SAMPLES)
        tail_notes = extract_notes(audio[clip_start:], self.model)
        clip_start_s = clip_start / AUDIO_SAMPLE_RATE
        tail_notes = [(s + clip_start_s, e + clip_start_s, p, a) for s, e, p, a in tail_notes]
        for n in self.stitcher.update(duration, tail_notes):
            print(f"[{n.start_s:6.2f}s -> {n.end_s:6.2f}s] note {midi_to_name(n.pitch_midi)}")
            self.all_notes.append((n.start_s, n.end_s, n.pitch_midi))
        self._report_new_chords(tail_notes, duration)


def run_file(audio_path: str, hop_seconds: float, plot_path: str = None) -> None:
    print(f"Loading {audio_path} ...")
    audio = load_audio(audio_path)
    duration = len(audio) / AUDIO_SAMPLE_RATE
    print(f"Duration: {duration:.1f}s, simulating stream with {hop_seconds:.2f}s hops\n")

    pipeline = ChordStreamPipeline(load_model())
    for _now_s, chunk in stream_new_chunks(audio, hop_seconds):
        pipeline.push(chunk)
    pipeline.finalize_from_file_tail(audio, duration)

    if plot_path:
        _plot_piano_roll(pipeline.all_notes, duration, plot_path)


def run_live(hop_seconds: float, duration_s: Optional[float] = None) -> None:
    from chord_poc.live_capture import stream_loopback_chunks

    print("Listening to default speaker output via WASAPI loopback ...")
    if duration_s:
        print(f"Will stop after {duration_s:.1f}s (Ctrl+C to stop earlier)\n")
    else:
        print("Press Ctrl+C to stop.\n")

    pipeline = ChordStreamPipeline(load_model())
    elapsed = 0.0
    try:
        for chunk in stream_loopback_chunks(hop_seconds):
            pipeline.push(chunk)
            elapsed += hop_seconds
            if duration_s and elapsed >= duration_s:
                break
    except KeyboardInterrupt:
        print("\nStopped.")


def _plot_piano_roll(notes, duration: float, plot_path: str) -> None:
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(max(6, duration), 4))
    for start_s, end_s, pitch in notes:
        ax.plot([start_s, end_s], [pitch, pitch], linewidth=4)
    ax.set_xlabel("time (s)")
    ax.set_ylabel("MIDI pitch")
    ax.set_title("Confirmed notes")
    fig.tight_layout()
    fig.savefig(plot_path)
    print(f"\nSaved piano-roll plot to {plot_path}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio_file", nargs="?", help="Path to a wav/mp3 file to analyze")
    parser.add_argument("--live", action="store_true", help="Capture live WASAPI loopback audio instead of a file")
    parser.add_argument("--duration", type=float, default=None, help="Live mode: stop after this many seconds")
    parser.add_argument("--hop", type=float, default=0.5, help="Streaming hop size in seconds")
    parser.add_argument("--plot", type=str, default=None, help="File mode: optional path to save a piano-roll PNG")
    args = parser.parse_args()

    if args.live:
        run_live(args.hop, args.duration)
    else:
        if not args.audio_file:
            parser.error("audio_file is required unless --live is given")
        run_file(args.audio_file, args.hop, args.plot)


if __name__ == "__main__":
    sys.exit(main())
