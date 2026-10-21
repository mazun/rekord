# Re:Kord (リコード)

A tool for real-time chord and note recognition from audio playing on your
PC (via WASAPI loopback). Recognition results are shown as a piano roll and
chord names, and the tool also estimates the key and shows Roman-numeral
labels, tracks the beat and transcribes the drums.

## Download

Windows builds are on the [Releases](https://github.com/mazun/rekord/releases/latest)
page, in two forms:

- **Installer** (`ReKord_X.Y.Z_x64-setup.exe`) — installs per user (no admin
  rights needed). The app checks for a new release on launch and can update
  itself in place ("Update and restart").
- **Portable** (`ReKord_X.Y.Z_x64_portable.zip`) — no installation: unzip the
  `ReKord` folder somewhere you can write to (not Program Files) and run
  `ReKord.exe`. While `portable.txt` sits next to the exe, everything the app
  saves (the recognition models and your settings) goes into a `data` folder
  beside it, so the whole folder can be moved or deleted as is. The app tells
  you when a new release is out and opens its page; to update, unzip the new
  version over the old one, keeping `data`.

Either way, the first launch downloads the recognition models (about 51 MB)
from Hugging Face; later launches and updates reuse them.

The builds aren't code-signed yet, so on first launch Windows SmartScreen may
say "Windows protected your PC". Click **More info** → **Run anyway** to
start it.

## How it works

1. **Note estimation** — Okarina, a note-transcription model for music with
   vocals and drums (an unofficial reimplementation and derivative of
   Harmonica, arXiv:2609.04640), estimates polyphonic pitch and separates it
   into vocal, bass and other parts. Trained from scratch and exported to
   ONNX in the separate harmonica-repro project (not the original paper's
   weights). Three sizes (S/M/L). By default the size is picked from the
   hardware: L on a discrete GPU, M on an integrated GPU or a CPU with 8+
   threads, S on a weaker CPU. It runs on ONNX Runtime's DirectML execution
   provider when available, otherwise on the CPU. A configurable look-ahead
   (8 or 16 frames, ~0.26s/0.51s) trades confirm latency for accuracy.
2. **Chord recognition** — chord names are decided by rule-based matching of
   pitch-class sets against chord templates (roughly 300 tension chords).
3. **Key estimation** — the key is estimated via Krumhansl-Schmuckler-style
   pitch-class correlation (Bellman-Budge profiles), and chords are shown as
   Roman numerals.
4. **Beat tracking** — Beat This! (small0) estimates beats and downbeats,
   decoded by a DBN; they are drawn as beat lines on the roll.
5. **Drum transcription** — ADTOF Frame_RNN transcribes kick, snare, hi-hat,
   tom and cymbal hits into the drum lane (on the CPU).

No machine learning model is used for chord names or key estimation. The
design notes and decision history are in [docs/notes/](docs/notes/).

## Requirements

- Windows 11, x64 (Windows-only, since audio capture uses WASAPI loopback)
- A GPU that supports DirectML is recommended (verified on an RTX 5070 Ti);
  without one the models run on the CPU with a smaller note model

To build from source you also need [Rust](https://rustup.rs/),
[Node.js](https://nodejs.org/) and Python 3.10 or later (see below).

## Building from source

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
| `--size <auto\|s\|m\|l>` | Okarina model size (default: `auto`, chosen from the hardware) |
| `--future <8\|16>` | Look-ahead frames the model uses before deciding a note (default: `8`; higher trades latency for accuracy) |
| `--cpu` | Disables DirectML and runs on the CPU (size M with 8+ threads, otherwise S) |
| `--hop <seconds>` | Interval between chunks of captured (or, in file mode, simulated) audio handed off for processing (default: 0.064s) |

The first `cargo build` puts the ONNX models into `rust/core/models/`, each
at a pinned revision and checked against a pinned SHA-256:

- Okarina and its CQT front end come from the Hugging Face model repo
  [mazun/rekord](https://huggingface.co/mazun/rekord), and the drum model
  from [thebluescreen/adtof-drums-onnx](https://huggingface.co/thebluescreen/adtof-drums-onnx).
  Both are public; no token is needed.
- Beat This! is only published as a PyTorch checkpoint, so the build exports
  it with `rust/core/export_beat_models.py`, in a throwaway virtual
  environment (about 1 GB of CPU-only packages, deleted afterwards). This is
  what needs Python; set `REKORD_PYTHON` to pick the interpreter.

Releases are built by GitHub Actions when a `vX.Y.Z` tag is pushed (see
[CLAUDE.md](CLAUDE.md#release)).

## Layout

```
rust/
  core/                rekord_core (inference, note/drum/beat extraction, chord/key detection, capture, playback)
  cli/                 rekord_engine (CLI)
  beat_dbn/            rekord_beat_dbn (DBN beat decoder)
rekord-piano-roll/     Tauri + Svelte 5 + Pixi.js piano-roll UI
docs/                  Design notes
sample_audio/          Piano recordings used for validation
```

## Limitations

- Key estimation runs over a window of the last few to ten-odd seconds, so
  it can be wrong in sections where the tonic chord hasn't sounded yet.
- Sources with a lot of non-piano sound (e.g. drums) will see more false
  detections.
- Planned future work is tracked in [TODO.md](TODO.md).

## Third party

- The Okarina note-transcription models (S/M/L) are an unofficial
  reimplementation and derivative of Harmonica, trained from scratch (not
  the paper authors' weights) in the separate harmonica-repro project, on
  data that includes CC BY-NC-SA 4.0 material (MAESTRO, and the GAPS
  dataset's Zenodo release) and the CC BY-NC 4.0 GOAT dataset, with bass
  labels made by MuScriptor (Kyutai / Mirelo, weights CC BY-NC 4.0) -- so, unlike
  the rest of this repository, they're for non-commercial use only and aren't
  covered by the MIT license below. See the
  [model card](https://huggingface.co/mazun/rekord) for the full breakdown.
  They aren't in this repository or the installer; the app downloads them on
  first launch, and `cargo build` downloads them for a checkout.
- The drum model is ADTOF Frame_RNN (Zehren et al.), CC BY-NC-SA 4.0 —
  likewise non-commercial only — used through a community ONNX conversion
  ([thebluescreen/adtof-drums-onnx](https://huggingface.co/thebluescreen/adtof-drums-onnx)).
- The beat model is Beat This! small0 (Foscarin, Schlüter, Widmer, CPJKU),
  MIT.
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
