# Rust backend (`rust/`)

Workspace: `core/` (`rekord_core`), `cli/` (`rekord_engine`, thin wrapper),
`beat_dbn/` (`rekord_beat_dbn`).

Full per-topic detail and rationale (read the relevant section only when
touching that area): [docs/notes/rust.md](../docs/notes/rust.md). Chord-layer
design and measurements: [docs/chord-layers.md](../docs/chord-layers.md).

## Rules that bite

- **Note transcription and beat tracking are developed in `harmonica-repro`:
  change behavior there first, regenerate golden vectors, then port.** Don't
  tune the Rust side alone. Golden tests skip when the data is absent.
- Chord/key constants are this project's own and were swept on RWC-Pop
  (odd songs to tune, even to check); tune empirically, never validate with
  synthetic audio.
- Key estimator: only the scoring is textbook; the window/switching rule is a
  streaming adaptation. The user declined a wider window; key misdetection is
  mitigated by a manual override (`TODO.md`), not more tuning.
- **Weights are non-commercial only.** Hosted in private HF repo
  `mazun/rekord`; `build.rs` downloads at pinned `HF_REVISION` + SHA-256.
  New weights: upload to HF first, then update revision and hashes.
- `build.rs` runs `core/export_beat_models.py` (throwaway venv) when the beat
  ONNX files are missing — a deliberate exception to "everything is Rust".
- **DirectML, not CUDA** (RTX 5070 Ti too new). `cpal` for WASAPI loopback.
- The full workspace doesn't link on WSL (`directml`); the user declined a
  workaround — don't "fix" it.
- `rekord_beat_dbn` is a separate crate only so the Tauri dev profile can
  optimize it alone; keep it that way.
- Capture and playback exclude each other (loopback would record playback).

## Map

- Notes: `harmonica_{model,stream,decode}.rs` (file names predate Okarina);
  Okarina S/M/L, all with 3 parts (vocal/bass/other), vocal notes feed only
  tensions. Size is `auto` by default (`gpu_class.rs` tells dGPU from iGPU).
  Latency is structural.
- Chords: `chord_templates.rs`, `chord_layers/` (instant / settled / confirmed
  layers, append-only), `window_chroma` in `pipeline.rs`.
- Key: `key_estimator.rs`. Spelling: `spelling.rs` (frontend mirrors
  `keyPairLabel` — keep in step with `key_pair_name`).
- Beats: `beat_model.rs`, `beat_tracker.rs`, `beat_dbn/`.
- Live session: `run_live_session` in `pipeline.rs` (beat tracker on a second
  thread). Recording: `audio_ring.rs`, `live_capture.rs`, `playback/`.

## Common commands

```bash
cargo test -p rekord_core --release okarina_ -- --test-threads=1   # note golden tests (S/M/L)
cargo test -p rekord_core --release beat_        # beat golden tests
cargo test -p rekord_beat_dbn --release
cargo test -p rekord_core --release --lib
cargo run --release -p rekord_core --example chord_replay -- <raw_dir> <out_dir>
```

Env tuning knobs (`REKORD_CHORD_TUNE_*`, `REKORD_KEY_TUNE`,
`REKORD_CHORD_EVENTS_OUT`) and the `beat_replay` workflow: see the notes file.
