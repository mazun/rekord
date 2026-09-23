# Re:Kord (リコード) — real-time chord/melody recognition

"Re:Kord" is the project's display name (window title, HTML title). Code
identifiers use `rekord` (crates `rekord_core`/`rekord_engine`, the
`rekord-piano-roll/` app directory, Tauri `productName` `ReKord` since `:`
is invalid in Windows filenames, bundle `identifier`
`com.mazun.rekord-piano-roll`). Musical-domain names (`chord_templates`,
`ChordDisplay`, …) intentionally stay "chord" — they mean chords, not the
project. The repo's root folder is still named `chord`.

Real-time PC-audio (WASAPI loopback) chord and note recognition. Pipeline:
Spotify's pretrained `basic-pitch` (ONNX) for polyphonic pitch/note
transcription, plus rule-based pitch-class-set chord-template matching
(no ML model for chord names — none exists for the ~300 tension-chord
vocabulary this needs, so this was a deliberate substitution for the
original proposal's from-scratch BTC-SL model).

Originally prototyped in Python (deleted once Rust reached parity — see
`git log` for `59669cf`..`1498f47`). Everything now lives in Rust:

- `rust/` — Cargo workspace
  - `core/` (`rekord_core` lib) — model inference, streaming note extraction,
    chord matching, key estimation + Roman-numeral labeling
    (`key_estimator.rs`), WASAPI capture, and the shared live-session
    driver (`pipeline.rs`) used by both the CLI and the Tauri app.
  - `cli/` (`rekord_engine` bin) — file-input and `--live` CLI, thin wrapper
    around `rekord_core`.
- `rekord-piano-roll/` — Tauri + Svelte 5 (Bits UI for headless widgets) +
  Pixi.js piano-roll UI, **live mode only** (no file-input UI; use the CLI
  for that). UI state lives in runes modules (`pipeline.svelte.ts`,
  `settings.svelte.ts`); `pianoroll.ts` stays a plain imperative Pixi class
  wrapped by `PianoRollView.svelte`, and note/`Started` events go straight
  to it rather than through reactive state. Capture starts automatically
  on launch.

See `TODO.md` for the current backlog of planned features — remove an
item from it once it's actually implemented, rather than leaving it to
rot as a stale wishlist.

## Build / run

```bash
cd rust
cargo build --release
./target/release/rekord_engine.exe sample_audio/piano_progression_C_Am_F_G.wav
./target/release/rekord_engine.exe --live
```

```powershell
Set-Location rekord-piano-roll   # from the repo root
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
- **Key estimation (`key_estimator.rs`) is Krumhansl-Schmuckler-style
  pitch-class-profile correlation**, not ML — same reasoning as chord
  naming above: this is a well-established method for exactly this
  problem, and there's no key-labeled corpus in this project to train
  anything else against. Uses **Bellman-Budge profiles**, not the more
  commonly-cited Aarden-Essen (its own source casts doubt on its minor
  weights) or Temperley-Kostka-Payne (highest published accuracy, but
  documented to bias toward calling minor keys their relative major —
  exactly the distinction this feature needs right). Only the core
  scoring (histogram + Pearson correlation against 24 profile rotations)
  is the textbook algorithm; the rolling window (`WINDOW_S`) and
  hysteresis (`SWITCH_MARGIN`/`MIN_HOLD_S`) are this project's own
  real-time adaptations with no canonical prescription, since the
  original algorithm has no streaming formulation at all — don't treat
  those as sacred the way the scoring itself should be.
- **Roman numerals are always spelled against the key's governing major
  scale** (a minor key's relative major, not its own tonic), and the key
  name always **shows both readings of the major/minor pair** (e.g.
  "C Major (A Minor)"), never switching which one displays depending on
  which the correlator locked onto. Deliberate: relative major/minor
  share all seven diatonic pitch classes, so that call is the noisiest
  part of the estimate, and this makes the noise invisible instead of
  surfacing as a flickering label.
- **A short-window key misdetection is an expected limitation, not
  necessarily a bug** — confirmed via live loopback testing (2026-09-03):
  a real G-major song displayed as "C Major (A Minor)" because within the
  12s rolling window, the tonic (G) chord simply hadn't sounded yet. K-S
  correlation's published ~90%+ accuracy figures are measured against
  whole pieces or large segments, not few-second windows. Deliberately
  not "fixed" by widening `WINDOW_S` (trades away modulation-tracking
  speed, and user declined this trade) — a manual key-override UI is the
  intended mitigation instead (see `TODO.md`), not further tuning of the
  automatic estimator.
- **`KeyEstimator` is fed from already-deduplicated `ConfirmedNote`s**,
  not the raw `note_events()` list, for the same O(T²) reason as the
  chord-matching path — each note should contribute to the rolling
  chroma histogram exactly once. Its stale-entry pruning must use
  `VecDeque::retain` over the whole deque, not a front-only pop loop:
  `observe()` is called in *confirmation* order (`NoteStitcher` releases
  a note once its end_s clears the confirm margin), not start_s order, so
  a long-held bass note routinely arrives (and gets pushed) after a
  shorter later-starting treble note already at the front — a front-only
  pop would leave that bass note's stale entry stuck behind the treble
  note forever, growing `history` without bound for the rest of the
  session. This was a real bug (not hypothetical) caught via live testing
  as a session that "worked at first, then gradually became unresponsive".
- **Melody/bass line labeling (`melody_bass.rs`) is a symbolic skyline rule
  over the live `NoteConfirmed` stream, not a source-separation model.** An
  earlier attempt used Demucs (source separation) for this and was kept on
  a branch, not merged — see `git log` for
  `feature/on-demand-analysis`: separating audio into stems and running
  basic-pitch on each just relabels notes basic-pitch would already
  detect, it doesn't recover any new ones, so the added complexity (a
  Python subprocess, since htdemucs has no working ONNX export) wasn't
  worth it for what's ultimately a labeling problem over an already-known
  note set. The skyline rule (highest/lowest sounding note) is the classic
  symbolic-music melody-extraction baseline (Uitdenbogerd & Zobel); the
  register cutoffs (`BASS_MAX_PITCH`/`MELODY_MIN_PITCH`), the relative-
  velocity filter, and the voice-leading jump gate are this project's own
  additions with no canonical prescription — tune those empirically, but
  don't casually swap out the core extreme-note rule itself without reason.
  Measured against Demucs-stem notes as pseudo ground truth on one real
  recording (2026-09-21): bass ~84% precision/68% recall, melody ~55%
  precision/26% recall (vs. a ~35% any-note baseline) — one song, method
  not re-verified since, worth re-running before trusting further tuning.
  `melody`/`bass` on `PipelineEvent::NoteConfirmed` aren't mutually
  exclusive in the backend (a lone low note can trivially satisfy both);
  the piano-roll frontend resolves that by always preferring melody when
  both are set, rather than the backend picking one.

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

**All three `sample_audio/` files are in C major** — none of them
exercise `key_estimator.rs`'s minor-key branch or any non-C-major key.
The G-major misdetection documented above was only caught via an ad hoc
live loopback session, not the file-based validation flow above. When
changing key-estimation logic, that gap still needs covering by ear
against real playing in a non-C key, not just a re-run of these three
files.

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
- **WASAPI loopback capture supports multiple simultaneous taps** — a
  second `rekord_engine.exe --live` instance can run concurrently with the
  Tauri app and listen to the same system audio. Used live (2026-09-03)
  to diagnose a key-misdetection report while the user was actively
  playing a song, by adding a temporary env-var-gated `eprintln!` of
  per-key correlation scores to `key_estimator.rs`, listening in via the
  CLI, and reading the diagnostic output directly — instead of asking the
  user to relay live UI behavior secondhand. Remove any such temporary
  instrumentation once done; don't leave it committed.
