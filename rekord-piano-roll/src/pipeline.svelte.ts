import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ConfirmedNote } from "./pianoroll";

type PipelineEvent =
  | { type: "Started"; total_latency_s: number }
  | ({ type: "NoteConfirmed" } & ConfirmedNote)
  | { type: "ChordChange"; t: number; label: string; roman_numeral: string | null }
  | { type: "TensionsChanged"; tensions: string[] }
  | { type: "KeyChange"; key_name: string };

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
});

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
}

// Note and clock events bypass reactive state entirely and go straight to
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

export async function startCapture(latencyMs: number): Promise<void> {
  pipeline.chord = "--";
  pipeline.keyName = "--";
  pipeline.roman = EMPTY_ROMAN;
  pipeline.tensions = [];
  pipeline.status = "running";
  try {
    // The piano roll's reset happens on the "Started" event, once the
    // pipeline has actually begun, not here.
    await invoke("start_capture", { latencySeconds: latencyMs / 1000 });
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
