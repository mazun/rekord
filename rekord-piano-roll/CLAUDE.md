# Frontend (`rekord-piano-roll/`)

Tauri + Svelte 5 (Bits UI for headless widgets) + Pixi.js. Live mode only; capture
starts automatically on launch. UI state lives in runes modules
(`pipeline.svelte.ts`, `settings.svelte.ts`, `transport.svelte.ts` for playback).
`pianoroll.ts` stays a plain imperative Pixi class wrapped by
`PianoRollView.svelte`; note/`Started` events go straight to it rather than
through reactive state (they are also logged with their times in the
framework-free `recording.ts`). What time the playhead is at comes from the
clocks in `clock.ts`. The look of `pianoroll.ts` is tuned by the user in the live
app.

## Clock

- **The live roll clock is a pure linear function of wall-clock time, offset by a
  constant fixed once per session** (2026-10-03, user): the playhead (keyboard's
  left edge) shows the audio clock minus `displayLagS` = the smallest
  `wall - start_s` over the `NoteStarted` events of the first 4 s
  (`LAG_MEASURE_S`) + 0.01 s (`LiveClock`, `clock.ts`), never re-estimated. The
  earlier `total_latency_s` offset (the wait for a note's *end*) drew new notes
  off-screen for their first ~0.6 s; `total_latency_s` is still sent on `Started`
  but ignored here. The delay is structural (block + look-ahead + capture hop),
  not jitter, so anchor-and-extrapolate and rate-correction both produced visible
  warping. Anchor to the `Started` event (fired when the pipeline truly begins,
  not on button click — model load and WASAPI init add unpredictable delay).
  Reached only after several rounds of live testing; don't re-litigate.
- Window (156 frames, 5 s) and ONNX input shapes are fixed at export; only the
  look-ahead `f` is configurable.

## History and scrolling

The roll keeps the last 5 minutes (`HISTORY_S` in `recording.ts`: notes, chord
lane, beat lines; off-screen items are only hidden). While capture is stopped,
the wheel/trackpad, dragging notes, or the bottom scrollbar scrolls back through
it (a `HeldClock` the user scrolls; the live clock is untouched). Scroll
speed/retention are untuned and not tried with a real 5-minute session.

## Chord lane (`chordlane.ts`, `tensions.ts`)

`chordlane.ts` (no Svelte/Pixi dependency) composes the backend's chord layers
(see rust/CLAUDE.md) into the lane by **splicing change events**: each layer owns
the span only it has decided, with a 0.25 s grace for the faster layer's changes
just before the hand-over (a "state of the slowest decided layer" composition
made slivers and false fades). `planChordUpdate` pairs old and new segments by
name and overlap so only chords that really appeared/disappeared fade; a chord
that merely moved slides (no per-layer styling). `tensions.ts` computes tensions
from `ActiveNotes` and the **shown** chord; the top readout follows the lane,
live too. Checks: `node dev/chordlane.test.ts`, and `dev/chords.html` for
rendering (log from `chord_replay`, header explains). Design: `docs/chord-layers.md` §4.

## Beat lines (`pianoroll.ts` `setBeatLines`)

Each snapshot line is matched to a drawn one within `BEAT_MATCH_TOLERANCE_S` (same
beat changing layer) and eased in layer/position/bar-ness with `BEAT_ANIM_TAU_S`;
unmatched lines fade in/out. `BEAT_LINE_ALPHA` per layer, colors and
`BAR_LINE_WIDTH` are user-tuned. The animation's feel is for the user to judge in
the real app.

Replay without audio: `npm run dev` (or `preview_start` with the repo-root
`.claude/launch.json`), open `http://localhost:1420/dev/replay.html?ref=ref-beats.json`.
`dev/replay-mock.js` fakes Tauri IPC and replays a log in real time (beat lines
only); reference beats are ticks on the top/bottom edges. The Claude desktop
browser pane is usually hidden, which throttles requestAnimationFrame — call
`window.__settle()` right before each screenshot (it returns the on-screen
lines); read state via `window.__roll`. Region zoom isn't supported there.

## With the Mix model

The roll colors by part (vocal = melody green, bass = orange, other = blue); the
legend toggles each part's visibility.

## Recording playback

A play button next to the record button replays the last 5 minutes (recorded
audio plus the roll's notes synthesized; backend in rust/CLAUDE.md).

- `PlaybackClock` anchors on the first backend position report and extrapolates
  at exactly 1x (`performance.timeOrigin + now()` is the backend's wall clock),
  re-anchoring only after three reports 40 ms off (`RESYNC_S`, untuned) so the
  roll doesn't change speed. There is no display lag in playback: the cursor is
  the audio being heard.
- Clicking the stopped roll sets a marker (`roll.setMarker`). Play starts at the
  marker, else at the playhead; a playhead within 1.5 s of the end plays from the
  left edge. Play/pause toggles; clicking while playing seeks. Chord/key/tension/
  BPM readouts follow the playhead (`Recording.stateAt`; that is why the chord
  lane, active notes and `KeyChange` carry times, and BPM is stamped with the
  time its beats describe). Piano keys highlight at the cursor in every non-live
  mode.
- The mix (audio and notes volumes; per-part wave pickers, volumes and mutes for
  the 3-part model) is a popover next to Play, not in settings, since opening
  settings stops playback. Waves and volumes are remembered, mutes are not.
- **Cursor vs view** (DAW-style): `RollClock` has the *view* (`displayS`, the time
  at the keyboard edge) and the *cursor* (`cursorS`, the audio's time). On
  play/seek the view stays put and a white cursor travels right at 1x until it
  reaches the keyboard edge, after which the roll scrolls under it
  (`PlaybackClock`: view = max(view at start, cursor)). A paused playback keeps
  its cursor; scrolling/dragging the stopped roll puts it back at the edge.

## Inspecting the real Tauri app

Start a second instance of the debug exe with its own WebView2 profile and debug
port (without its own profile it shares the first instance's browser process and
ignores the port):

```powershell
$env:WEBVIEW2_USER_DATA_FOLDER="$env:TEMP\rekord2-wv2"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9333"
Start-Process rekord-piano-roll\src-tauri\target\debug\rekord-piano-roll.exe
```

It loads the running `tauri dev` server. `http://localhost:9333/json` lists the
page; drive it over the DevTools protocol from Node (global `WebSocket`):
`Runtime.evaluate` (e.g. import `/src/pianoroll.ts` and hook a prototype method
to capture the instance), `Runtime.consoleAPICalled`, `Page.captureScreenshot`.
Loopback allows several taps, so it hears the same audio. Close it and delete
that profile folder afterwards.
