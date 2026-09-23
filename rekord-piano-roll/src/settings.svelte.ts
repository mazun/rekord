// User preferences persisted to localStorage. Session-only conveniences,
// not correctness-critical values (the backend clamps latency regardless),
// so a missing or garbled stored value just falls back to the shipped
// default rather than needing validation.

const LATENCY_STORAGE_KEY = "chord.latencyMs";
const PIANOROLL_VISIBLE_STORAGE_KEY = "chord.pianorollVisible";

const DEFAULT_LATENCY_MS = 250;

function loadLatencyMs(): number {
  const stored = Number(localStorage.getItem(LATENCY_STORAGE_KEY));
  return stored > 0 ? stored : DEFAULT_LATENCY_MS;
}

export const settings = $state({
  latencyMs: loadLatencyMs(),
  pianorollVisible: localStorage.getItem(PIANOROLL_VISIBLE_STORAGE_KEY) !== "false",
});

export function saveLatency(): void {
  localStorage.setItem(LATENCY_STORAGE_KEY, String(settings.latencyMs));
}

export function savePianorollVisible(): void {
  localStorage.setItem(PIANOROLL_VISIBLE_STORAGE_KEY, String(settings.pianorollVisible));
}
