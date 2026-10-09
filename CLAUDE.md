# Re:Kord (リコード) — real-time chord/melody recognition

"Re:Kord" is the project's display name (window title, HTML title). Code
identifiers use `rekord` (crates `rekord_core`/`rekord_engine`, the
`rekord-piano-roll/` app directory, Tauri `productName` `ReKord` since `:`
is invalid in Windows filenames, bundle `identifier`
`com.mazun.rekord-piano-roll`). Musical-domain names (`chord_templates`,
`ChordDisplay`, …) intentionally stay "chord" — they mean chords, not the
project. The repo's root folder is still named `chord`.

Real-time PC-audio (WASAPI loopback) chord and note recognition. Pipeline:
the Harmonica transcription model (ONNX, reimplemented and trained in the
separate `harmonica-repro` project; replaced Spotify's basic-pitch on
2026-09-27) for polyphonic note transcription, plus rule-based
pitch-class-set chord-template matching
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
  for that), plus replay of the last 5 minutes recorded while capturing (see
  "Recording playback" below). UI state lives in runes modules
  (`pipeline.svelte.ts`, `settings.svelte.ts`, `transport.svelte.ts` for
  playback); `pianoroll.ts` stays a plain imperative Pixi class wrapped by
  `PianoRollView.svelte`, and note/`Started` events go straight to it rather
  than through reactive state (they are also logged, with their times, in the
  framework-free `recording.ts`). What time the playhead is at comes from the
  clocks in `clock.ts`. Capture starts automatically on launch.

See `TODO.md` for the current backlog of planned features — remove an
item from it once it's actually implemented, rather than leaving it to
rot as a stale wishlist.

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

## Key design decisions (don't re-litigate without reason)

