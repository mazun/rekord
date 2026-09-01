"""Load an audio file and simulate a live stream by yielding a growing buffer."""

from typing import Iterator, Tuple

import librosa
import numpy as np

from basic_pitch.constants import AUDIO_SAMPLE_RATE


def load_audio(path: str) -> np.ndarray:
    """Load an audio file as mono float32, resampled to basic-pitch's expected rate."""
    audio, _ = librosa.load(path, sr=AUDIO_SAMPLE_RATE, mono=True)
    return audio.astype(np.float32)


def stream_chunks(audio: np.ndarray, hop_seconds: float) -> Iterator[Tuple[float, np.ndarray]]:
    """Simulate a live stream by yielding (elapsed_seconds, buffer_so_far).

    Each step reveals `hop_seconds` more audio, mimicking how a real capture
    pipeline would hand off newly arrived samples at a fixed cadence.
    """
    hop_samples = int(round(hop_seconds * AUDIO_SAMPLE_RATE))
    total = len(audio)
    for end in range(hop_samples, total + hop_samples, hop_samples):
        end = min(end, total)
        yield end / AUDIO_SAMPLE_RATE, audio[:end]
