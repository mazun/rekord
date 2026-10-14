// User preferences persisted to localStorage. Session-only conveniences,
// not correctness-critical values (the backend re-validates the model
// settings regardless -- see start_capture in lib.rs), so a missing or
// garbled stored value just falls back to the shipped default.

// A new key: the old "chord.modelSize" held the Harmonica sizes ("mix", "large", ...), whose names
// now mean something else (Okarina L is not Harmonica large).
const MODEL_SIZE_STORAGE_KEY = "chord.noteModel";
const FUTURE_FRAMES_STORAGE_KEY = "chord.futureFrames";
const PIANOROLL_VISIBLE_STORAGE_KEY = "chord.pianorollVisible";
const DRUMS_VISIBLE_STORAGE_KEY = "chord.drumsVisible";
const ACCIDENTAL_STORAGE_KEY = "chord.accidental";

// The Okarina note models S/M/L ("small"/"medium"/"large"; all label vocal/bass/other), or "auto":
// the backend picks by hardware (large on a discrete GPU, medium on an integrated GPU or a fast CPU,
// else small).
export type ModelSize = "auto" | "small" | "medium" | "large";

// How black-key pitch classes are spelled in chord and key names: "auto" picks the most natural
// spelling per chord and key (rules in rust/core/src/spelling.rs).
export type Accidental = "auto" | "sharp" | "flat";

const DEFAULT_MODEL_SIZE: ModelSize = "auto";
const DEFAULT_FUTURE_FRAMES = 8;

function loadModelSize(): ModelSize {
  const stored = localStorage.getItem(MODEL_SIZE_STORAGE_KEY);
  return stored === "auto" || stored === "small" || stored === "medium" || stored === "large"
    ? stored
    : DEFAULT_MODEL_SIZE;
}

function loadAccidental(): Accidental {
  const stored = localStorage.getItem(ACCIDENTAL_STORAGE_KEY);
  return stored === "sharp" || stored === "flat" ? stored : "auto";
}

// Playback mix (volumes are 0..1 slider positions; the backend squares them): the recorded audio,
// and the notes synthesized on top of it -- an overall volume plus, per model part, a wave shape and
// a volume. Untuned defaults: the lead stands out as a square, the bass is a soft triangle, the
// accompaniment a plain sine (what chord recognition reads best).
const VOLUME_ORIGINAL_STORAGE_KEY = "chord.volumeOriginal";
const VOLUME_NOTES_STORAGE_KEY = "chord.volumeSine"; // named from when the notes were always sine waves
const VOICES_STORAGE_KEY = "chord.voices";
const DEFAULT_VOLUME_ORIGINAL = 0.8;
const DEFAULT_VOLUME_NOTES = 0.5;

export type Wave = "sine" | "triangle" | "square" | "saw";
export const WAVES: readonly Wave[] = ["sine", "triangle", "square", "saw"];

/** The parts notes are labeled with. */
export const PARTS = ["vocal", "bass", "other"] as const;
export type NotePart = (typeof PARTS)[number];

export interface Voice {
  wave: Wave;
  volume: number;
  muted: boolean;
}

const DEFAULT_VOICES: Record<NotePart, Voice> = {
  vocal: { wave: "square", volume: 1, muted: false },
  bass: { wave: "triangle", volume: 1, muted: false },
  other: { wave: "sine", volume: 1, muted: false },
};

function loadVolume(key: string, fallback: number): number {
  const stored = localStorage.getItem(key);
  const value = stored === null ? NaN : Number(stored);
  return Number.isFinite(value) ? Math.min(1, Math.max(0, value)) : fallback;
}

// Wave and volume per part; mute is deliberately not remembered.
function loadVoices(): Record<NotePart, Voice> {
  const voices = structuredClone(DEFAULT_VOICES);
  try {
    const stored = JSON.parse(localStorage.getItem(VOICES_STORAGE_KEY) ?? "{}");
    for (const part of PARTS) {
      const { wave, volume } = stored?.[part] ?? {};
      if (WAVES.includes(wave)) voices[part].wave = wave;
      if (typeof volume === "number" && Number.isFinite(volume))
        voices[part].volume = Math.min(1, Math.max(0, volume));
    }
  } catch {
    // garbled: defaults
  }
  return voices;
}

function loadFutureFrames(): number {
  const stored = Number(localStorage.getItem(FUTURE_FRAMES_STORAGE_KEY));
  return stored === 8 || stored === 16 ? stored : DEFAULT_FUTURE_FRAMES;
}

// Piano roll time-axis zoom as a slider position in [-1, 1] with the shipped scale (1x) at the
// centre: the left half runs 0.5x..1x, the right half 1x..4x.
const ROLL_ZOOM_STORAGE_KEY = "chord.rollZoomPos";

export function zoomScale(position: number): number {
  return position < 0 ? 2 ** position : 4 ** position;
}

function loadRollZoom(): number {
  const value = Number(localStorage.getItem(ROLL_ZOOM_STORAGE_KEY));
  return Number.isFinite(value) ? Math.min(1, Math.max(-1, value)) : 0;
}

export function saveRollZoom(): void {
  localStorage.setItem(ROLL_ZOOM_STORAGE_KEY, String(settings.rollZoom));
}

export const settings = $state({
  rollZoom: loadRollZoom(),
  modelSize: loadModelSize(),
  futureFrames: loadFutureFrames(),
  // The keyboard (pitch rows, keys and notes) and the drum lane are shown independently; with both
  // hidden the window collapses to the readouts above.
  pianorollVisible: localStorage.getItem(PIANOROLL_VISIBLE_STORAGE_KEY) !== "false",
  drumsVisible: localStorage.getItem(DRUMS_VISIBLE_STORAGE_KEY) !== "false",
  accidental: loadAccidental(),
  volumeOriginal: loadVolume(VOLUME_ORIGINAL_STORAGE_KEY, DEFAULT_VOLUME_ORIGINAL),
  volumeNotes: loadVolume(VOLUME_NOTES_STORAGE_KEY, DEFAULT_VOLUME_NOTES),
  muteOriginal: false,
  muteNotes: false,
  voices: loadVoices(),
});

export function saveVolumes(): void {
  localStorage.setItem(VOLUME_ORIGINAL_STORAGE_KEY, String(settings.volumeOriginal));
  localStorage.setItem(VOLUME_NOTES_STORAGE_KEY, String(settings.volumeNotes));
  const voices = Object.fromEntries(
    Object.entries(settings.voices).map(([part, v]) => [part, { wave: v.wave, volume: v.volume }]),
  );
  localStorage.setItem(VOICES_STORAGE_KEY, JSON.stringify(voices));
}

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

export function saveDrumsVisible(): void {
  localStorage.setItem(DRUMS_VISIBLE_STORAGE_KEY, String(settings.drumsVisible));
}
