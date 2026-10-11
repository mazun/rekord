import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ChordLayerEvent, LaneUpdate } from "./chordlane";
import type { BeatLines, ConfirmedNote, StartedNote } from "./pianoroll";
import { recording, type DisplayState, type RecordingInfo } from "./recording";
import { settings, type Accidental } from "./settings.svelte";
import type { ChordVocabulary } from "./tensions";
import { stopPlayback } from "./transport.svelte";

// The model actually running, after "auto" and any DirectML->CPU fallback the backend
// applies -- see NoteModelConfig/run_live_session in lib.rs. Distinct from
// `settings.modelSize`, which is just the user's request.
export interface NoteModel {
  size: "small" | "medium" | "large";
  device: "directml" | "cpu";
  future_frames: number;
  // A fixed choice that may not keep up with real time on this hardware (large without a discrete GPU).
  over_budget: boolean;
}

type PipelineEvent =
  | { type: "Started"; total_latency_s: number; note_model: NoteModel; chord_vocabulary: ChordVocabulary }
  | ({ type: "NoteStarted" } & StartedNote)
  | ({ type: "NoteConfirmed" } & ConfirmedNote)
  | ({ type: "ChordLayer" } & ChordLayerEvent)
  | { type: "ActiveNotes"; changes: { t: number; pcs: number }[] }
  | { type: "KeyChange"; t: number; key_name: string; tonic_pc: number }
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
  /** The relative-major pitch class of the key shown (null: none yet). */
  keyPc: null as number | null,
  chord: "--",
  roman: EMPTY_ROMAN,
  tensions: [] as string[],
  bpm: null as number | null,
  // The last capture left audio that can be played back (mirrors `recording.hasAudio`, which
  // isn't reactive).
  hasRecording: false,
  // Set on every "Started" event and left alone on stop -- the settings
  // page (the only place this is read) is only reachable while capture is
  // stopped, so it needs to keep showing what the last session actually
  // ran (e.g. a CPU fallback) rather than resetting to null.
  noteModel: null as NoteModel | null,
});

/** Shows the chord/key/tension/tempo readouts as they were at one moment of the recording (live:
 * the latest, i.e. `recording.stateAt(Infinity)`; replaying: at the playhead). Only what changed
 * is assigned, so calling it every frame costs nothing downstream. */
export function showDisplay(state: DisplayState): void {
  const chord = state.chord ?? "--";
  const roman = state.roman ?? EMPTY_ROMAN;
  const keyName = state.keyName ?? "--";
  if (pipeline.chord !== chord) pipeline.chord = chord;
  if (pipeline.roman !== roman) pipeline.roman = roman;
  if (pipeline.keyName !== keyName) pipeline.keyName = keyName;
  if (pipeline.keyPc !== state.keyPc) pipeline.keyPc = state.keyPc;
  // A $state array is a proxy, so compare by content, not identity.
  const same = state.tensions.length === pipeline.tensions.length && state.tensions.every((v, i) => v === pipeline.tensions[i]);
  if (!same) pipeline.tensions = state.tensions;
  if (pipeline.bpm !== state.bpm) pipeline.bpm = state.bpm;
}

// Mirrors rekord_core::spelling::key_pair_name (the "X Major (Y Minor)" key name) -- the
// manual-key menu offers the same 12 relative major/minor pairs the automatic estimator can
// ever display, so switching to a manual pick never shows a label Auto mode couldn't have
// shown itself. Auto: majors by key signature on the flat side (G♭, not F♯), relative minors
// with the fewest accidentals (C♯m, F♯m, G♯m but E♭m, B♭m).
const SHARP_NAMES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const FLAT_NAMES = ["C", "D♭", "D", "E♭", "E", "F", "G♭", "G", "A♭", "A", "B♭", "B"];
const AUTO_MINOR_KEY_NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "G♯", "A", "B♭", "B"];

export function keyPairLabel(relativeMajorPc: number, mode: Accidental = settings.accidental): string {
  const minorPc = (relativeMajorPc + 9) % 12; // -3 semitones, wrapped
  const major = mode === "sharp" ? SHARP_NAMES : FLAT_NAMES;
  const minor = mode === "sharp" ? SHARP_NAMES : mode === "flat" ? FLAT_NAMES : AUTO_MINOR_KEY_NAMES;
  return `${major[relativeMajorPc]} Major (${minor[minorPc]} Minor)`;
}

