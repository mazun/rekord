"""WASAPI loopback capture: stream whatever's currently playing through the
default output device, as mono float32 chunks at basic-pitch's expected
sample rate -- same shape/dtype/rate as chunks from audio_io.stream_new_chunks,
so the rest of the pipeline (StreamingNoteExtractor etc.) doesn't need to
know whether audio came from a file or a live device.
"""

import queue
import threading
from typing import Iterator

import librosa
import numpy as np
import soundcard as sc

from chord_poc.note_extractor import AUDIO_SAMPLE_RATE

# Recording directly at basic-pitch's target rate (22050) caused the
# backend to report "data discontinuity in recording" -- it's not a rate
# this class of device runs natively. Recording at a standard native-ish
# rate and resampling ourselves in Python is glitch-free.
CAPTURE_SAMPLE_RATE = 48000


def _default_loopback_microphone():
    """sc.get_microphone(name, include_loopback=True) matches by name, but
    an audio interface can expose a real input device with the exact same
    display name as its loopback endpoint (verified on this machine: two
    separate devices both named "Analogue 1/2 (Audient iD14)", one a real
    line input, one the loopback) -- name lookup can silently resolve to
    the wrong one, which looks like "capture works but only picks up
    near-silent noise floor" rather than an obvious error. Filter
    explicitly on the isloopback flag instead."""
    speaker = sc.default_speaker()
    for mic in sc.all_microphones(include_loopback=True):
        if mic.isloopback and mic.name == speaker.name:
            return mic
    raise RuntimeError(f"No loopback device found for default speaker {speaker.name!r}")


def stream_loopback_chunks(hop_seconds: float) -> Iterator[np.ndarray]:
    """Yield mono float32 chunks of `hop_seconds` (resampled to
    AUDIO_SAMPLE_RATE) captured from whatever the default speaker is
    currently playing. Runs forever until the caller stops iterating.

    Recording happens on its own thread that does nothing but call
    rec.record() in a tight loop, handing raw chunks off through a queue.
    Doing the resampling (and, upstream, model inference) in the same
    thread as recording risked the recorder's internal buffer overrunning
    whenever a hop took a little too long to process -- observed directly
    as "data discontinuity in recording" warnings during testing.
    """
    mic = _default_loopback_microphone()
    hop_frames = int(round(hop_seconds * CAPTURE_SAMPLE_RATE))

    chunks: "queue.Queue[np.ndarray]" = queue.Queue(maxsize=50)
    stop_event = threading.Event()

    def record_loop() -> None:
        with mic.recorder(samplerate=CAPTURE_SAMPLE_RATE, channels=2) as rec:
            while not stop_event.is_set():
                chunks.put(rec.record(numframes=hop_frames))

    recorder_thread = threading.Thread(target=record_loop, daemon=True)
    recorder_thread.start()
    try:
        while True:
            stereo = chunks.get()
            mono = stereo.mean(axis=1).astype(np.float32)
            resampled = librosa.resample(mono, orig_sr=CAPTURE_SAMPLE_RATE, target_sr=AUDIO_SAMPLE_RATE)
            yield resampled.astype(np.float32)
    finally:
        stop_event.set()
