# Rust backend (`rust/`)

Workspace: `core/` (`rekord_core`), `cli/` (`rekord_engine`, thin wrapper over
core), `beat_dbn/` (`rekord_beat_dbn`).

**Models developed in `harmonica-repro` (note transcription, beat tracking):
change behavior there first, regenerate its golden vectors, then port. Don't
tune the Rust side alone.** Both ports match the reference on the golden
vectors; the tests skip when the golden data is absent.

## Note transcription (Okarina S/M/L; `okarina_{model,stream,decode}.rs`)

Okarina is an unofficial Harmonica derivative (the module names predate it).
Spec, accuracy, speed, parameter choices: `docs/phase5-report.md`,
`docs/mix-s2-report.md` (L) and `docs/okarina-sm-report.md` (S/M, sizes,
hand-over) in harmonica-repro (reference `harmonica/infer.py::predict_song_stream`,
`harmonica/decode.py`).

- Streaming: 16 kHz mono, 32 ms frames; every 8 frames (0.256 s) one 5 s window
  (156 frames) ending `f` frames after the block, audio after "now" zeroed, CQT
  rows before the stream start zeroed, output rows `[148-f, 156-f)`. No LSTM
  state is carried. `f` = 8 (default) or 16 is a user setting; longer doesn't
  help, 0 loses a lot of accuracy.
- **Sizes** `okarina_{s,m,l}.onnx` (`ModelSize::{Small,Medium,Large}`; the
  files keep the training side's names in their `size`/`train_run` metadata,
  Re:Kord tells them apart by file name). The setting is `ModelChoice`:
  **`Auto` (default)** or a fixed size. Budget is 0.256 s per block; measured
  on the dev PC (CQT + model): L 17 ms on the dGPU but 278 ms on 8 CPU threads
  and 273 ms (model only) on the iGPU; M ~90 ms on 8 CPU threads, ~80 ms on the
  iGPU; S ~40 ms on 4 CPU threads. Times on weak CPUs/iGPUs are unmeasured.
  - `Auto` + DirectML: dGPU → L, iGPU → M (`gpu_class.rs`: first
    non-software DXGI adapter in high-performance order — the one DirectML's
    device 0 uses; dGPU = at least 1 GiB dedicated VRAM). No hardware adapter →
    CPU rule.
  - CPU rule (`cpu_size`): 8+ threads → M, else S. It also applies when
    DirectML fails to load, and caps a fixed choice (L chosen but no DirectML →
    M or S): running over budget on the CPU would silently break real time.
  - A fixed L without a dGPU is loaded as asked; `NoteModelInfo.over_budget`
    makes the settings page warn. `load` logs the size and adapter name to
    stderr (check it on a new machine).
- All three output `[1, 3, T, 88]` per head, parts vocal/bass/other. `parts`
  and `thresholds_f8`/`thresholds_f16` (per part `[th_on, th_sus]`) come from
  the ONNX metadata, not constants here (they differ per size; the vocal th_sus
  differs between f8 and f16 for M only (.45 / .40); S is .20/.35 and L .25/.40
  at both); the part order is checked at load
  (`part_thresholds` picks by `future_frames`). `NoteTranscriber` runs one
  `StreamingDecoder` per part; every `ConfirmedNote`/`NoteEvent`/
  `NoteConfirmed` carries its `Part`.
  The thresholds were partly hand-lowered (bass/other th_sus) because the CV
  picks cut held notes short.
