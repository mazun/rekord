"""GPU-capable replacement for basic_pitch.inference.Model, which hardcodes
providers=["CPUExecutionProvider"] and so never uses a GPU even when one is
available. DirectML is used (rather than the CUDA execution provider)
because it works across any DirectX 12 GPU without needing a CUDA/cuDNN
version matched to the installed driver -- important on a GPU new enough
that CUDA execution-provider wheel support is not yet reliable.
"""

import logging
from typing import Dict

import numpy as np
import numpy.typing as npt
import onnxruntime as ort
from basic_pitch import FilenameSuffix, build_icassp_2022_model_path

ONNX_MODEL_PATH = build_icassp_2022_model_path(FilenameSuffix.onnx)

_OUTPUT_NAMES = ["StatefulPartitionedCall:1", "StatefulPartitionedCall:2", "StatefulPartitionedCall:0"]
_OUTPUT_KEYS = ["note", "onset", "contour"]
_INPUT_NAME = "serving_default_input_2:0"

_PREFERRED_PROVIDERS = ["DmlExecutionProvider", "CPUExecutionProvider"]


class OnnxNoteModel:
    """Drop-in replacement for basic_pitch.inference.Model's ONNX predict()
    path, but tries GPU (DirectML) execution first."""

    def __init__(self) -> None:
        available = ort.get_available_providers()
        providers = [p for p in _PREFERRED_PROVIDERS if p in available] or ["CPUExecutionProvider"]
        self.session = ort.InferenceSession(str(ONNX_MODEL_PATH), providers=providers)
        self.active_provider = self.session.get_providers()[0]
        logging.info("basic-pitch ONNX session using provider: %s", self.active_provider)

    def predict(self, x: npt.NDArray[np.float32]) -> Dict[str, npt.NDArray[np.float32]]:
        outputs = self.session.run(_OUTPUT_NAMES, {_INPUT_NAME: x})
        return dict(zip(_OUTPUT_KEYS, outputs))
