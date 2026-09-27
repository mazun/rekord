import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { BeatLines, ConfirmedNote } from "./pianoroll";
import { settings, type ModelSize } from "./settings.svelte";

// The model actually running, after any DirectML->CPU fallback the backend
// applies -- see NoteModelConfig/run_live_session in lib.rs. Distinct from
// `settings.modelSize`, which is just the user's request.
export interface NoteModel {
  size: ModelSize;
  device: "directml" | "cpu";
  future_frames: number;
}

type PipelineEvent =
  | { type: "Started"; total_latency_s: number; note_model: NoteModel }
  | ({ type: "NoteConfirmed" } & ConfirmedNote)
  | { type: "ChordChange"; t: number; label: string; roman_numeral: string | null }
  | { type: "TensionsChanged"; tensions: string[] }
  | { type: "KeyChange"; key_name: string }
  | ({ type: "BeatLines" } & BeatLines);

// "loading" until the piano roll exists and the event listener is
// registered -- Start stays disabled until then, since a "Started" event
// arriving before the listener is up would leave the roll's clock
// unanchored. "stopping" keeps Start disabled until stop_capture has
// actually returned, same as the pre-Svelte button wiring did.
export type CaptureStatus = "loading" | "idle" | "running" | "stopping";

// A non-breaking space, not "" -- an empty element's rendered height
// depends on min-height alone, which doesn't exactly match a real text
// line's height (governed by line-height/font metrics instead), so
// toggling between the two shifted the chord row's height by a couple of
// px every time silence was reported, visibly jittering the piano roll
// below it. Real (if invisible) inline content keeps the row's height
// identical either way.
const EMPTY_ROMAN = " ";

export const pipeline = $state({
  status: "loading" as CaptureStatus,
  keyName: "--",
  chord: "--",
  roman: EMPTY_ROMAN,
  tensions: [] as string[],
  bpm: null as number | null,
  // Set on every "Started" event and left alone on stop -- the settings
  // page (the only place this is read) is only reachable while capture is
  // stopped, so it needs to keep showing what the last session actually
  // ran (e.g. a CPU fallback) rather than resetting to null.
  noteModel: null as NoteModel | null,
});

// How much of the beat history the tempo is read from: long enough that the beats' 20 ms
// frame quantization averages out (~0.3 BPM at 120 BPM), short enough to follow a new song
// within a few bars.
const BPM_WINDOW_S = 10;
// The committed lines run this far behind the audio (beat_tracker.rs); the window ends there.
const COMMITTED_DELAY_S = 4.5;
// No committed beat within this of where they end means the beat tracker has stopped seeing
// a beat -- silence, or a gap between songs.
const BPM_STALE_S = 3;
const BPM_MIN_BEATS = 5;

// Tempo from the committed beat lines of one BeatLines snapshot. Only committed ones: the
// provisional lines skip beats and, mid-crossfade, briefly run at double time, while the
// DBN's decisions stay on one tempo -- and ~4.5 s of delay doesn't matter for a tempo
// readout. Takes the trailing run of beats that stay on one grid (each interval close to a
// whole number of median intervals, in case the DBN does skip one) and divides its whole
// span by the number of beats it covers, rather than taking a median interval, which would
// only ever land on the 20 ms frame steps (120 vs 125 BPM).
function estimateBpm(lines: BeatLines): number | null {
  const end = lines.from_s + 30 - COMMITTED_DELAY_S; // + BEAT_LINES_WINDOW_S in pipeline.rs
  const beats = lines.committed.map((l) => l.t).filter((t) => t > end - BPM_WINDOW_S);
  if (beats.length < BPM_MIN_BEATS || beats[beats.length - 1] < end - BPM_STALE_S) return null;
  const ioi = beats.slice(1).map((t, i) => t - beats[i]);
  const median = [...ioi].sort((a, b) => a - b)[Math.floor(ioi.length / 2)];
  let first = ioi.length;
  let count = 0;
  while (first > 0) {
    const k = Math.round(ioi[first - 1] / median);
    if (k < 1 || k > 3 || Math.abs(ioi[first - 1] / median - k) > 0.2) break;
    count += k;
    first--;
  }
  if (count < BPM_MIN_BEATS - 1) return null;
  return (60 * count) / (beats[beats.length - 1] - beats[first]);
}

// A blank this short keeps the last tempo up (the estimate drops out briefly when the beat
// lines get irregular), so the display doesn't flicker mid-song.
const BPM_HOLD_S = 4;
// A new tempo (after a blank, or differing by more than this) is only shown once two
// snapshots in a row agree on it, so a single odd estimate (e.g. mid-crossfade) never
// reaches the screen.
const BPM_JUMP = 0.03;
let bpmSeenAt = -Infinity;
let bpmCandidate: number | null = null;

