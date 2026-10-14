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

## Drum lane (`pianoroll.ts` `setDrumHits`)

A lane under the pitch rows in the same Pixi canvas (so it shares the clock, zoom,
beat lines, selection and scroll): one row per drum (CY/HH/TM/SD/BD from the top),
each hit a short tick tinted by drum, alpha by strength; the labels sit in the strip
at the keyboard's x. `PipelineEvent::DrumHits` arrives per layer (instant / settled /
confirmed, `decided_s` each); like the chord lane, a later layer replaces the earlier
ones over the span it has decided, so `setDrumHits` destroys a lower layer's sprites
at or before `decided_s` and the layers' own hits never need a per-frame ownership
check. `Recording` keeps the hits per layer with the same replacement rule (`synthDrumHits`) and
playback sounds them (below); they are not in the debug export.

The keyboard toggle (`PianorollToggle`) and the drum toggle (`DrumToggle`) are
independent settings (`pianorollVisible`, `drumsVisible`); the window collapses only
when both are off (`windowSize.ts` `isRollShown`). The chord lane stays whenever
either is shown. Layout: `pianoBottom()` ends the pitch rows above the drum lane;
without pitch rows the lane starts under the chord lane with the same row height.
The roll writes `--zoom-corner-height` / `--roll-bottom-reserve` on its container for
the zoom control and the part legend. Drums only (keyboard off): `windowSize.ts`
`fitDrumsOnly` shrinks the window so the roll area is just chord lane + drum lane
(`DRUMS_ONLY_ROLL_HEIGHT`, minimum height lifted, size not saved) and `restoreFullHeight`
brings back the previous height when the keyboard returns.

## Note parts

The roll colors by part (vocal = green, bass = orange, other = blue); the
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
  the 3-part model; one volume and mute for all the drums, which the backend synthesizes from
  the transcribed hits in `playback/drum_synth.rs`: sweep sine for kick/tom, sine + noise for
  snare, high-passed noise for hi-hat/cymbal, untuned; per-drum volumes not done) is a popover next to Play, not in settings, since opening
  settings stops playback. Waves and volumes are remembered, mutes are not.
- **Cursor vs view** (DAW-style): `RollClock` has the *view* (`displayS`, the time
  at the keyboard edge) and the *cursor* (`cursorS`, the audio's time). On
  play/seek the view stays put and a white cursor travels right at 1x until it
  reaches the keyboard edge, after which the roll scrolls under it
  (`PlaybackClock`: view = max(view at start, cursor)). A paused playback keeps
  its cursor; scrolling/dragging the stopped roll puts it back at the edge.

## Range selection and debug export

While the roll is stopped, playing or capturing, a plain **drag** selects a range (`pianoroll.ts`: a
translucent band with edge lines) and **Space+drag** scrolls (hand-tool style; the wheel and the
scrollbar still scroll too; none of that while capturing, and no export either). A click still places
the marker or seeks (while capturing it only places the marker; the selection and marker are in audio
time, so they scroll away with the notes). A click outside the band or Esc
clears the selection; a click inside only moves the position. Pointing at the roll takes the focus off
a toolbar button, so Space reaches the roll. In **debug builds only** (`debug_export_enabled` in lib.rs,
i.e. `cfg!(debug_assertions)`; `tauri dev` and `tauri build --debug`) **right-clicking the selection**
opens a one-item menu that exports it (`debugExport.svelte.ts` → `debug_export.rs`) to
`Documents/ReKord debug exports/export_<unix s>/`:
`audio.wav`, `raw/001.txt` (notes in `rekord_engine`'s line format, times relative to the window, so
`chord_replay <folder>/raw <out>` re-runs the chord logic), `chord_layers.json` (each layer's
reports, the lane shown, keys; recording time) and `meta.json`. The window is the selection plus 4 s
before / 3 s after (`EXPORT_PAD_*` in recording.ts) so the 2 s layer reproduces it. Beat lines are
not exported (the roll keeps them, `Recording` does not). `dev/chords-mock.js` fakes the commands
(`window.__lastExport` holds the request).

**MIDI out** (all builds; `midiCopy.svelte.ts` → `midi_copy.rs`, not while capturing). The selection's
notes (clipped to it, times relative to its start; parts hidden with the legend left out) become a
.mid in `%TEMP%/ReKord midi/` (newest 20 kept): one track per part, channels 1-3 for Vocal/Bass/Other
when split, **no tempo/time-signature event** (a DAW would ask to import it) -- ticks are laid out in
beats of the tracker's BPM (120 without one), so the project tempo must match the song's. **Drums**: with
the drum lane shown, a drag pressed on empty roll also writes a "Drums" track on channel 10 in the General
MIDI map (kick 36, snare 38, tom 47, closed hi-hat 42, crash 49; 16th-note lengths, velocity from the
hit's strength); a drag pressed on a note stays that part alone. With the keyboard hidden the notes are
left out too (what is on screen goes in), and the drag picture shows the hits as coloured rows. A **drag from
inside the selection** drags the file out of the window (`os_drag.rs`: `DoDragDrop` on the shell's own
file data object; the picture, drawn in `midi_copy.rs` `thumbnail` (the notes as bars in the roll's
colours), is a click-through topmost layered window that follows the cursor from `QueryContinueDrag`.
The shell's `IDragSourceHelper` image was tried: the drop target draws it, so it showed only over
Explorer, not over the app or a DAW; Bitwig and Studio One take the drop): pressed on empty roll = all three parts on their own channels, pressed on a note = only that
note's part on one channel. A click inside the selection still only seeks; edges still resize; to start
a new selection press outside the band. (A right-click "copy to the clipboard" as `CF_HDROP` existed
and was removed: Studio One didn't take it.) (The `drag` crate was used before: whatever picture it was
given, Windows showed its own file-drag look plus a stray image.) `dev/chords-mock.js`
keeps the request in `window.__lastMidi`.

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
