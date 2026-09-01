# chord — real-time chord/melody recognition

Real-time PC-audio (WASAPI loopback) chord and note recognition. Pipeline:
Spotify's pretrained `basic-pitch` (ONNX) for polyphonic pitch/note
transcription, plus rule-based pitch-class-set chord-template matching
(no ML model for chord names — none exists for the ~300 tension-chord
vocabulary this needs, so this was a deliberate substitution for the
original proposal's from-scratch BTC-SL model).

Originally prototyped in Python (deleted once Rust reached parity — see
`git log` for `59669cf`..`1498f47`). Everything now lives in Rust:

- `rust/` — Cargo workspace
  - `core/` (`chord_core` lib) — model inference, streaming note extraction,
    chord matching, WASAPI capture, and the shared live-session driver
    (`pipeline.rs`) used by both the CLI and the Tauri app.
  - `cli/` (`chord_engine` bin) — file-input and `--live` CLI, thin wrapper
    around `chord_core`.
- `chord-piano-roll/` — Tauri + Pixi.js piano-roll UI, **live mode only**
  (no file-input UI; use the CLI for that).

## Build / run

```bash
cd rust
cargo build --release
./target/release/chord_engine.exe sample_audio/piano_progression_C_Am_F_G.wav
./target/release/chord_engine.exe --live
```

```powershell
Set-Location "<repo root>\chord-piano-roll"
npm run tauri dev
```

## Key design decisions (don't re-litigate without reason)

- **DirectML, not CUDA**, for ONNX Runtime GPU inference (`onnx_model.rs`).
  RTX 5070 Ti (Blackwell) is too new for reliable prebuilt CUDA EP wheel
  support; DirectML benchmarked ~3.5–3.9x speedup over CPU and just works.
- **`cpal` for WASAPI loopback**, not a hand-rolled `wasapi` crate binding:
  passing an output device into `build_input_stream` makes cpal set the
  loopback flag internally — no name-based device-matching ambiguity at all
  (the Python prototype's `soundcard` library had a real bug here: it could
  silently bind to the wrong device when a line-input and its loopback
  endpoint shared a display name).
- **Retained-history cap in `streaming_extractor.rs` is time-based**
  (`MAX_RETAINED_SECONDS = 3.3`), not a window count — needed once the
  window-advance stride became configurable (a "window" no longer
  contributes a fixed amount of new data each time). The *only* reason to
  retain more than the newest data is that a note sustained across a
  window boundary needs the next window's data to resolve its true end;
  3.3s matches what the old `MAX_RETAINED_WINDOWS = 2` retained. Don't
  raise this to "be safe" — it was raised to the equivalent of 64 windows
  once by mistake and caused O(T²)-ish per-call cost growth (unbounded
  reprocessing of retained history), confirmed via direct timing
  instrumentation. Verified flat at 0.3–0.7ms/call over 90s at the old
  2-window cap; re-verify if this ever needs to move again.
- **Piano-roll clock is a pure linear function of wall-clock time, offset
  by a fixed known constant** (`total_latency_s`, computed once per
  session in `pipeline.rs` from the actual stride/hop/confirm-margin in
  use and sent on the `PipelineEvent::Started` event — `pianoroll.ts`
  just consumes it, it doesn't compute or hardcode it) — never estimated
  or corrected at runtime. The confirm-delay is structural (window-advance
  stride + capture hop + `NoteStitcher`'s confirm margin), not jitter, so
  anchor-and-extrapolate or rate-correction approaches both produce
  visible warping/acceleration; a fixed offset anchored to the `Started`
  event (fired once the pipeline truly begins, not on button click —
  model load + WASAPI init add unpredictable delay) is the correct model
  and was reached only after several rounds of live-app testing. The
  window length itself (2s) is a hard-fixed property of the ONNX model's
  input shape (confirmed by direct experiment: any other length throws an
  ORT shape-mismatch error immediately) — only how often the window
  advances is configurable, via `StreamingNoteExtractor::with_stride`.
- **`directml` ort feature has no Linux prebuilt binary** — the full
  workspace does not link on WSL (the `core` lib alone builds fine). User
  explicitly declined making it `cfg(windows)`-conditional; this is a
  known, accepted gap, not a bug to fix opportunistically.

## Testing

No unit test suite — validation is empirical, against the real recordings
in `sample_audio/` (`piano_progression_C_Am_F_G.wav`,
`piano_progression_ConG_A7sus4_FM7add9_G6.wav`,
`piano_complex_progression_ConG_A7sus4_FM7add9_G6.wav`). **Never validate
against synthetic sine-wave test audio** — harmonics produce false
tension-chord detections (e.g. spurious add9) that don't occur with real
piano recordings. When changing the extraction/stitching/clock logic,
re-run all three files (and, for UI changes, an actual live session in
the running Tauri app — this UI's timing bugs are only visible in the
real app, not from reading the code) before calling it done.

## Working conventions

- This user reviews UI/timing changes against the **real running app**,
  not code inspection — expect iterative rounds of "watch it live, report
  what's wrong" for anything touching `pianoroll.ts` or the capture
  pipeline's timing.
- Prefer empirical verification (benchmarks, actual builds, actual runs)
  over reasoning from documentation when they conflict — several past
  issues (ort API drift, cpal API drift, the WSL/directml link failure)
  were only resolved by reading source / compiling and observing, not by
  trusting external docs.