export interface PipelineHandlers {
  onStarted(totalLatencyS: number): void;
  onNoteStarted(note: StartedNote): void;
  onNote(note: ConfirmedNote): void;
  onBeatLines(lines: BeatLines): void;
  /** Capture was stopped; the piano roll should freeze. `info` is the audio now available to
   * play back (null if none). */
  onStopped(info: RecordingInfo | null): void;
  /** The chord lane changed (a layer decided something): from `update.fromS` on it is
   * `update.segments`. */
  onChordLane(update: LaneUpdate): void;
}

// Note, beat-line and clock events bypass reactive state entirely and go straight to
// the piano roll -- notes arrive far too often to be worth routing
// through $state, and nothing else in the UI renders them.
let onStopped: ((info: RecordingInfo | null) => void) | null = null;

export async function connectPipeline(handlers: PipelineHandlers): Promise<UnlistenFn> {
  onStopped = handlers.onStopped;
  const unlistenEvent = await listen<PipelineEvent>("pipeline-event", (event) => {
    const payload = event.payload;
    // Everything is logged with its time (for replaying it); the readouts below show the latest.
    recording.apply(payload);
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
    } else if (payload.type === "NoteStarted") {
      handlers.onNoteStarted({
        start_s: payload.start_s,
        pitch_midi: payload.pitch_midi,
        velocity: payload.velocity,
        part: payload.part,
      });
    } else if (payload.type === "NoteConfirmed") {
      handlers.onNote({
        start_s: payload.start_s,
        end_s: payload.end_s,
        pitch_midi: payload.pitch_midi,
        velocity: payload.velocity,
        part: payload.part,
      });
    } else if (payload.type === "ChordLayer") {
      const update = recording.lane.takeUpdate();
      if (update) handlers.onChordLane(update);
    } else if (payload.type === "BeatLines") {
      handlers.onBeatLines(payload);
    }
    // The chord, key, tension and tempo readouts (all but the notes): show the latest.
    if (payload.type !== "Started" && payload.type !== "NoteStarted" && payload.type !== "NoteConfirmed") {
      showDisplay(recording.stateAt(Infinity));
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
  // Playback has to be stopped first: its output would be captured, and the recording it plays
  // is about to be replaced.
  await stopPlayback();
  pipeline.chord = "--";
  pipeline.keyName = "--";
  pipeline.keyPc = null;
  pipeline.roman = EMPTY_ROMAN;
  pipeline.tensions = [];
  pipeline.bpm = null;
  pipeline.hasRecording = false;
  pipeline.status = "running";
  try {
    // The piano roll's reset happens on the "Started" event, once the
    // pipeline has actually begun, not here.
    await invoke("start_capture", {
      modelSize: settings.modelSize,
      futureFrames: settings.futureFrames,
      accidental: settings.accidental,
    });
  } catch (err) {
    console.error(err);
    pipeline.status = "idle";
  }
}

export async function stopCapture(): Promise<void> {
  pipeline.status = "stopping";
  let info: RecordingInfo | null = null;
  try {
    // What audio the backend kept (null if none, or when running without a backend).
    info = (await invoke<RecordingInfo | null>("stop_capture")) ?? null;
  } finally {
    pipeline.status = "idle";
    recording.seal(info);
    pipeline.hasRecording = recording.hasAudio;
    onStopped?.(info);
  }
}

export async function setKeyMode(tonicPc: number | null): Promise<void> {
  // Auto mode's label only refreshes on the next KeyChange event (i.e.
  // once capture is running and a checkpoint fires); a manual pick's
  // label is already fully known, so show it immediately instead of
  // waiting on that same round-trip.
  if (tonicPc !== null) {
    pipeline.keyName = keyPairLabel(tonicPc);
    pipeline.keyPc = tonicPc;
  }
  try {
    await invoke("set_key_mode", { tonicPc });
  } catch (err) {
    console.error(err);
  }
}

export async function setAccidental(): Promise<void> {
  // While capture runs the backend re-announces the chord and key in the new spelling. A key
  // already on screen from before (or while stopped, when nothing is re-announced) is
  // respelled here.
  if (pipeline.keyPc !== null) pipeline.keyName = keyPairLabel(pipeline.keyPc);
  try {
    await invoke("set_accidental", { accidental: settings.accidental });
  } catch (err) {
    console.error(err);
  }
}
