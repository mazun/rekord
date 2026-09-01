"""True incremental note extraction: run basic-pitch inference only on newly
arrived audio, stitching windows onto a running posteriorgram, instead of
re-running inference over the whole buffer-so-far on every hop (what
note_extractor.extract_notes does, which is O(elapsed_time) per call).

This mirrors basic_pitch.inference.run_inference's own windowing/trimming
scheme (see unwrap_output) exactly, just applied one window at a time as
audio arrives instead of all at once over a whole file. Because the trim
per window only depends on that window's own output (not on neighboring
windows), processing windows one at a time as they complete is equivalent
to the offline whole-file pass, up to the point the stream has reached.
"""

from typing import Dict, List

import numpy as np
from basic_pitch import note_creation as infer
from basic_pitch.constants import AUDIO_N_SAMPLES, AUDIO_SAMPLE_RATE, FFT_HOP

from chord_poc.note_extractor import HOP_SIZE, N_OVERLAPPING_FRAMES, OVERLAP_LEN, NoteEvent
from chord_poc.onnx_model import OnnxNoteModel

_N_OLAP = N_OVERLAPPING_FRAMES // 2
ANNOTATIONS_FPS = AUDIO_SAMPLE_RATE / FFT_HOP


def _trim_window_output(output_batch: np.ndarray) -> np.ndarray:
    """output_batch: shape (1, n_times_per_window, n_freqs) -> (n_times_trimmed, n_freqs)."""
    trimmed = output_batch[:, _N_OLAP:-_N_OLAP, :]
    shape = trimmed.shape
    return trimmed.reshape(shape[0] * shape[1], shape[2])


class StreamingNoteExtractor:
    def __init__(self, model: OnnxNoteModel) -> None:
        self._model = model
        # Front-pad like basic_pitch.inference.get_audio_input does, so the
        # first real window is centered the same way the offline pass would.
        self._buffer = np.zeros(OVERLAP_LEN // 2, dtype=np.float32)
        self._window_start = 0
        self._frames: Dict[str, list] = {"note": [], "onset": [], "contour": []}
        self._total_frames = 0

    @property
    def covered_seconds(self) -> float:
        """How much of the incoming audio has actually been turned into
        confirmed model output so far. Lags real elapsed time by up to one
        window's hop size (~1.6s) between window completions -- callers
        should treat this, not wall-clock time, as "now" when asking what's
        currently active, since that's the most recent info available."""
        return self._total_frames / ANNOTATIONS_FPS

    def push(self, new_audio: np.ndarray) -> int:
        """Feed newly arrived audio samples. Returns how many windows were
        completed (and thus how much new posteriorgram data is available)."""
        self._buffer = np.concatenate([self._buffer, new_audio.astype(np.float32)])
        completed = 0
        while self._window_start + AUDIO_N_SAMPLES <= len(self._buffer):
            window = self._buffer[self._window_start : self._window_start + AUDIO_N_SAMPLES]
            batch = np.expand_dims(np.expand_dims(window, axis=-1), axis=0)
            for k, v in self._model.predict(batch).items():
                trimmed = _trim_window_output(v)
                self._frames[k].append(trimmed)
            self._total_frames += trimmed.shape[0]
            self._window_start += HOP_SIZE
            completed += 1
        return completed

    def note_events(
        self,
        onset_threshold: float = 0.5,
        frame_threshold: float = 0.3,
        minimum_note_length_ms: float = 127.70,
    ) -> List[NoteEvent]:
        if not self._frames["note"]:
            return []
        model_output = {k: np.concatenate(v, axis=0) for k, v in self._frames.items()}
        min_note_len = int(round(minimum_note_length_ms / 1000 * (AUDIO_SAMPLE_RATE / FFT_HOP)))
        _midi, note_events = infer.model_output_to_notes(
            model_output,
            onset_thresh=onset_threshold,
            frame_thresh=frame_threshold,
            min_note_len=min_note_len,
            melodia_trick=True,
        )
        return [(float(s), float(e), int(p), float(a)) for s, e, p, a, *_ in note_events]
