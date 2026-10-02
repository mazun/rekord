// User preferences persisted to localStorage. Session-only conveniences,
// not correctness-critical values (the backend re-validates the model
// settings regardless -- see start_capture in lib.rs), so a missing or
// garbled stored value just falls back to the shipped default.

const MODEL_SIZE_STORAGE_KEY = "chord.modelSize";
const FUTURE_FRAMES_STORAGE_KEY = "chord.futureFrames";
const PIANOROLL_VISIBLE_STORAGE_KEY = "chord.pianorollVisible";
const ACCIDENTAL_STORAGE_KEY = "chord.accidental";

// "mix" is the large network with three output parts (vocal/bass/other) trained for full
// mixes; the others are single-part models.
export type ModelSize = "small" | "medium" | "large" | "mix";

// How black-key pitch classes are spelled in chord and key names: "auto" picks the most natural
// spelling per chord and key (rules in rust/core/src/spelling.rs).
export type Accidental = "auto" | "sharp" | "flat";

const DEFAULT_MODEL_SIZE: ModelSize = "mix";
const DEFAULT_FUTURE_FRAMES = 8;

function loadModelSize(): ModelSize {
  const stored = localStorage.getItem(MODEL_SIZE_STORAGE_KEY);
  return stored === "small" || stored === "medium" || stored === "large" || stored === "mix" ? stored : DEFAULT_MODEL_SIZE;
}

function loadAccidental(): Accidental {
  const stored = localStorage.getItem(ACCIDENTAL_STORAGE_KEY);
  return stored === "sharp" || stored === "flat" ? stored : "auto";
}

function loadFutureFrames(): number {
  const stored = Number(localStorage.getItem(FUTURE_FRAMES_STORAGE_KEY));
  return stored === 8 || stored === 16 ? stored : DEFAULT_FUTURE_FRAMES;
}

export const settings = $state({
  modelSize: loadModelSize(),
  futureFrames: loadFutureFrames(),
  pianorollVisible: localStorage.getItem(PIANOROLL_VISIBLE_STORAGE_KEY) !== "false",
  accidental: loadAccidental(),
});

export function saveModelSize(): void {
  localStorage.setItem(MODEL_SIZE_STORAGE_KEY, settings.modelSize);
}

export function saveFutureFrames(): void {
  localStorage.setItem(FUTURE_FRAMES_STORAGE_KEY, String(settings.futureFrames));
}

export function saveAccidental(): void {
  localStorage.setItem(ACCIDENTAL_STORAGE_KEY, settings.accidental);
}

export function savePianorollVisible(): void {
  localStorage.setItem(PIANOROLL_VISIBLE_STORAGE_KEY, String(settings.pianorollVisible));
}
