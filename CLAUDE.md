# Re:Kord (リコード) — real-time chord/melody recognition

"Re:Kord" is the display name (window title, HTML title). Code identifiers use
`rekord` (crates `rekord_core`/`rekord_engine`, the `rekord-piano-roll/` app,
Tauri `productName` `ReKord` since `:` is invalid in Windows filenames, bundle
`identifier` `com.mazun.rekord-piano-roll`). Musical-domain names
(`chord_templates`, `ChordDisplay`, …) intentionally stay "chord". The repo's
root folder is still named `chord`.

Real-time PC-audio (WASAPI loopback) chord and note recognition: the Harmonica
transcription model (ONNX, trained in the separate `harmonica-repro` project)
for polyphonic notes, plus rule-based pitch-class chord-template matching (no
ML model for chord names — none exists for the ~300 tension-chord vocabulary).
The old Python prototype was deleted once Rust reached parity (`git log`
`59669cf`..`1498f47`).

## Layout (details in the per-directory CLAUDE.md files)

- `rust/` — Cargo workspace: `core/` (`rekord_core` lib: inference, note
  extraction, chords, key, beats, capture, playback, and the live-session driver
  `pipeline.rs`), `cli/` (`rekord_engine` bin, file input and `--live`),
  `beat_dbn/`. → [rust/CLAUDE.md](rust/CLAUDE.md)
- `rekord-piano-roll/` — Tauri + Svelte 5 + Pixi.js UI, live mode only
  (use the CLI for files), plus replay of the last 5 minutes.
  → [rekord-piano-roll/CLAUDE.md](rekord-piano-roll/CLAUDE.md)
- `docs/chord-layers.md` — design and evaluation history of the 3-layer chord
  estimate (Japanese).
- `TODO.md` — backlog. Remove an item once implemented.
- `sample_audio/` — three real C-major piano recordings used for validation.

## Build / run

```bash
cd rust
cargo build --release
./target/release/rekord_engine.exe ../sample_audio/piano_progression_C_Am_F_G.wav
./target/release/rekord_engine.exe --live
```

```powershell
Set-Location rekord-piano-roll   # from the repo root
npm run tauri dev
```

## Testing

- Golden-vector tests against the Python reference in `harmonica-repro` cover
  note transcription and beat tracking (they skip when the golden data is
  absent; see rust/CLAUDE.md). Small pure-function tests exist for the playback
  pieces (`cargo test -p rekord_core --release --lib`) and the chord lane
  (`node rekord-piano-roll/dev/chordlane.test.ts`).
- Everything else is validated empirically against `sample_audio/`. **Never
  validate with synthetic sine-wave audio** — harmonics cause false tension
  detections (e.g. spurious add9). When changing note streaming/decode/clock
  logic, re-run all three files (with the Mix model they give the same chord
  sequence as single-part large) and, for UI changes, run a real live session in
  the Tauri app; its timing bugs are not visible from reading code.
- All three files are in C major: nothing there exercises the key estimator's
  minor/non-C branches. Cover key changes by ear on real playing in another key.

## Working conventions

- This user reviews UI/timing changes in the **real running app**, not by code
  inspection — expect iterative "watch it live, report what's wrong" rounds for
  anything touching `pianoroll.ts` or capture timing.
- Prefer empirical verification (benchmarks, builds, runs) over documentation
  when they conflict (ort/cpal API drift and the WSL/directml link failure were
  only resolved that way).
- **WASAPI loopback allows several simultaneous taps**: a second
  `rekord_engine.exe --live` can run beside the Tauri app and hear the same
  audio. Use it to diagnose live behavior yourself (e.g. a temporary env-gated
  `eprintln!`) instead of asking the user to relay it; remove such
  instrumentation afterwards, never commit it.
