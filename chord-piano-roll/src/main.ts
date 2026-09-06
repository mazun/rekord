import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { PianoRoll } from "./pianoroll";

type PipelineEvent =
  | { type: "Started"; total_latency_s: number }
  | { type: "NoteConfirmed"; start_s: number; end_s: number; pitch_midi: number; velocity: number }
  | { type: "ChordChange"; t: number; label: string; roman_numeral: string | null }
  | { type: "TensionsChanged"; tensions: string[] }
  | { type: "KeyChange"; key_name: string };

const LATENCY_STORAGE_KEY = "chord.latencyMs";

// Mirrors chord_core::chord_templates::NOTE_NAMES and
// KeyEstimate::name()'s "X Major (Y Minor)" formatting -- the manual-key
// dropdown offers the same 12 relative major/minor pairs the automatic
// estimator can ever display, so switching to a manual pick never shows a
// label the Auto mode couldn't have shown itself.
const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
function keyPairLabel(relativeMajorPc: number): string {
  const minorPc = (relativeMajorPc + 9) % 12; // -3 semitones, wrapped
  return `${NOTE_NAMES[relativeMajorPc]} Major (${NOTE_NAMES[minorPc]} Minor)`;
}

window.addEventListener("DOMContentLoaded", async () => {
  const container = document.querySelector<HTMLDivElement>("#pianoroll-container");
  const keyLabel = document.querySelector<HTMLDivElement>("#key-label");
  const chordLabel = document.querySelector<HTMLDivElement>("#chord-label");
  const romanLabel = document.querySelector<HTMLDivElement>("#roman-label");
  const startBtn = document.querySelector<HTMLButtonElement>("#start-btn");
  const stopBtn = document.querySelector<HTMLButtonElement>("#stop-btn");
  const status = document.querySelector<HTMLSpanElement>("#status");
  const latencyInput = document.querySelector<HTMLInputElement>("#latency-input");
  const latencyValue = document.querySelector<HTMLSpanElement>("#latency-value");
  const keySelect = document.querySelector<HTMLSelectElement>("#key-select");
  if (
    !container ||
    !keyLabel ||
    !chordLabel ||
    !romanLabel ||
    !startBtn ||
    !stopBtn ||
    !status ||
    !latencyInput ||
    !latencyValue ||
    !keySelect
  )
    return;

  for (let pc = 0; pc < 12; pc++) {
    const opt = document.createElement("option");
    opt.value = String(pc);
    opt.textContent = keyPairLabel(pc);
    keySelect.appendChild(opt);
  }

  keySelect.addEventListener("change", async () => {
    const tonicPc = keySelect.value === "auto" ? null : Number(keySelect.value);
    // Auto mode's label only refreshes on the next KeyChange event (i.e.
    // once capture is running and a checkpoint fires); a manual pick's
    // label is already fully known, so show it immediately instead of
    // waiting on that same round-trip.
    if (tonicPc !== null) {
      keyLabel.textContent = keyPairLabel(tonicPc);
    }
    try {
      await invoke("set_key_mode", { tonicPc });
    } catch (err) {
      status.textContent = `error: ${err}`;
    }
  });

  // Restore a previously-chosen latency, if any -- session-only convenience,
  // not a correctness-critical value (the backend clamps regardless).
  const storedMs = localStorage.getItem(LATENCY_STORAGE_KEY);
  if (storedMs) {
    latencyInput.value = storedMs;
  }
  latencyValue.textContent = latencyInput.value;

  latencyInput.addEventListener("input", () => {
    latencyValue.textContent = latencyInput.value;
  });
  latencyInput.addEventListener("change", () => {
    localStorage.setItem(LATENCY_STORAGE_KEY, latencyInput.value);
  });

  const roll = await PianoRoll.create(container);

  const tensionChips = new Map<string, HTMLElement>();
  document.querySelectorAll<HTMLElement>(".chip[data-tension]").forEach((chip) => {
    const key = chip.dataset.tension;
    if (key) tensionChips.set(key, chip);
  });

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
      roll.reset(payload.total_latency_s);
      status.textContent = "listening...";
    } else if (payload.type === "NoteConfirmed") {
      roll.addNote({
        start_s: payload.start_s,
        end_s: payload.end_s,
        pitch_midi: payload.pitch_midi,
        velocity: payload.velocity,
      });
    } else if (payload.type === "ChordChange") {
      chordLabel.textContent = payload.label;
      // A non-breaking space, not "" -- an empty element's rendered
      // height depends on min-height alone, which doesn't exactly match
      // a real text line's height (governed by line-height/font metrics
      // instead), so toggling between the two shifted this row's height
      // by a couple of px every time silence was reported, visibly
      // jittering the piano roll below it. Real (if invisible) inline
      // content keeps the row's height identical either way.
      romanLabel.textContent = payload.roman_numeral ?? " ";
    } else if (payload.type === "TensionsChanged") {
      const active = new Set(payload.tensions);
      tensionChips.forEach((chip, key) => {
        chip.classList.toggle("lit", active.has(key));
      });
    } else {
      keyLabel.textContent = payload.key_name;
    }
  });

  await listen<string>("pipeline-error", (event) => {
    status.textContent = `error: ${event.payload}`;
  });

  startBtn.addEventListener("click", async () => {
    chordLabel.textContent = "--";
    keyLabel.textContent = "--";
    romanLabel.textContent = " ";
    tensionChips.forEach((chip) => chip.classList.remove("lit"));
    status.textContent = "starting...";
    startBtn.disabled = true;
    stopBtn.disabled = false;
    latencyInput.disabled = true;
    try {
      const latencySeconds = Number(latencyInput.value) / 1000;
      await invoke("start_capture", { latencySeconds });
      // roll.reset() happens on the "Started" event above, once the
      // pipeline has actually begun, not here.
    } catch (err) {
      status.textContent = `error: ${err}`;
      startBtn.disabled = false;
      stopBtn.disabled = true;
      latencyInput.disabled = false;
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
      latencyInput.disabled = false;
    }
  });
});
