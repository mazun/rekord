# Re:Kord (リコード) — real-time chord/note recognition

"Re:Kord" is the display name; code identifiers use `rekord` (crates
`rekord_core`/`rekord_engine`, `rekord-piano-roll/`, Tauri `productName`
`ReKord`). Musical-domain names (`chord_templates`, …) stay "chord". The repo
folder is still `chord`.

Real-time PC-audio (WASAPI loopback) note recognition (Okarina S/M/L ONNX
models — an unofficial Harmonica derivative trained in the separate
`harmonica-repro` project) plus rule-based chord-template
matching (no ML chord model exists for the ~300 tension-chord vocabulary).

## Layout

- `rust/` — Cargo workspace (`core/`, `cli/`, `beat_dbn/`).
  → [rust/CLAUDE.md](rust/CLAUDE.md)
- `rekord-piano-roll/` — Tauri + Svelte 5 + Pixi.js UI (live mode; use the CLI
  for files). → [rekord-piano-roll/CLAUDE.md](rekord-piano-roll/CLAUDE.md)
- `docs/notes/{rust,frontend}.md` — detailed design notes and decision history.
  **Consult only when working on that area**, not by default.
- `docs/chord-layers.md` — 3-layer chord estimate design/evaluation (Japanese).
- `TODO.md` — backlog; remove an item once implemented.
- `sample_audio/` — three real C-major piano recordings for validation.

## Build / run

```bash
cd rust && cargo build --release
./target/release/rekord_engine.exe ../sample_audio/piano_progression_C_Am_F_G.wav
./target/release/rekord_engine.exe --live
```

```powershell
Set-Location rekord-piano-roll; npm run tauri dev
```

## Testing

- Golden-vector tests (vs `harmonica-repro`) cover notes and beats; they skip
  without the golden data. Pure-function tests: `cargo test -p rekord_core
  --release --lib`, `node rekord-piano-roll/dev/chordlane.test.ts`,
  `node rekord-piano-roll/dev/snap.test.ts`.
- Everything else is validated empirically on `sample_audio/`. **Never validate
  with synthetic sine-wave audio** (harmonics cause false tensions). After
  changing note streaming/decode/clock logic, re-run all three files; for UI
  changes, run a real live session. All samples are C major, so cover key logic
  by ear in another key.

## Lint / format

- Rust: `cargo fmt --all` and `cargo clippy --release --all-targets` (workspace
  lints in `rust/Cargo.toml`); keep both clean.
- Frontend: Biome. `npm run lint` / `npm run format` in `rekord-piano-roll/`
  (config `biome.json`; `.svelte` unused-variable rules are off because Biome
  can't see template usage — `npm run check` covers that).

## Working conventions

- The user reviews UI/timing changes in the **real running app**; expect
  iterative "watch live, report" rounds for `pianoroll.ts` or capture timing.
- Prefer empirical verification over documentation when they conflict.
- WASAPI loopback allows several taps: run a second `rekord_engine.exe --live`
  beside the app to diagnose live behavior yourself (temporary env-gated
  `eprintln!`; remove it afterwards, never commit it).
