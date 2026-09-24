# Re:Kord (リコード)

A tool for real-time chord and note recognition from audio playing on your
PC (via WASAPI loopback). Recognition results are shown as a piano roll and
chord names, and the tool also estimates the key and shows Roman-numeral
labels along with melody/bass classification.

## How it works

1. **Note estimation** — Spotify's [basic-pitch](https://github.com/spotify/basic-pitch)
   (ONNX) estimates polyphonic pitch.
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
| `--latency <seconds>` | Sets the delay until confirmation (affects the model's window-advance stride) |
| `--hop <seconds>` | Interval between chunks of captured audio handed off for processing |

On first build, the basic-pitch model (`nmp.onnx`) is downloaded from
GitHub into `rust/core/models/`.

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

- The [basic-pitch](https://github.com/spotify/basic-pitch) model (Apache
  License 2.0) is not bundled in the repository; it's downloaded at build
  time.
- Main dependencies: [ort](https://github.com/pykeio/ort),
  [cpal](https://github.com/RustAudio/cpal), [Tauri](https://tauri.app/),
  [Svelte](https://svelte.dev/), [Bits UI](https://bits-ui.com/),
  [Pixi.js](https://pixijs.com/)

## License

[MIT License](LICENSE)

All recordings in `sample_audio/` were produced by the author and are
provided under the same license.