- **Note transcription is Harmonica (arXiv 2609.04640), reimplemented and
  trained in the separate `harmonica-repro` project; only its ONNX files come
  here** (`harmonica_model.rs`, `harmonica_stream.rs`, `harmonica_decode.rs`).
  Spec, accuracy, speed and every parameter choice: `docs/phase5-report.md`
  there (reference code `harmonica/infer.py::predict_song_stream`,
  `harmonica/decode.py`). As with beat tracking, change behavior there
  first, regenerate its golden vectors (`tools/make_golden.py`), then port.
  - Streaming: 16 kHz mono, 32 ms frames; every 8 frames (0.256 s) one 5 s
    window (156 frames) ending `f` frames after the block, audio after "now"
    zeroed, CQT rows before the stream start zeroed, output rows
    `[148-f, 156-f)` (of the time axis, for every part). No LSTM state is carried (the model normalizes over the
    whole window). `f` = 8 (0.26 s, default) or 16 (0.51 s) is a user
    setting; longer doesn't help, 0 loses a lot of accuracy.
  - Sizes small/medium/large (single part) and **mix** (large architecture,
    three output parts; see the next bullet) are all shipped and
    user-selectable; the default is **mix** (2026-10-02, user). If DirectML
    can't be used, it runs **small on the CPU** whatever was chosen
    (medium/large/mix are too slow on CPU). Single-part thresholds are per
    size, shared by both `f` (small 0.25/0.45, medium and large 0.20/0.45).
  - **The Mix model (`mix_large_v2.onnx`, `ModelSize::Mix`) outputs
    `[1, 3, T, 88]` per head: parts vocal, bass, other** (harmonica-repro
    `docs/mix-s2-report.md`, section 9; trained on full mixes incl. drums and
    with GOAT among the training data, COnP 50 ms on RWC-Pop .543 at f16
    vs .512 for its predecessor B4 = `mix_large_v1.onnx`, replaced 2026-10-07).
    The `parts` and
    `thresholds_f8`/`thresholds_f16` (per part, `[th_on, th_sus]`) come from
    the ONNX metadata, not from constants here; the part order is checked at
    load. **f8 and f16 differ** (vocal th_sus .35 vs .30; `part_thresholds`
    picks by `future_frames`); v2: vocal .25/.35|.30, bass .25/.50, other
    .15/.20 (bass and other th_sus were hand-lowered from the CV picks .85/.40,
    which cut held notes short). `NoteTranscriber` runs one `StreamingDecoder` per part with that
    part's thresholds, and every `ConfirmedNote`/`NoteEvent`/`NoteConfirmed`
    carries its `Part` (`All` for single-part models). Known behavior, not a
    bug: low notes of solo instruments (piano bass, contrabass, cello) get
    split between the bass and other parts, so the same pitch can appear in
    both, and bass-part notes can be short fragments (seen on `sample_audio`).
    `velocity` is not on a common scale across parts (other's `th_on` is .15,
    vocal/bass .25), which also skews `KeyEstimator`'s velocity weighting
    toward bass-part notes -- not re-tuned.
  - With the Mix model: the piano roll colors by part (vocal = melody green,
    bass = orange, other = blue; the legend toggles each part's visibility),
    `melody_bass.rs`'s skyline rule is only used for `Part::All` notes, and
    the chord is named from bass + other only -- **vocal notes feed only the
    tensions** (`evaluate_active` in `pipeline.rs`), because sung melody is
    mostly non-chord tones. Vocal-only moments read as chord silence.
    Slash chords show the window's dominant bass (see "Chord naming"), whenever it differs
    from the root: a bass-part-only slash rule was designed (harmonica-repro
    measured `majmin_inv` .769 -> .779 for the offline version) but the user
    chose to leave it, since chord display beyond tensions is going to be
    rebuilt around a separate chord model.
  - The piano roll shows a note from its onset: `NoteTranscriber::take_started_notes`
    reports newly opened notes as `PipelineEvent::NoteStarted`, drawn growing
    with the clock (`OPEN_NOTE_LAG_S` in `pianoroll.ts`, untuned) until the
    `NoteConfirmed` with the same start/pitch/part fixes the length.
  - The decode (onset local maxima, ended by sustain or the next onset) is
    exact in streaming form and never revises a note, so there is no confirm
    margin and no `NoteStitcher`: a note is emitted once, when it ends.
    `velocity` is the onset probability at the onset frame (mean sustain
    saturates near 1 and gave no contrast). Chord matching takes note ends
    as exact: basic-pitch's 0.2 s grace period after a note's end
    (`ACTIVE_EDGE_TOLERANCE_S`) was removed because with Harmonica it only
    let the previous chord linger as a mixed chord (Am7 between C and Am).
  - Display latency is structural: a note's end is known at most
    `(8 + f) × 32 ms` later, so `total_latency_s = (8 + f)·0.032 + capture
    hop (64 ms) + 0.3` = 0.88 s at f = 8, 1.13 s at f = 16.
  - `cargo test -p rekord_core --release harmonica_` checks the Rust port
    against harmonica-repro's golden vectors (`runs/p5/golden`, override with
    `HARMONICA_GOLDEN_DIR`; skipped when absent), and the `harmonica_mix_*`
    tests do the same for the Mix model (`runs/s2_deploy/golden` = GuitarSet,
    notes only in `other` (86), and `golden_rwc001` = 20 s of RWC-Pop 001 with
    all three parts (48/86/456 notes); override the parent with
    `HARMONICA_MIX_GOLDEN_DIR`; also skipped without
    `models/mix_large_v2.onnx`). The golden's `thresholds` are the f8 set; the
    metadata test reads the f16 set from its `onnx_metadata`. RWC is not
    redistributable, so it is only ever read from harmonica-repro. Mix matches
    to ~1e-6 steady-state on DirectML; on CPU up to 1.5e-4 in the first blocks
    (tolerance 2e-4), notes identical. The single-part tests match to ~1e-6 except
    the first 18 blocks of a stream (up to ~1.5e-3), where the window is
    mostly zeroed rows: `ort` statically links pyke's own ONNX Runtime 1.28
    build, which differs slightly from Microsoft's (Python onnxruntime 1.24.4
    and 1.28.0 both reproduce the golden vectors exactly), and such windows
    amplify the difference. Verified to be the binary, not the windowing
    (2026-09-27); notes are identical.
  - The weights are for non-commercial use only (training data include
    CC BY-NC-SA material, and for v2 GOAT, CC BY-NC 4.0; see the model card). They're hosted in the Hugging
    Face model repo `mazun/rekord` (private for now), which `build.rs`
    downloads from at a pinned commit (`HF_REVISION`) and checks against
    pinned SHA-256s; `rust/core/models/` is gitignored. New weights: upload
    them there first, then update `HF_REVISION` and the hashes.
    `mix_large_v2.onnx` was uploaded and pinned on 2026-10-07 (revision
    `85d1b355`; `mix_large_v1.onnx` stays in the repo for older revisions, and
    the model card was updated for v2 in the same commit). Its training data also include the user's own commercial CDs
    (328 h, mixes + pseudo-labels), which the model card's license text says.

