# Re:Kord (リコード)

A tool for real-time chord and note recognition from audio playing on your
PC (via WASAPI loopback). Recognition results are shown as a piano roll and
chord names, and the tool also estimates the key and shows Roman-numeral
labels along with melody/bass classification.

## How it works

1. **Note estimation** — Harmonica, an instrument-agnostic note-
   transcription model, estimates polyphonic pitch. Reimplemented, trained
   from scratch and exported to ONNX in the separate harmonica-repro
   project (not the original paper's weights). Three model sizes (small/
   medium/large, default large); runs on ONNX Runtime's DirectML execution
   provider when available, falling back automatically to the small model
   on CPU. A configurable look-ahead (8 or 16 frames, ~0.26s/0.51s) trades
   confirm latency for accuracy.
2. **Chord recognition** — chord names are decided by rule-based matching of
   pitch-class sets against chord templates (roughly 300 tension chords).
3. **Key estimation** — the key is estimated via Krumhansl-Schmuckler-style
   pitch-class correlation (Bellman-Budge profiles), and chords are shown as
   Roman numerals.
4. **Melody/bass** — determined by a symbolic skyline rule (highest/lowest
   note) applied to the stream of confirmed notes.

No machine learning model is used for chord names or key estimation. The
reasoning behind these design decisions is in [CLAUDE.md](CLAUDE.md).

## Requirements

- Windows 11 (Windows-only, since audio capture uses WASAPI loopback)
- A GPU that supports ONNX Runtime's DirectML execution provider (verified
  working on an RTX 5070 Ti)
- [Rust](https://rustup.rs/) and [Node.js](https://nodejs.org/)

## Usage

### Desktop app (piano-roll UI)

```powershell
Set-Location rekord-piano-roll
npm install
npm run tauri dev
```

Capture starts immediately on launch. Whatever audio the system is playing
is what gets analyzed. This is live-input only; there's no UI for loading
files.

### CLI

```bash
cd rust
cargo build --release
./target/release/rekord_engine.exe ../sample_audio/piano_progression_C_Am_F_G.wav
./target/release/rekord_engine.exe --live
```

| Option | Description |
| --- | --- |
| `--live` | Captures the default speaker output via loopback and analyzes it |
| `--duration <seconds>` | Stops after the given number of seconds (for `--live`) |
| `--size <small\|medium\|large>` | Note-transcription model size (default: `large`) |
| `--future <8\|16>` | Look-ahead frames the model uses before deciding a note (default: `8`; higher trades latency for accuracy) |
| `--cpu` | Disables DirectML, forcing the small model onto CPU |
| `--hop <seconds>` | Interval between chunks of captured (or, in file mode, simulated) audio handed off for processing (default: 0.064s) |

`cargo build` downloads Harmonica's ONNX model files into
`rust/core/models/` from the Hugging Face model repo
[mazun/rekord](https://huggingface.co/mazun/rekord), at a pinned revision
and checked against pinned SHA-256s. That repo is private for now, so the
build needs `HF_TOKEN` set to a read-access token (or Hugging Face's token
file, `~/.cache/huggingface/token`).

## Layout

```
rust/
  core/                rekord_core (inference, note extraction, chord/key detection, capture)
  cli/                 rekord_engine (CLI)
rekord-piano-roll/     Tauri + Svelte 5 + Pixi.js piano-roll UI
sample_audio/          Piano recordings used for validation
```

## Limitations

- Key estimation runs over a window of the last few to ten-odd seconds, so
  it can be wrong in sections where the tonic chord hasn't sounded yet.
- Sources with a lot of non-piano sound (e.g. drums) will see more false
  detections.
- Planned future work is tracked in [TODO.md](TODO.md).

## Third party

- The Harmonica note-transcription models are reimplemented and trained
  from scratch (not the paper authors' weights) in the separate
  harmonica-repro project, on data that includes CC BY-NC-SA 4.0 material
  (MAESTRO, and the GAPS dataset's Zenodo release) and, for the 3-part
  model, the CC BY-NC 4.0 GOAT dataset -- so, unlike the rest
  of this repository, they're for non-commercial use only and aren't
  covered by the MIT license below. See the
  [model card](https://huggingface.co/mazun/rekord) for the full breakdown.
  They aren't in this repository; `cargo build` downloads them (see the CLI
  section above).
- Main dependencies: [ort](https://github.com/pykeio/ort),
  [cpal](https://github.com/RustAudio/cpal), [Tauri](https://tauri.app/),
  [Svelte](https://svelte.dev/), [Bits UI](https://bits-ui.com/),
  [Pixi.js](https://pixijs.com/)

## License

[MIT License](LICENSE)

All recordings in `sample_audio/` were produced by the author and are
provided under the same license.

`rekord-piano-roll/src/assets/fonts/DejaVuSans-accidentals.woff2` is
[DejaVu Sans](https://dejavu-fonts.github.io/) reduced to the three glyphs
♭ ♮ ♯ (U+266D-266F); it keeps its own license, not MIT
(`DejaVu-LICENSE.txt` next to it).
