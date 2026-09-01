"""Thin wrapper around basic-pitch's ONNX model for running inference on an
in-memory audio buffer (rather than a file path, which is all the upstream
`basic_pitch.inference.run_inference` supports).

Phase 1 re-runs inference on the whole buffer-so-far on every hop. That's
O(buffer_length) per call and only viable for short validation clips -- a
real streaming implementation would instead run inference on just the newest
window and stitch it onto prior output (basic_pitch.inference.run_inference
already does this trick across an entire file offline; see unwrap_output).
"""

from typing import Dict, List, Tuple

import numpy as np
from basic_pitch import FilenameSuffix, build_icassp_2022_model_path
from basic_pitch.inference import Model, get_audio_input, window_audio_file, unwrap_output
from basic_pitch.constants import AUDIO_N_SAMPLES, AUDIO_SAMPLE_RATE, FFT_HOP
from basic_pitch import note_creation as infer

NoteEvent = Tuple[float, float, int, float]  # start_s, end_s, pitch_midi, amplitude

ONNX_MODEL_PATH = build_icassp_2022_model_path(FilenameSuffix.onnx)

# Same overlap basic_pitch's own offline pipeline uses (see inference.run_inference).
N_OVERLAPPING_FRAMES = 30
OVERLAP_LEN = N_OVERLAPPING_FRAMES * FFT_HOP
HOP_SIZE = AUDIO_N_SAMPLES - OVERLAP_LEN


def load_model() -> Model:
    return Model(ONNX_MODEL_PATH)


def _run_inference_on_array(audio: np.ndarray, model: Model) -> Dict[str, np.ndarray]:
    """Same windowing/stitching as basic_pitch.inference.run_inference, but
    takes an in-memory array instead of loading a file from disk."""
    original_length = audio.shape[0]
    padded = np.concatenate([np.zeros((OVERLAP_LEN // 2,), dtype=np.float32), audio])

    output: Dict[str, list] = {"note": [], "onset": [], "contour": []}
    for window, _window_time in window_audio_file(padded, HOP_SIZE):
        for k, v in model.predict(np.expand_dims(window, axis=0)).items():
            output[k].append(v)

    return {
        k: unwrap_output(np.concatenate(v), original_length, N_OVERLAPPING_FRAMES)
        for k, v in output.items()
    }


def extract_notes(
    audio: np.ndarray,
    model: Model,
    onset_threshold: float = 0.5,
    frame_threshold: float = 0.3,
    minimum_note_length_ms: float = 127.70,
) -> List[NoteEvent]:
    """Run basic-pitch on the full buffer-so-far and return note events."""
    model_output = _run_inference_on_array(audio, model)
    min_note_len = int(round(minimum_note_length_ms / 1000 * (AUDIO_SAMPLE_RATE / FFT_HOP)))
    _midi, note_events = infer.model_output_to_notes(
        model_output,
        onset_thresh=onset_threshold,
        frame_thresh=frame_threshold,
        min_note_len=min_note_len,
        melodia_trick=True,
    )
    return [(float(s), float(e), int(p), float(a)) for s, e, p, a, *_ in note_events]
