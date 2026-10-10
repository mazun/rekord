# Frontend (`rekord-piano-roll/`)

Tauri + Svelte 5 (Bits UI) + Pixi.js. Live mode only; capture starts on launch.
UI state in runes modules (`pipeline.svelte.ts`, `settings.svelte.ts`,
`transport.svelte.ts`). `pianoroll.ts` is a plain imperative Pixi class wrapped
by `PianoRollView.svelte`; note/`Started` events go straight to it. Its look is
tuned by the user in the live app.

Detail and rationale per area (read only the relevant section when touching it):
[docs/notes/frontend.md](../docs/notes/frontend.md).

## Rules that bite

- **Live roll clock = linear function of wall time + a constant offset fixed
  once per session** (`LiveClock` in `clock.ts`). Reached after many live
  rounds; **don't re-litigate** (no re-estimation, anchor-and-extrapolate, or
  `total_latency_s` offset).
- `chordlane.ts` has no Svelte/Pixi dependency; it splices layer change events.
  Check with `node dev/chordlane.test.ts`.
- History is 5 minutes (`HISTORY_S` in `recording.ts`), matching the backend
  ring.
- Debug export (right-click a selection) exists in debug builds only.
- Feel of animations/scroll/sync (beat lines, playback cursor, scroll speed) is
  judged by the user in the real app.

## Where things are

- Clocks: `clock.ts` (`LiveClock`, `HeldClock`, `PlaybackClock`).
- Chord lane: `chordlane.ts`, `tensions.ts`; render check `dev/chords.html`.
- Beat lines: `pianoroll.ts` `setBeatLines`; offline replay via `npm run dev` →
  `http://localhost:1420/dev/replay.html?ref=ref-beats.json` (call
  `window.__settle()` before screenshots).
- Playback/selection/export: see notes file sections "Recording playback" and
  "Range selection and debug export".
- Inspecting the real Tauri app over DevTools (second instance with its own
  WebView2 profile): notes file, "Inspecting the real Tauri app".
