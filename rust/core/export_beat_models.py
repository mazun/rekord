"""Builds the beat-tracking ONNX models into core/models/. build.rs runs this in a throwaway venv
(see build.rs for the pinned requirements) only when a model file is missing.

- beatthis_small0.onnx: Beat This! small0 (Foscarin, Schlüter, Widmer, ISMIR 2024; MIT,
  https://github.com/CPJKU/beat_this). The published checkpoint is downloaded by the beat_this
  package and exported as is, with a sigmoid on both outputs:
  spect (1, T, 128) -> beat (1, T) = P(beat incl. downbeat), downbeat (1, T) = P(downbeat).
- logmel.onnx: Beat This!'s input features (its LogMelSpect settings: 22050 Hz, n_fft 1024,
  hop 441, 30-11000 Hz, 128 Slaney mels, log1p(1000 x)), with the STFT written as a strided
  Conv1d so that ONNX Runtime needs no STFT op. No learned weights: the tables are the DFT basis,
  the Hann window and torchaudio's mel filterbank. audio (N,) -> spect (1 + (N - 1024) // 441, 128);
  frame i covers samples [441 i, 441 i + 1024), so with 512 zeros prepended it is frame i of
  Beat This!'s own center=True spectrogram.
- beatthis_small0.LICENSE.txt: the beat_this license, shipped next to the model.

Checked against PyTorch in harmonica-repro (beattrack/export_onnx.py): probabilities within
4e-6, mel within 1e-4.

Usage: python export_beat_models.py <models dir>
"""

import math
import shutil
import sys
from importlib import metadata
from pathlib import Path

import torch
import torch.nn as nn
import torchaudio

SR, N_FFT, HOP, N_MELS = 22050, 1024, 441, 128


class LogMelFrames(nn.Module):
    def __init__(self):
        super().__init__()
        ref = torchaudio.transforms.MelSpectrogram(
            sample_rate=SR, n_fft=N_FFT, hop_length=HOP, f_min=30, f_max=11000, n_mels=N_MELS,
            mel_scale="slaney", normalized="frame_length", power=1)
        win = ref.spectrogram.window.double()  # periodic Hann
        k = torch.arange(N_FFT // 2 + 1, dtype=torch.float64)[:, None]
        n = torch.arange(N_FFT, dtype=torch.float64)[None, :]
        ang = 2 * math.pi * k * n / N_FFT
        # normalized="frame_length" divides the STFT by sqrt(n_fft)
        basis = torch.cat([torch.cos(ang), -torch.sin(ang)]) * win / math.sqrt(N_FFT)
        self.register_buffer("basis", basis.float()[:, None, :])  # (2*513, 1, 1024)
        self.register_buffer("fb", ref.mel_scale.fb.float())  # (513, 128)

    def forward(self, audio):
        x = nn.functional.conv1d(audio[None, None, :], self.basis, stride=HOP)[0]
        re, im = x[: N_FFT // 2 + 1], x[N_FFT // 2 + 1:]
        mag = torch.sqrt(re * re + im * im)
        return torch.log1p(1000 * (mag.T @ self.fb))


class ProbModel(nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, spect):
        out = self.model(spect)
        return out["beat"].sigmoid(), out["downbeat"].sigmoid()


def export(module, example, path, inputs, outputs, axes):
    tmp = path.with_suffix(".onnx.part")
    torch.onnx.export(module.eval(), example, tmp, dynamo=False, opset_version=17,
                      input_names=inputs, output_names=outputs, dynamic_axes=axes)
    tmp.replace(path)
    print(f"wrote {path} ({path.stat().st_size / 1e6:.1f} MB)")


def main(out: Path):
    from beat_this.inference import load_model

    out.mkdir(parents=True, exist_ok=True)
    torch.set_grad_enabled(False)
    net = ProbModel(load_model("small0", device="cpu"))
    export(net, (torch.randn(1, 500, N_MELS),), out / "beatthis_small0.onnx", ["spect"], ["beat", "downbeat"],
           {"spect": {1: "T"}, "beat": {1: "T"}, "downbeat": {1: "T"}})
    export(LogMelFrames(), (torch.randn(SR * 2),), out / "logmel.onnx", ["audio"], ["spect"],
           {"audio": {0: "N"}, "spect": {0: "T"}})
    lic = [f for f in metadata.distribution("beat_this").files or [] if f.name.upper().startswith("LICENSE")]
    if lic:
        shutil.copyfile(lic[0].locate(), out / "beatthis_small0.LICENSE.txt")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
