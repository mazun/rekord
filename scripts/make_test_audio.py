"""Generate a synthetic C -> Am -> F -> G piano-ish chord progression as a
smoke-test fixture for the Phase 1 pipeline (not a substitute for real audio)."""

import numpy as np
import soundfile as sf

SR = 44100
CHORD_SECONDS = 2.0

CHORDS = {
    "C": [60, 64, 67],
    "Am": [57, 60, 64],
    "F": [53, 57, 60],
    "G": [55, 59, 62],
}


def midi_to_freq(m: int) -> float:
    return 440.0 * 2 ** ((m - 69) / 12)


def render_chord(pitches, seconds: float) -> np.ndarray:
    t = np.linspace(0, seconds, int(SR * seconds), endpoint=False)
    out = np.zeros_like(t)
    for p in pitches:
        f = midi_to_freq(p)
        partials = sum((1.0 / k) * np.sin(2 * np.pi * f * k * t) for k in (1, 2, 3))
        out += partials
    envelope = np.minimum(1.0, np.minimum(t * 20, (seconds - t) * 20 + 0.05))
    out *= envelope
    return out / max(1, len(pitches))


def main() -> None:
    audio = np.concatenate([render_chord(p, CHORD_SECONDS) for p in CHORDS.values()])
    audio = 0.3 * audio / np.max(np.abs(audio))
    sf.write("sample_audio/progression_C_Am_F_G.wav", audio.astype(np.float32), SR)
    print("wrote sample_audio/progression_C_Am_F_G.wav")


if __name__ == "__main__":
    main()