- The Harmonica single-part models (small/medium/large_uni) and
  `mix_large_v1/v2.onnx` names were retired. 2026-10-13/14: `okarina_{s,m,l}.onnx`
  are now `mix_{small,medium,large}_v3.onnx` (runs `s2g_a_*`: the vocal teacher
  is openvpi GAME's single notes instead of the earlier one; same structure,
  speed and I/O). RWC-Pop vocal COnP50 / COnPOff: S .382/.174 → .531/.258,
  M .406/.188 → .575/.304, L .422/.193 → .602/.331; bass/other within ±0.003
  (S/M bass COnPOff −.005/−.008). Vocal th_sus: S .30/.35 → .35 (f8/f16),
  M .35 → .45/.40, L .35/.30 → .40. The previous files are kept locally as
  `okarina_{s,m,l}_prev.onnx` (byte-identical to the old v2 exports) for A/B
  listening; build.rs ignores them. Golden data: `runs/okarina_deploy_g/{small,
  medium}`, `runs/s2g_deploy`. GAME licensing: code MIT, model files CC BY-NC-SA
  4.0 (v1.0.0 release notes); compatible with our weights' license, noted in the
  model card. HF revision: see `HF_REVISION`.
  Known behavior, not a bug: low notes of solo instruments are split between bass
  and other, so a pitch can appear in both; bass-part notes can be short
  fragments. `velocity` is not on a common scale across parts, which skews the
  key estimator's weighting toward bass — not re-tuned.
- The roll colors by part (the symbolic melody/bass skyline, `melody_bass.rs`,
  was removed with the single-part models; it is in git history); the chord is
  named from bass + other only — **vocal notes feed only the tensions** (`evaluate_active` in `pipeline.rs`), since sung
  melody is mostly non-chord tones (vocal-only moments read as chord silence).
  The user chose to leave a bass-part-only slash rule because chord display
  beyond tensions is to be rebuilt around a separate chord model.
- Decode is exact in streaming form and never revises a note, so there is no
  confirm margin and no stitcher: a note is emitted once, when it ends.
  `NoteTranscriber::take_started_notes` reports newly opened notes as
  `PipelineEvent::NoteStarted` (drawn growing until the matching `NoteConfirmed`).
  `velocity` = onset probability at the onset frame. Chord matching takes note
  ends as exact (no grace period; it only let the previous chord linger).
- Display latency is structural: `total_latency_s = (8 + f)·0.032 + capture hop
  (64 ms) + 0.3` (0.88 s at f = 8). The frontend ignores it (see its CLAUDE.md).
- Finished notes are kept 3.3 s (`RETAINED_SECONDS`) as context for chord
  matching; this only bounds `note_events()`.
