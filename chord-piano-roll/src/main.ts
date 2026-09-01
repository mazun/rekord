import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { PianoRoll } from "./pianoroll";

type PipelineEvent =
  | { type: "Started" }
  | { type: "NoteConfirmed"; start_s: number; end_s: number; pitch_midi: number }
  | { type: "ChordChange"; t: number; label: string };

window.addEventListener("DOMContentLoaded", async () => {
  const container = document.querySelector<HTMLDivElement>("#pianoroll-container");
  const chordLabel = document.querySelector<HTMLDivElement>("#chord-label");
  const startBtn = document.querySelector<HTMLButtonElement>("#start-btn");
  const stopBtn = document.querySelector<HTMLButtonElement>("#stop-btn");
  const status = document.querySelector<HTMLSpanElement>("#status");
  if (!container || !chordLabel || !startBtn || !stopBtn || !status) return;

  const roll = await PianoRoll.create(container);

  await listen<PipelineEvent>("pipeline-event", (event) => {
    const payload = event.payload;
    if (payload.type === "Started") {
      // Anchor the piano roll's clock here, once model loading and audio
      // device setup are actually done -- not on the button click, which
      // is before that (sometimes noticeably before it, e.g. on a cold
      // DirectML session the first time). Anchoring too early left extra,
      // unaccounted-for delay baked into every note's position, which
      // reappeared as a gap near the playhead no fixed pipeline-latency
      // constant could account for.
      roll.reset();
      status.textContent = "listening...";
    } else if (payload.type === "NoteConfirmed") {
      roll.addNote({
        start_s: payload.start_s,
        end_s: payload.end_s,
        pitch_midi: payload.pitch_midi,
      });
    } else {
      chordLabel.textContent = payload.label;
    }
  });

  await listen<string>("pipeline-error", (event) => {
    status.textContent = `error: ${event.payload}`;
  });

  startBtn.addEventListener("click", async () => {
    chordLabel.textContent = "--";
    status.textContent = "starting...";
    startBtn.disabled = true;
    stopBtn.disabled = false;
    try {
      await invoke("start_capture");
      // roll.reset() happens on the "Started" event above, once the
      // pipeline has actually begun, not here.
    } catch (err) {
      status.textContent = `error: ${err}`;
      startBtn.disabled = false;
      stopBtn.disabled = true;
    }
  });

  stopBtn.addEventListener("click", async () => {
    stopBtn.disabled = true;
    status.textContent = "stopping...";
    try {
      await invoke("stop_capture");
    } finally {
      status.textContent = "stopped";
      startBtn.disabled = false;
    }
  });
});