function updateBpm(lines: BeatLines) {
  const now = lines.from_s;
  const bpm = estimateBpm(lines);
  if (bpm === null) {
    if (now - bpmSeenAt > BPM_HOLD_S) pipeline.bpm = null;
    return;
  }
  bpmSeenAt = now;
  const shown = pipeline.bpm;
  const agrees = (a: number | null) => a !== null && Math.abs(bpm - a) <= BPM_JUMP * a;
  if (agrees(shown) || agrees(bpmCandidate)) {
    // Hysteresis: a tempo hovering around x.5 would otherwise flicker between two values.
    if (shown === null || Math.abs(bpm - shown) >= 0.8) pipeline.bpm = Math.round(bpm);
    bpmCandidate = null;
  } else {
    bpmCandidate = bpm;
  }
}

// Mirrors rekord_core::chord_templates::NOTE_NAMES and
// KeyEstimate::name()'s "X Major (Y Minor)" formatting -- the manual-key
// menu offers the same 12 relative major/minor pairs the automatic
// estimator can ever display, so switching to a manual pick never shows a
// label the Auto mode couldn't have shown itself.
const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
export function keyPairLabel(relativeMajorPc: number): string {
  const minorPc = (relativeMajorPc + 9) % 12; // -3 semitones, wrapped
  return `${NOTE_NAMES[relativeMajorPc]} Major (${NOTE_NAMES[minorPc]} Minor)`;
}

export interface PipelineHandlers {
  onStarted(totalLatencyS: number): void;
  onNote(note: ConfirmedNote): void;
  onBeatLines(lines: BeatLines): void;
}

// Note, beat-line and clock events bypass reactive state entirely and go straight to
// the piano roll -- notes arrive far too often to be worth routing
// through $state, and nothing else in the UI renders them.
export async function connectPipeline(handlers: PipelineHandlers): Promise<UnlistenFn> {
  const unlistenEvent = await listen<PipelineEvent>("pipeline-event", (event) => {
    const payload = event.payload;
    if (payload.type === "Started") {
      // Anchor the piano roll's clock here, once model loading and audio
      // device setup are actually done -- not on the button click, which
      // is before that (sometimes noticeably before it, e.g. on a cold
      // DirectML session the first time). Anchoring too early left extra,
      // unaccounted-for delay baked into every note's position, which
      // reappeared as a gap near the playhead no fixed pipeline-latency
      // constant could account for.
      pipeline.noteModel = payload.note_model;
      handlers.onStarted(payload.total_latency_s);
    } else if (payload.type === "NoteConfirmed") {
      handlers.onNote({
        start_s: payload.start_s,
        end_s: payload.end_s,
        pitch_midi: payload.pitch_midi,
        velocity: payload.velocity,
        melody: payload.melody,
        bass: payload.bass,
      });
    } else if (payload.type === "ChordChange") {
      pipeline.chord = payload.label;
      pipeline.roman = payload.roman_numeral ?? EMPTY_ROMAN;
    } else if (payload.type === "TensionsChanged") {
      pipeline.tensions = payload.tensions;
    } else if (payload.type === "BeatLines") {
      handlers.onBeatLines(payload);
      updateBpm(payload);
    } else {
      pipeline.keyName = payload.key_name;
    }
  });
  const unlistenError = await listen<string>("pipeline-error", (event) => {
    console.error(event.payload);
  });
  pipeline.status = "idle";
  return () => {
    unlistenEvent();
    unlistenError();
  };
}

export async function startCapture(): Promise<void> {
  pipeline.chord = "--";
  pipeline.keyName = "--";
  pipeline.roman = EMPTY_ROMAN;
  pipeline.tensions = [];
  pipeline.bpm = null;
  bpmSeenAt = -Infinity;
  bpmCandidate = null;
  pipeline.status = "running";
  try {
    // The piano roll's reset happens on the "Started" event, once the
    // pipeline has actually begun, not here.
    await invoke("start_capture", {
      modelSize: settings.modelSize,
      futureFrames: settings.futureFrames,
    });
  } catch (err) {
    console.error(err);
    pipeline.status = "idle";
  }
}

export async function stopCapture(): Promise<void> {
  pipeline.status = "stopping";
  try {
    await invoke("stop_capture");
  } finally {
    pipeline.status = "idle";
  }
}

export async function setKeyMode(tonicPc: number | null): Promise<void> {
  // Auto mode's label only refreshes on the next KeyChange event (i.e.
  // once capture is running and a checkpoint fires); a manual pick's
  // label is already fully known, so show it immediately instead of
  // waiting on that same round-trip.
  if (tonicPc !== null) {
    pipeline.keyName = keyPairLabel(tonicPc);
  }
  try {
    await invoke("set_key_mode", { tonicPc });
  } catch (err) {
    console.error(err);
  }
}