- **Chord naming is a windowed pitch-class-energy match, not the instant's note set**
  (2026-10-07, user; `match_chroma` in `chord_templates.rs`, `window_chroma` in
  `pipeline.rs`). For each checkpoint `t` the non-vocal notes overlapping
  `[t - 0.5 s, t]` are summed per pitch class by overlap length (`CHORD_WINDOW_S`), a template's
  score is inside share - 0.7 x outside share - 3 x (shortfall of each tone below a 6% share) -
  0.1 per tone - a per-quality prior + 0.3 x the bass part's share on the root. The prior is 0 for
  maj/m, .1 for the 7ths, .2 for 6/m6 and the 9/11/13 family, .5 for sus/dim/aug/m7b5/dim7, and .15
  for the power chord `5` -- the rarer the chord, the more evidence it needs. The bass (and the
  slash bass, `ChordMatch::bass_pc`) is the bass part's dominant pitch class in the window (the
  lowest note per moment for single-part models), no longer the lowest note at `t`. A committed
  chord's start is backdated by `CHORD_BACKDATE_S` (0.3 s) because a new chord only wins once it
  fills about half the window; `CHORD_MIN_HOLD_S` is 0.25 s. With a known key, a winner `5` on a
  diatonic root whose 3rd is not sounding (< 12% share) is renamed to the key's major/minor triad.
  Every number was swept on RWC-Pop (odd songs to tune, even songs to check; the evaluation tools
  and the whole story are in harmonica-repro's `tools/chord_eval/` and `runs/chord_rwc_v2/`), so
  they are this project's own, like the key estimator's. Result, 100 songs, mix condition,
  file input (no beat snapping): majmin .680 -> .769, majmin_inv .615 -> .702, sevenths .547 ->
  .642, tetrads .513 -> .587 (BTC .815 is the offline reference, not a target). Known costs:
  RWC's labels contain almost no add9/9/11/sus2/power chords, so everything ornate scores as
  wrong there (the drop from the 5-chord vocabulary's ceiling is expected); `5` costs about
  .03 majmin against disabling it; a longer window (1 s+) blurs changes. Not done, on purpose:
  correcting the chord lane after a bar or so (fixed-lag Viterbi over these scores measured
  majmin .797 / tetrads .593 on the 5-chord vocabulary, +.03 over the windowed match alone;
  needs the chord display to be revisable like `BeatLines`, to be designed in its own session).
  Confirmed by sample_audio (same sequences as before incl. G6). Checked live by the user
  (2026-10-07): accuracy is fine, but the chord display is noticeably less real-time (the 0.5 s
  window + 0.25 s hold delay each change). Accepted for now; the next session is meant to trade
  this the other way: confirm chords after listening ~2 s and tune the display for that, ideally
  as three revisable stages (one of them confirmed at 0.5 s, like `BeatLines`' layers; the exact
  stages are to be settled in that session).
  Quick loop without the model: `cargo run --release -p rekord_core --example chord_replay --
  <raw_dir> <out_dir>` replays the chord logic over a `rekord_engine` file-mode stdout
  (harmonica-repro's `runs/chord_replay.sh`), matching the real exe to +-.001 majmin.

- **Chord layers** (2026-10-08, user; the whole design, the step-2 numbers and the step-3 notes
  are in `docs/chord-layers.md`; five steps with a confirmation after each): the chord estimate
  runs as append-only layers of increasing delay -- **instant**, **settled** (~0.5 s: the chord
  naming that used to be the only one, effective delay 0.55 s) and **confirmed** (2 s) --
  each reporting where its chord changes plus how far it has decided (`decided_s`; a layer never
  revises what it reported). Backend: `rust/core/src/chord_layers/` -- `hold.rs` (`HoldLayer`: the
  window match + hold + backdate + score margin; the two layers are `HoldParams::instant()` and
  `::settled()`), `confirmed.rs` (`ConfirmedLayer`, below), `evidence.rs`, `engine.rs` (`ChordEngine`: runs the layers at the same
  checkpoints, emits `PipelineEvent::ChordLayer { layer, changes, decided_s }` per block per
  layer, `ActiveNotes` (the sounding pitch classes) and `KeyChange`; `Started` carries the chord
  vocabulary). Frontend: `chordlane.ts` composes the layers into the lane by **splicing change
  events** (each layer owns the span only it has decided, with a 0.25 s grace for the faster
  layer's changes just before the hand-over; a "state of the slowest decided layer" composition made
  slivers and false fades there), `planChordUpdate` pairs the old and new segments by name and
  overlap so that only chords that really appeared or disappeared fade (a chord that merely moved
  slides; no per-layer styling), `tensions.ts` computes the tensions from `ActiveNotes` and the
  **shown** chord, and the top readout follows the lane, live too. Checked with
  `node rekord-piano-roll/dev/chordlane.test.ts` and, for rendering, `dev/chords.html` (a log
  written by `chord_replay` with `REKORD_CHORD_EVENTS_OUT`, see its header). Parameters can be
  tried in the running app with the environment variables `REKORD_CHORD_TUNE_INSTANT` /
  `REKORD_CHORD_TUNE_SETTLED` (`h=0.05,c=0`: see `HoldParams::apply_tune`) and
  `REKORD_CHORD_TUNE_CONFIRMED` (`D=1.5`, or `off` for no 2 s layer); the CLI prints one layer by
  default (`--chord-layer instant|settled|confirmed|all`). **Status: all five steps done; the user checked the whole live and approved it
  (2026-10-09).** (Step 3 was confirmed after a live try the same day; the 2 s layer below was
  evaluated in step 4 and registered in `ChordEngine::standard()` in step 5 -- docs/chord-layers.md
  section 13; the frontend needed no change.)
  The instant layer (window 0.4 s with 0.1 s exponential decay, 0.05 s hold, 0.4 score margin for
  the shown chord, **slash chords on, a change of the bass alone held 0.4 s**) names a chord
  ~0.16 s after the notes show it (settled: 0.55 s), with ~19 lane corrections a minute. The user
  chose `c=0,h=0.05` after trying it live; on its own that made 71 corrections a minute, because
  the score margin does not cover the slash bass, hence `bass_hold_s` (`bh`). The settled layer is
  the old naming plus a 0.2 score margin (identical to the old output with `m=0`: 100 RWC songs,
  20990 chords); the margin does not weaken major/minor (the 3rd comes out reversed on 3.3% of the
  time instead of 3.7%). Finding: the instant layer is about as accurate as the settled one (RWC
  majmin .791 vs .783, tetrads .586 vs .601), so what a correction adds is mostly the ornate
  names; the accuracy gain is the 2 s layer's (below). A wrong major/minor on a chord whose 3rd the transcription lacks comes from the
  **key**: a bare fifth gets the key's triad (`chord_templates.rs`), and the key lags a modulation;
  see the key estimation bullet and `docs/chord-layers.md` section 11.
  **The 2 s layer** (`confirmed.rs`, 2026-10-09; `docs/chord-layers.md` section 12): a fixed-lag
  Viterbi over (root, quality, bass) + silence, one step per 0.1 s from the same template scores
  (`score_templates`, shared with the other layers) of a 0.4 s window centred 0.07 s early, costs
  2.5 for another chord and 3 for another bass, decided 2.0 s after `covered` (1.5 s is as
  accurate). RWC-Pop 100 songs, pos: majmin .824 (settled .783), majmin_inv .801 (.705), sevenths
  .693 (.657), tetrads .633 (.601), 36.5 chords a minute against settled's 48 (reference 33); it is
  right where settled is wrong 7.4% of the time and wrong where settled is right 3.3%. A bare
  fifth gets the 3rd the song has shown for that root, else the key's, but only as a tie-break
  (`key_lift`): letting the key's triad outscore the notes (the other layers' rule) spread a stale
  key over whole chords (the modulating song cdex:2c6693d2bd4f 20.7 s wrong against 10.4 s). Offline it runs with `chord_replay`
  `REKORD_CHORD_TUNE="layer=confirmed,..."` (keys: `ConfirmedParams::apply_tune`). With it the
  lane shows roughly 10-20 more chords a minute disappear and 4-16 more appear (settled's flicker
  removed, ornate names changed): to be judged live. Not fixed: the lag of the key estimate itself (needs a layer that looks 8 s+
  ahead).

- **Beat/downbeat tracking (`beat_model.rs`, `beat_tracker.rs`, and the DBN
  decoder in its own crate `rust/beat_dbn/`) is Beat This! small0 + madmom's
  DBN, ported from the Python reference in the separate `harmonica-repro` project**
  (`beattrack/online.py`, `beattrack/dbn_ref.py`; evaluation and every
  parameter choice in its `docs/beat-report.md`). The Rust port matches that
  reference exactly on its golden vectors (`cargo test -p rekord_core
  --release beat_` and `cargo test -p rekord_beat_dbn --release`; tests skip
  when `harmonica-repro/runs/beat/golden` is absent, override with `BEAT_GOLDEN_DIR`). Change behavior there first,
  regenerate the golden vectors, then port -- don't tune the Rust side alone.
- **The DBN decoder is a separate crate (`rekord_beat_dbn`) only so the Tauri
  app's dev profile can optimize it alone** (`[profile.dev.package.rekord_beat_dbn]`
  in `src-tauri/Cargo.toml`): unoptimized, its float64 Viterbi took 440 ms of CPU
  per second of audio (17 ms optimized) and the tracker barely kept up in
  `tauri dev`. The user preferred this over optimizing all of `rekord_core`.
- **The one exception to "everything lives in Rust": `build.rs` runs
  `core/export_beat_models.py` in a throwaway venv when
  `models/beatthis_small0.onnx` or `models/logmel.onnx` is missing.** Beat
  This! is only published as a PyTorch checkpoint (MIT), and the user chose
  converting at build time over hosting a converted file. `logmel.onnx` has
  no learned weights (DFT basis + Hann window + mel filterbank) and is
  generated by the same script rather than committed. Needs Python >= 3.10
  on the build machine (or `REKORD_PYTHON`); the first build downloads
  ~1 GB of CPU-only PyTorch (about 1.5 min here), then deletes the venv.

- **DirectML, not CUDA**, for ONNX Runtime GPU inference (`harmonica_model.rs`).
  RTX 5070 Ti (Blackwell) is too new for reliable prebuilt CUDA EP wheel
  support; DirectML just works (Harmonica large: 18 ms per 0.256 s block vs
  308 ms on the CPU; basic-pitch saw ~3.5–3.9x).
- **`cpal` for WASAPI loopback**, not a hand-rolled `wasapi` crate binding:
  passing an output device into `build_input_stream` makes cpal set the
  loopback flag internally — no name-based device-matching ambiguity at all
  (the Python prototype's `soundcard` library had a real bug here: it could
  silently bind to the wrong device when a line-input and its loopback
  endpoint shared a display name).
- **Recently finished notes are kept for 3.3 s** (`RETAINED_SECONDS` in
  `harmonica_stream.rs`) as context for chord matching and melody/bass
  labeling; the decode itself is incremental, so this only bounds the size
  of `note_events()`, not any reprocessing. (Under basic-pitch this cap
  bounded re-decoding of retained frames, and raising it caused O(T²)-ish
  growth; that concern is gone.)
- **Piano-roll clock is a pure linear function of wall-clock time, offset
  by a constant fixed once per session** (2026-10-03, user): the playhead
  is the keyboard's left edge and shows the audio clock minus
  `displayLagS` = the smallest `wall - start_s` over the `NoteStarted`
  events of the first 4 s (`LAG_MEASURE_S`) + 0.01 s, measured in
  `LiveClock` (`clock.ts`) and then never re-estimated. This replaced the earlier
  `total_latency_s` offset (~0.88 s, the wait for a note's *end*), which
  drew every new note off-screen to the right / under the keyboard for
  its first ~0.6 s. `total_latency_s` is still sent on `Started` but the
  piano roll ignores it. The delay is structural (block length +
  look-ahead + capture hop), not jitter, so
  anchor-and-extrapolate or rate-correction approaches both produce
  visible warping/acceleration (for the *live* clock; playback has no such
  delay to hide -- see "Recording playback"); a fixed offset anchored to the `Started`
  event (fired once the pipeline truly begins, not on button click —
  model load + WASAPI init add unpredictable delay) is the correct model
  and was reached only after several rounds of live-app testing. The
  window (156 frames, 5 s) and the ONNX input shapes are fixed at export;
  only the look-ahead `f` is configurable.
- **The piano roll keeps the last 5 minutes** (`HISTORY_S` in `recording.ts`:
  notes, chord lane, beat lines; off-screen items are only hidden, not
  destroyed). While capture is stopped, the wheel/trackpad, dragging the
  notes, or a draggable scrollbar along the bottom edge scrolls back through
  it (the stopped view is a `HeldClock` whose position the user scrolls; the
  live clock is untouched and is the only clock while running).
  Added 2026-10-07, user; the scroll speed/retention are untuned and it has
  not been tried with a real 5-minute session in the live app.
- **Recording playback** (2026-10-07, user: a play button next to the record
  button replays the last 5 minutes, recorded audio plus the roll's notes
  synthesized). Built and checked live against the sample files (a second
  `--live` CLI hears the playback and reads the same chords, also with the
  recorded audio muted, and nothing with everything muted), not yet judged by
  ear/eye by the user.
  - **The recording** is a `StereoRing` (`audio_ring.rs`): native-rate stereo
    i16, `RECORDING_HISTORY_S` = `HISTORY_S` = 300 s (~57 MB at 48 kHz), indexed
    by absolute frame so frame `n` is pipeline time `n / rate`, i.e. a note's
    `start_s` maps straight to a frame. It is filled by a `CaptureTap`
    (`live_capture.rs`) on the capture worker thread -- the same chunk stream
    the pipeline gets, so chunks lost before that are lost to both -- which is
    why the cpal callback now sends the raw interleaved audio and the
    downmix moved to that thread. It lives in Tauri managed state
    (`SharedRing`), outlives `stop_capture` (which returns its
    `RecordingInfo`) and is replaced when the next capture begins. The
    time base is audio delivered, not wall time: loopback sends no packets
    during silence, so a silent stretch is absent from the recording (and
    the live roll's wall clock drifts against it -- pre-existing).
  - **Playback** (`playback/engine.rs`): an engine thread owns the cpal output
    stream and renders 10 ms blocks ahead; the callback only pops them and
    applies a 5 ms fade on pause/seek. The recorded audio is copied as is
    (linear interpolation only if the default device's rate changed), the
    notes come from `playback/synth.rs` (pure, analytic phase, so seeking and
    block boundaries don't matter), gains are slider values squared. Each
    part (`all` for the single-part models, else vocal/bass/other) has its
    own wave -- sine, triangle, or PolyBLEP-band-limited square/saw, with a
    rough per-wave level equalization -- and its own volume, under an overall
    notes volume; every gain ramps linearly across a 10 ms block, so mute and
    slider drags don't click. The frontend sends all notes at Play; a muted or
    legend-hidden part is just gain 0, and the waves, volumes and mutes can
    change live (`playback_set_params`). Capture and playback exclude each other (the
    loopback would record the playback): `start_capture` stops playback, and
    Play is disabled while capturing.
  - **Sync**: the callback reports `(pos_s, at_epoch_ms)` -- the recording
    time audible at a wall-clock instant, from cpal's own playback latency --
    every 50 ms, tagged with the `gen` of the play/seek command (reports of an
    older `gen` are dropped). `PlaybackClock` anchors on the first report and
    extrapolates at exactly 1x (`performance.timeOrigin + now()` is the same
    wall clock as the backend's), re-anchoring only after three reports 40 ms
    off, so the roll doesn't change speed; there is no display lag in
    playback, the cursor is the audio being heard. Untuned: `RESYNC_S`, the
    fade, the note envelope/amplitude/per-wave levels, the default waves and
    volumes (lead = square, bass = triangle, rest = sine); a Bluetooth
    output's latency is probably only partly reported by WASAPI.
  - **UI**: clicking the stopped roll sets a marker (`roll.setMarker`); Play
    starts at the marker, else at the playhead (so pause/resume and scrolling
    work), and a recording whose playhead is within 1.5 s of its end plays
    from the roll's left edge instead. Play/pause toggles; clicking while
    playing seeks; the chord/key/tension/BPM readouts follow the playhead
    (`Recording.stateAt`; the chord lane, the active-notes stream and
    `KeyChange` therefore carry times, and the BPM is stamped with the time its
    beats describe). The
    mix (recorded audio and notes volumes, plus per-part wave pickers and
    volumes for the 3-part model, each with a mute button) is a popover next
    to Play rather than in settings, since opening settings stops playback;
    waves and volumes are remembered, mutes are not. Piano keys are
    highlighted at the cursor in every non-live mode, including while
    scrolling a stopped roll (previously only live).
  - **Cursor vs view** (DAW-style): `RollClock` has the *view* (`displayS`,
    the time at the keyboard edge, which positions everything) and the
    *cursor* (`cursorS`, the time of the audio). When playback starts or
    seeks, the view stays put and a white cursor line travels right from the
    start position (the marker, or the left edge) at 1x until it reaches the
    keyboard edge, after which the roll scrolls under it as before
    (`PlaybackClock`: view = max(view at start, cursor)). A paused playback
    keeps its cursor line where it stopped (resume plays from there);
    scrolling or dragging the stopped roll puts the cursor back at the edge.
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
  **Re-tuned 2026-10-09 (user's modulating song cdex:2c6693d2bd4f; docs/chord-layers.md section 11):** an
  8 s window with 2 s exponential decay, a 0.1 penalty on minor keys' correlation (a bare-fifth
  riff has no 3rd and read as the minor key sharing the old key's scale), and switching by
  accumulated evidence over the 12 relative-major classes (`estimate_cusum`: a challenger must gain
  `cusum_s` = 1, correlation x seconds, on the shown key) instead of the margin / hold rule (still
  there with `REKORD_KEY_TUNE="l=0,w=12,d=0,mp=0"`). Chord naming asks `naming_key()`, which goes
  with a challenger already after half that gain, because a bare fifth named after the old key's
  scale is the main way a modulation shows as wrong major/minor. Against reference keys read off
  RWC-Pop's chord annotations (`harmonica-repro/tools/chord_eval/key_eval.py`): agreement .691 ->
  .754, delay 8.7 -> 7.0 s, switches 2.3 -> 2.8 a minute (reference 1.0); the 0.5 s layer's majmin
  .778 -> .783. Not fixable from the notes: the first chord of a modulated section (the notes
  have no 3rd and the key has not caught up yet). A 3rd read from the vocal part or a major 7th
  overruling the key was tried and does nothing (+.001).
- **Roman numerals are always spelled against the key's governing major
  scale** (a minor key's relative major, not its own tonic), and the key
  name always **shows both readings of the major/minor pair** (e.g.
  "C Major (A Minor)"), never switching which one displays depending on
  which the correlator locked onto. Deliberate: relative major/minor
  share all seven diatonic pitch classes, so that call is the noisiest
  part of the estimate, and this makes the noise invisible instead of
  surfacing as a flickering label.
- **Chord/key spelling (`spelling.rs`) has three user modes -- Auto / always ♯ /
  always ♭** (`Accidental`, setting `chord.accidental`, live-switchable via
  `set_accidental`; each chord layer re-announces its latest chord, at its own
  start time, in the new spelling and the key goes out as a normal `KeyChange`;
  past chords on the roll keep their spelling). Auto = "most natural on a piano score" (2026-10-02,
  user): dim/dim7/m7b5 roots are always ♯; otherwise a root is spelled as a
  scale degree of the estimated key (so A major gets C♯m/F♯m, D major a
  borrowed B♭), with a fixed table while there is no key (flats, but F♯ and
  C♯m/F♯m/G♯m); a slash-chord bass is spelled by letter arithmetic from the
  spelled root (E/G♯, Fm/A♭, Gmaj7/F♯, C/B♭, never G♭/A♯); key names are G♭-side
  majors with the fewest-accidentals relative minor ("E Major (C♯ Minor)").
  C♭/F♭/E♯/B♯ and double accidentals are never produced. The frontend
  mirrors only the key-pair table (`keyPairLabel` in `pipeline.svelte.ts`) for
  the manual key picker; keep it in step with `key_pair_name`. Because the Auto
  spelling follows the key estimate, a short-window key misdetection also
  respells chords (accepted). Not yet checked by ear/eye in the live app.
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
  `observe()` is called in *confirmation* order (`NoteTranscriber` emits
  a note once its end is decided), not start_s order, so
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
  Those numbers, and `MIN_RELATIVE_VELOCITY`, date from basic-pitch notes,
  whose velocity was mean frame activation; Harmonica's velocity is the
  onset peak, and nothing here has been re-tuned for it yet.
  `melody`/`bass` on `PipelineEvent::NoteConfirmed` aren't mutually
  exclusive in the backend (a lone low note can trivially satisfy both);
  the piano-roll frontend resolves that by always preferring melody when
  both are set, rather than the backend picking one.

## Beat lines (beat/bar lines on the piano roll)

Where things are:

- **Backend**: `run_live_session` (`pipeline.rs`) runs the note pipeline on
  the calling thread and `run_beat_tracker` on a scoped second thread, which
  gets a copy of every capture chunk (Beat This! costs ~40% of a core; it must
  not delay notes). `on_event` is therefore `FnMut + Send` and called through
  a `Mutex` from both threads. A failing beat model only disables the lines.
- **Tracker**: `BeatTracker::bounded` (`beat_tracker.rs`) keeps only the last
  30 s of audio/spect/activations (`Tail` buffers); `BeatTracker::new` keeps
  everything and is what the golden tests compare against, and a test checks
  that `bounded` produces identical lines.
- **Event**: `PipelineEvent::BeatLines { from_s, committed, provisional,
  predicted }` is a snapshot of every line in the last 30 s, sent whenever a
  line changed (~3/s); it replaces what the frontend has after `from_s`.
- **Frontend**: `pianoroll.ts` `setBeatLines` matches each snapshot line to a
  drawn one within `BEAT_MATCH_TOLERANCE_S` (same beat changing layer) and
  eases layer/position/bar-ness with `BEAT_ANIM_TAU_S`; unmatched lines fade in
  or out. The look (`BEAT_LINE_ALPHA` per layer, colors, `BAR_LINE_WIDTH`) is
  tuned by the user in the live app, like everything else in `pianoroll.ts`.

The three layers are the same beats at increasing delay. Measured by
real-time replay (2026-09-27, stream s00 of harmonica-repro), relative to the
display lag of 1.3 s with 250 ms capture chunks under basic-pitch. Since the
switch to Harmonica the chunk is 64 ms (lines arrive a little sooner) and the
display lag is 0.88 s at the default look-ahead, so provisional lines now
mostly firm up just after passing the playhead (accepted by the user,
2026-09-27; not re-measured):

| layer | first shown after its time (median) | before the playhead |
|---|---|---|
| predicted | 0.3 s | 100% |
| provisional | 1.2 s (0.5 s of withheld future + net stride + chunk + ~0.2 s compute) | 67% |
| committed | 5.3 s | never (turns committed ~480 px behind it) |

Expected, not bugs: lines appearing/disappearing behind the playhead around
song changes (committed lines correcting provisional ones, and the song-change
reset); some songs where Beat This! locks onto the off-beat (model accuracy).
Behavior changes go to harmonica-repro first (see Key design decisions).

### Beat lines: checking timing and rendering

Without playing audio, using stream audio + reference beats from
harmonica-repro (`runs/beat/stream/audio/sNN.wav`/`.json`):

1. Record what the frontend would receive (runs at real-time pace):
   `cargo run --release -p rekord_core --example beat_replay --
   ../../harmonica-repro/runs/beat/stream/audio/s00.wav
   ../rekord-piano-roll/dev/replay-events.jsonl --seconds 90` (from `rust/`).
2. Numbers: in harmonica-repro's WSL venv (`wsl -d Ubuntu-24.04`,
   `~/venv-beatnet/bin/python`; numpy is all it needs),
   `python -m beattrack.replay_analyze <that jsonl> --stream 0
   --write-ref <this repo>/rekord-piano-roll/dev/ref-beats.json` prints the
   table above plus misses/corrections by time.
3. Rendering: `npm run dev` in `rekord-piano-roll` (or preview_start with
   the repo-root `.claude/launch.json`), then open
   `http://localhost:1420/dev/replay.html?ref=ref-beats.json`.
   `dev/replay-mock.js` fakes Tauri's IPC and replays the log in real time
   (beat lines only, no notes); reference beats are ticks on the top/bottom
   edges. The Claude desktop browser pane is usually *hidden*, which
   throttles requestAnimationFrame so the Pixi ticker barely runs -- call
   `window.__settle()` right before each screenshot (it also returns the
   on-screen lines), and read state via `window.__roll`. Region zoom isn't
   supported there, so judge fine visual detail from `__settle()`'s numbers.
4. The feel of the animation is for the user to judge in the real app.

To inspect the **real Tauri app** (not the replay): start a second instance
of the built debug exe with its own WebView2 profile and a debug port --
`$env:WEBVIEW2_USER_DATA_FOLDER="$env:TEMP\rekord2-wv2";
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9333";
Start-Process rekord-piano-roll\src-tauri\target\debug\rekord-piano-roll.exe`
(it loads the running `tauri dev` server; without its own profile it shares
the first instance's browser process and ignores the port). Then
`http://localhost:9333/json` lists the page; drive it over the DevTools
protocol from Node (global `WebSocket`): `Runtime.evaluate` (e.g. import
`/src/pianoroll.ts` and hook a prototype method to capture the instance),
`Runtime.consoleAPICalled` for console output, `Page.captureScreenshot`.
Loopback capture allows several taps, so it hears the same audio. Close it
and delete that profile folder afterwards. (Once, 2026-09-27, the user's
instance showed a blank piano roll after these changes while a fresh
instance worked; a restart fixed it, cause unknown.)

## Testing

The beat-tracking and note-transcription code are the exceptions: both have
golden-vector tests against their Python reference in harmonica-repro (see
Key design decisions), and beat tracking also has the replay tools above.
Everything else has no unit test suite -- except small pure-function tests for
the playback pieces (`cargo test -p rekord_core --release --lib`: the ring buffer,
the note synth and the block renderer) -- validation is empirical, against the real recordings
in `sample_audio/` (`piano_progression_C_Am_F_G.wav`,
`piano_progression_ConG_A7sus4_FM7add9_G6.wav`,
`piano_complex_progression_ConG_A7sus4_FM7add9_G6.wav`; with the Mix model
(2026-10-02) all three give the same chord sequence as the single-part large,
only the third file's early Roman numerals differ, from a start-of-session key
guess on a single bass note). **Never validate
against synthetic sine-wave test audio** — harmonics produce false
tension-chord detections (e.g. spurious add9) that don't occur with real
piano recordings. When changing the note streaming/decode/clock logic,
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