- Tests: `cargo test -p rekord_core --release okarina_ -- --test-threads=1`
  (`tests/okarina_golden.rs`, 10 tests, ~2.5 min). Per size two datasets from
  harmonica-repro (`HARMONICA_REPRO_DIR` overrides the checkout path): S/M in
  `runs/okarina_deploy_g/{small,medium}/`, L in `runs/s2g_deploy/`, each with
  `golden` (GuitarSet, notes only in `other`) and `golden_rwc001` (20 s of
  RWC-Pop 001, all parts; RWC is not redistributable, only ever read from
  harmonica-repro). The files there keep the internal names (`mix_small_*`
  etc.); the test maps them. A size whose model or golden dir is missing is
  skipped. Tolerance 1e-4 CPU / 1e-3 DirectML (observed < 5e-5 on CPU), but
  loose in the first blocks of a stream (window mostly zeroed
  rows amplifies the difference between pyke's statically linked ONNX Runtime
  and Microsoft's); notes are identical.
- **Weights are non-commercial only** (training data include CC BY-NC(-SA)
  material and the user's own commercial CDs; see the model card). Hosted in the
  private HF repo `mazun/rekord`; `build.rs` downloads at the pinned commit
  (`HF_REVISION`) and checks pinned SHA-256s; `core/models/` is gitignored.
  New weights: upload to HF first, then update `HF_REVISION` and the hashes.
  The old file names (`mix_large_v1/v2.onnx`, `*_uni_*.onnx`) stay in the HF
  repo for older revisions.

## Chord naming (`chord_templates.rs`, `window_chroma` in `pipeline.rs`, `chord_layers/`)

Windowed pitch-class-energy match (`match_chroma`), not the instant's note set:
non-vocal notes overlapping the window are summed per pitch class by overlap
length; a template scores inside share − 0.7·outside share − 3·(shortfall of
each tone below a 6% share) − 0.1 per tone − a per-quality prior (rarer chord =
more evidence needed) + 0.3·bass part's share on the root. The slash bass
(`ChordMatch::bass_pc`) is the bass part's dominant pitch class in the window.
An `aug` triad reads the same from each of its three tones, so its root is a tie-break:
the one with the bass a whole tone above it (B♭aug/C, the "Blackadder chord") gets a
small bonus, else the root-position bass bonus decides. That whole-tone-above tone also
counts as *inside* the aug template, weighted by its share of the bass part, so E♭ G B over an
F bass is E♭aug/F rather than G7/F with a stray note (docs/chord-layers.md §14).
With a known key, a winning bare fifth `5` on a diatonic root whose 3rd isn't
sounding is renamed to the key's major/minor triad. Constants are in code;
every number was swept on RWC-Pop (odd songs to tune, even to check) with the
tools in harmonica-repro (`tools/chord_eval/`, `runs/chord_rwc_v2/`) — they are
this project's own. RWC labels contain almost no add9/9/11/sus2/power chords,
so ornate names score as wrong there (expected).

**Chord layers** (the design and every measurement: `docs/chord-layers.md`):
append-only layers of increasing delay — **instant** (~0.16 s), **settled**
(~0.55 s) and **confirmed** (2 s) — each reporting where its chord changes plus
how far it has decided (`decided_s`); a layer never revises what it reported.
- `hold.rs` `HoldLayer` (window match + hold + backdate + score margin; layers
  are `HoldParams::instant()`/`::settled()`), `confirmed.rs` `ConfirmedLayer`
  (fixed-lag Viterbi over (root, quality, bass) + silence from the same
  `score_templates`; a bare fifth gets the 3rd the song showed for that root,
  else the key's, only as a tie-break), `evidence.rs`, `engine.rs`
  `ChordEngine::standard()` runs all at the same checkpoints and emits
  `PipelineEvent::ChordLayer { layer, changes, decided_s }`, `ActiveNotes`,
  `KeyChange`; `Started` carries the chord vocabulary.
- The instant layer has slash chords on; a change of the bass alone is held 0.4 s
  (`bass_hold_s`) because the score margin doesn't cover the slash bass.
- Offline loop without the model: `cargo run --release -p rekord_core --example
  chord_replay -- <raw_dir> <out_dir>` replays the chord logic over a
  `rekord_engine` file-mode stdout (harmonica-repro `runs/chord_replay.sh`);
  `REKORD_CHORD_EVENTS_OUT` writes a log for the frontend's `dev/chords.html`,
  `REKORD_CHORD_TUNE="layer=confirmed,..."` tunes offline.
- Tune in the running app: env vars `REKORD_CHORD_TUNE_INSTANT` /
  `_SETTLED` (`h=0.05,c=0`: `HoldParams::apply_tune`) and
  `REKORD_CHORD_TUNE_CONFIRMED` (`D=1.5`, or `off`). The CLI prints one layer by
  default (`--chord-layer instant|settled|confirmed|all`).
- Wrong major/minor on a chord whose 3rd is missing mostly comes from the key
  lagging a modulation (below).

## Key estimation (`key_estimator.rs`)

Krumhansl-Schmuckler-style pitch-class-profile correlation, not ML (no
key-labeled corpus to train on), with **Bellman-Budge profiles** (Aarden-Essen's
source doubts its minor weights; Temperley-Kostka-Payne biases minor keys to
their relative major — exactly the call this needs right). Only the scoring
(histogram + Pearson against 24 rotations) is textbook; the rolling window and
switching rule are this project's own streaming adaptation, not sacred. Current
tuning: 8 s window with 2 s exponential decay, 0.1 penalty on minor keys, and
switching by accumulated evidence over relative-major classes (`estimate_cusum`);
the old margin/hold rule is still reachable via
`REKORD_KEY_TUNE="l=0,w=12,d=0,mp=0"`. Chord naming asks `naming_key()`, which
goes with a challenger after half the gain. Details: `docs/chord-layers.md` §11.

- Roman numerals are spelled against the key's governing major scale, and the
  key name always shows both readings ("C Major (A Minor)") — the major/minor
  call is the noisiest part, so this hides the noise instead of flickering.
- A short-window misdetection (e.g. G major read as C major before the tonic
  sounded) is an expected limitation. The user declined a wider window; the
  intended mitigation is a manual key override (`TODO.md`), not more tuning.
- `KeyEstimator` is fed from deduplicated `ConfirmedNote`s so each note counts
  once. `observe()` arrives in *confirmation* order, not `start_s` order, so
  stale-entry pruning must `VecDeque::retain` over the whole deque — a
  front-only pop leaked entries forever (a real bug: "worked, then gradually
  became unresponsive").

## Spelling (`spelling.rs`)

Three modes — Auto / always ♯ / always ♭ (`Accidental`, setting
`chord.accidental`, live via `set_accidental`; each chord layer re-announces its
latest chord in the new spelling, past chords keep theirs). Auto = most natural
on a piano score: dim/dim7/m7b5 roots always ♯; otherwise the root is a scale
degree of the estimated key; a fixed table (flats, but F♯, C♯m/F♯m/G♯m) when no
key; slash basses by letter arithmetic from the spelled root (E/G♯, never
G♭/A♯); key names are G♭-side majors with the fewest-accidentals relative minor.
C♭/F♭/E♯/B♯ and double accidentals never appear. The frontend mirrors only the
key-pair table (`keyPairLabel` in `pipeline.svelte.ts`); keep it in step with
`key_pair_name`. Because Auto follows the key, a key misdetection also respells
chords (accepted). Not yet checked by ear in the live app.

## Beat tracking (`beat_model.rs`, `beat_tracker.rs`, `beat_dbn/`)

Beat This! small0 + madmom's DBN, ported from harmonica-repro
(`beattrack/online.py`, `beattrack/dbn_ref.py`; evaluation in
`docs/beat-report.md`). Tests: `cargo test -p rekord_core --release beat_` and
`cargo test -p rekord_beat_dbn --release` (golden dir
`harmonica-repro/runs/beat/golden`, override `BEAT_GOLDEN_DIR`).

- `run_live_session` (`pipeline.rs`) runs the note pipeline on the calling thread
  and `run_beat_tracker` on a scoped second thread that gets a copy of every
  capture chunk (~40% of a core; must not delay notes). So `on_event` is
  `FnMut + Send`, called through a `Mutex`. A failing beat model only disables
  the lines.
- `BeatTracker::bounded` keeps the last 30 s (`Tail` buffers); `::new` keeps
  everything and is what the golden tests compare; a test checks both give
  identical lines.
- `PipelineEvent::BeatLines { from_s, committed, provisional, predicted }` is a
  snapshot of every line in the last 30 s, sent when a line changed (~3/s); it
  replaces what the frontend has after `from_s`. The three layers are the same
  beats at increasing delay (predicted ≈0.3 s, provisional ≈1.2 s, committed
  ≈5.3 s after the beat). Provisional lines now mostly firm up just after the
  playhead (accepted). Lines vanishing behind the playhead around song changes
  and Beat This! locking onto the off-beat are expected.
- **`rekord_beat_dbn` is its own crate only so the Tauri dev profile can
  optimize it alone** (`[profile.dev.package.rekord_beat_dbn]` in
  `src-tauri/Cargo.toml`): unoptimized, the float64 Viterbi cost 440 ms CPU per
  second of audio (17 ms optimized). The user preferred this to optimizing all
  of `rekord_core`.
- Checking timing without audio: `cargo run --release -p rekord_core --example
  beat_replay -- ../../harmonica-repro/runs/beat/stream/audio/s00.wav
  ../rekord-piano-roll/dev/replay-events.jsonl --seconds 90` (from `rust/`)
  records what the frontend would receive; analyze it with harmonica-repro's
  `python -m beattrack.replay_analyze … --write-ref` (WSL venv-beatnet); render
  with the frontend's `dev/replay.html` (see its CLAUDE.md).

## Build / platform decisions

- **Exception to "everything is Rust": `build.rs` runs `core/export_beat_models.py`
  in a throwaway venv when `models/beatthis_small0.onnx` or `models/logmel.onnx`
  is missing** (Beat This! is only published as a PyTorch checkpoint, MIT; the
  user chose converting at build time over hosting a converted file).
  `logmel.onnx` has no learned weights and is generated by the same script.
  Needs Python ≥ 3.10 (or `REKORD_PYTHON`); the first build downloads ~1 GB of
  CPU-only PyTorch (~1.5 min), then deletes the venv.
- **DirectML, not CUDA**, for GPU inference: the RTX 5070 Ti (Blackwell) is too
  new for reliable prebuilt CUDA EP support (large: 18 ms per block on DirectML
  vs 308 ms on CPU).
- **`cpal` for WASAPI loopback**: passing an output device to
  `build_input_stream` sets the loopback flag internally, so there is no
  name-based device matching (the Python prototype's `soundcard` could silently
  bind the wrong device).
- The `directml` ort feature has no Linux prebuilt, so the full workspace does
  not link on WSL (the `core` lib alone builds). The user declined a
  `cfg(windows)` workaround; a known, accepted gap — don't "fix" it.

## Recording and playback (backend)

- The recording is a `StereoRing` (`audio_ring.rs`): native-rate stereo i16,
  300 s (= the frontend's `HISTORY_S`), indexed by absolute frame so frame `n` is
  pipeline time `n / rate` (a note's `start_s` maps straight to a frame). It is
  filled by a `CaptureTap` (`live_capture.rs`) on the capture worker thread —
  the same chunk stream the pipeline gets (cpal's callback sends raw interleaved
  audio; downmix happens on that thread). It lives in Tauri managed state
  (`SharedRing`), outlives `stop_capture` (which returns its `RecordingInfo`) and
  is replaced when the next capture begins. The time base is audio delivered,
  not wall time, but loopback sends nothing during silence, so the capture
  worker feeds zeros for whatever the device didn't deliver (once >0.1 s behind
  real time). Without that, audio time froze in silence and the live roll's
  wall clock drifted against it: open notes after a long pause were drawn
  screen-wide. Silent stretches are therefore in the recording.
- `playback/engine.rs`: an engine thread owns the cpal output stream and renders
  10 ms blocks ahead; the callback pops them and applies a 5 ms fade on
  pause/seek. Recorded audio is copied as is (linear interpolation only if the
  output rate changed); notes come from `playback/synth.rs` (pure, analytic
  phase, so seeking and block boundaries don't matter). Each part has its own
  wave (sine/triangle/PolyBLEP square/saw) and volume under an overall notes
  volume; gains are slider values squared and ramp across a 10 ms block. The
  frontend sends all notes at Play; a muted/hidden part is gain 0; waves,
  volumes and mutes change live (`playback_set_params`).
- Capture and playback exclude each other (the loopback would record the
  playback): `start_capture` stops playback, and Play is disabled while capturing.
- Sync: the callback reports `(pos_s, at_epoch_ms)` — the recording time audible
  at a wall-clock instant, from cpal's playback latency — every 50 ms, tagged
  with the `gen` of the play/seek command (older `gen`s are dropped).
- Untuned: the fade, note envelope/amplitude/per-wave levels, default waves and
  volumes (lead = square, bass = triangle, rest = sine); a Bluetooth output's
  latency is probably only partly reported by WASAPI. Not yet judged by ear/eye
  by the user.
