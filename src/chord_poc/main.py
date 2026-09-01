"""Phase 1 CLI: feed an audio file through basic-pitch as a simulated live
stream, print newly confirmed notes and the current best-guess chord.

Usage:
    python -m chord_poc.main <audio_file> [--hop 0.5] [--plot out.png]
"""

import argparse
import sys

from chord_poc.audio_io import load_audio, stream_chunks
from chord_poc.chord_templates import match_chord, midi_pitches_to_classes
from chord_poc.note_extractor import extract_notes, load_model
from chord_poc.note_stitcher import NoteStitcher

MIDI_NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

# Near the live edge of the buffer, basic-pitch tends to report a note's
# end_s a little before the true buffer boundary (edge effect from lacking
# future context). Without this tolerance, "currently active" notes at the
# live edge are missed entirely and the chord estimate never updates.
ACTIVE_EDGE_TOLERANCE_S = 0.2

# A drum hit can momentarily fragment notes so no chord match is found for a
# single hop, even though the actual chord hasn't changed. Require this many
# consecutive silent/ambiguous hops before declaring "(silence)", and simply
# hold the last confident label through brief ambiguous gaps instead of
# flashing "?".
SILENCE_HOLD_HOPS = 4


def midi_to_name(pitch: int) -> str:
    return f"{MIDI_NOTE_NAMES[pitch % 12]}{pitch // 12 - 1}"


def run(audio_path: str, hop_seconds: float, plot_path: str = None) -> None:
    print(f"Loading {audio_path} ...")
    audio = load_audio(audio_path)
    duration = len(audio) / 22050
    print(f"Duration: {duration:.1f}s, simulating stream with {hop_seconds:.2f}s hops\n")

    model = load_model()
    stitcher = NoteStitcher()
    all_notes = []  # for optional plotting: (start_s, end_s, pitch)
    last_chord_label = None
    ambiguous_hops = 0

    for now_s, buffer in stream_chunks(audio, hop_seconds):
        note_events = extract_notes(buffer, model)

        for n in stitcher.update(now_s, note_events):
            print(f"[{n.start_s:6.2f}s -> {n.end_s:6.2f}s] note {midi_to_name(n.pitch_midi)}")
            all_notes.append((n.start_s, n.end_s, n.pitch_midi))

        active_pitches = [
            p for s, e, p, _a in note_events if s <= now_s and e >= now_s - ACTIVE_EDGE_TOLERANCE_S
        ]
        chord = None
        if active_pitches:
            bass_pc = min(active_pitches) % 12
            chord = match_chord(midi_pitches_to_classes(active_pitches), bass_pc=bass_pc)

        if chord is not None:
            ambiguous_hops = 0
            if chord.label != last_chord_label:
                print(f"  t={now_s:6.2f}s  chord: {chord.label}")
                last_chord_label = chord.label
        else:
            # No confident match this hop (e.g. a drum hit fragmented the
            # notes). Hold the previous label rather than flashing "?", and
            # only declare silence after it persists for a while.
            ambiguous_hops += 1
            if ambiguous_hops >= SILENCE_HOLD_HOPS and last_chord_label != "(silence)":
                print(f"  t={now_s:6.2f}s  chord: (silence)")
                last_chord_label = "(silence)"

    if plot_path:
        _plot_piano_roll(all_notes, duration, plot_path)


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
    parser.add_argument("audio_file", help="Path to a wav/mp3 file to analyze")
    parser.add_argument("--hop", type=float, default=0.5, help="Simulated streaming hop size in seconds")
    parser.add_argument("--plot", type=str, default=None, help="Optional path to save a piano-roll PNG")
    args = parser.parse_args()

    run(args.audio_file, args.hop, args.plot)


if __name__ == "__main__":
    sys.exit(main())
