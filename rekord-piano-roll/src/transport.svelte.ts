import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { NotePart, PianoRoll } from "./pianoroll";
import { pipeline, showDisplay, stopCapture } from "./pipeline.svelte";
import { recording } from "./recording";
import { settings, type VoicePart } from "./settings.svelte";

// Replaying the recording (the last HISTORY_S seconds of audio the backend kept, plus the notes
// shown on the roll, synthesized). The backend plays the sound and reports which recording
// time is audible at which wall-clock instant; the roll follows that (PlaybackClock).
//
//   stopped --Play--> playing <--Play--> paused
//   playing --end of recording--> stopped
//   playing / paused --start recording, open settings--> stopped
//
// Capture and playback exclude each other: the loopback would record the playback. Pressing Play
// is only possible while capture is stopped, and starting capture stops playback first.

export type PlaybackState = "stopped" | "playing" | "paused";

// `marked`: a marker is placed on the roll (mirrors PianoRoll.marker, which is not reactive). While
// capture runs it is what lets Play stop the capture and start from there.
// `loop`: the Repeat button; while on, playing with a range selected returns to its left edge on
// reaching its right edge.
export const transport = $state({ state: "stopped" as PlaybackState, marked: false, loop: false });

const LOOP_POLL_MS = 20;

function setMarker(t: number | null): void {
  roll?.setMarker(t);
  transport.marked = t !== null;
}

// Starting this close to the end of the recording (stopped, nothing chosen) plays from the left
// edge of the roll instead: what the user was just looking at, rather than a fraction of a second.
// Right after stopping, the roll's playhead is about the display lag (~0.9 s) short of the end.
const NEAR_END_S = 1.5;

type PlaybackEvent =
  | { type: "Position"; gen: number; pos_s: number; at_epoch_ms: number }
  | { type: "Ended"; gen: number };

let roll: PianoRoll | null = null;
// Identifies the latest play/seek command: reports of an older one are stale.
let gen = 0;
// Parts hidden on the roll are not sounded either.
const hiddenParts = new Set<NotePart>();

const VOICE_PARTS: VoicePart[] = ["vocal", "bass", "other"];

// What the backend mixes: the muted and the hidden are sent as gain 0.
function params() {
  return {
    original_gain: settings.muteOriginal ? 0 : settings.volumeOriginal,
    notes_gain: settings.muteNotes ? 0 : settings.volumeNotes,
    voices: VOICE_PARTS.map((part) => {
      const voice = settings.voices[part];
      const silent = voice.muted || hiddenParts.has(part);
      return { part, wave: voice.wave, gain: silent ? 0 : voice.volume };
    }),
  };
}

function clampToRecording(t: number): number {
  return Math.min(recording.endS, Math.max(recording.startS, t));
}

/** Wires the transport to the piano roll and the backend. */
export async function connectTransport(pianoRoll: PianoRoll): Promise<UnlistenFn> {
  roll = pianoRoll;
  roll.onSeekRequest = (t) => void seekTo(t);
  roll.onPlayheadTime = (t) => {
    // The readouts follow the playhead whenever there is something to replay; while capture is
    // just stopped they keep showing the latest.
    if (transport.state !== "stopped") showDisplay(recording.stateAt(t));
  };
  const unlisten = await listen<PlaybackEvent>("playback-event", (event) => {
    const payload = event.payload;
    if (payload.gen !== gen) return; // a command since superseded this one
    if (payload.type === "Position") {
      roll?.onPlaybackPosition(payload.pos_s, payload.at_epoch_ms);
    } else if (transport.state === "playing") {
      // The last frame has been heard.
      transport.state = "stopped";
      roll?.stopPlayback(recording.endS);
      showDisplay(recording.stateAt(recording.endS));
    }
  });
  // The cursor is extrapolated from the backend's position reports, so it is read on a timer
  // rather than waiting for the next report.
  const loopTimer = setInterval(() => {
    if (!roll || !transport.loop || transport.state !== "playing") return;
    const range = roll.selectedRange;
    if (range && roll.playheadS() >= range.endS) void playFrom(range.startS);
  }, LOOP_POLL_MS);
  return () => {
    clearInterval(loopTimer);
    unlisten();
    roll = null;
  };
}

// Where Play starts: the marker the user clicked, else the roll's playhead (so it resumes from
// where a pause or scroll left it); a stopped recording sitting at its end plays from the left
// edge instead.
function startPositionS(): number {
  if (!roll) return recording.startS;
  const wanted = roll.marker ?? roll.playheadS();
  if (transport.state === "stopped" && wanted >= recording.endS - NEAR_END_S) {
    return clampToRecording(roll.leftEdgeS());
  }
  return clampToRecording(wanted);
}

// Starts playing from `fromS`: jumps within the playback in progress, or opens a new one when there
// is none (or it has ended).
async function playFrom(fromS: number): Promise<void> {
  if (!roll) return;
  // Repeating a range: starting right of it goes to its left edge.
  const range = roll.selectedRange;
  if (transport.loop && range && fromS >= range.endS) fromS = range.startS;
  const myGen = ++gen;
  const hadPlayback = transport.state !== "stopped";
  setMarker(null);
  transport.state = "playing";
  roll.startPlayback(fromS);
  if (hadPlayback) {
    try {
      await invoke("playback_seek", { posS: fromS, gen: myGen });
      return;
    } catch {
      // The playback had just ended: open a new one below.
    }
  }
  try {
    await invoke("playback_start", { fromS, notes: recording.synthNotes(), params: params(), gen: myGen });
  } catch (err) {
    console.error(err);
    if (gen === myGen) {
      transport.state = "stopped";
      roll.stopPlayback(fromS);
    }
  }
}

/** The Play button: start, pause or resume. */
export async function togglePlayback(): Promise<void> {
  if (!roll) return;
  if (pipeline.status === "running") {
    // Capture is running and a marker is placed: stop capturing, then play from the marker.
    if (!transport.marked) return;
    await stopCapture();
    if (recording.hasAudio) await playFrom(startPositionS());
    return;
  }
  if (transport.state === "stopped") {
    if (recording.hasAudio) await playFrom(startPositionS());
  } else if (transport.state === "playing") {
    transport.state = "paused";
    try {
      // The audio fades out and stops after the frames already queued to the device; the roll
      // runs on to that position and holds there.
      roll.holdPlayback(await invoke<number>("playback_pause"));
    } catch (err) {
      // No playback any more (it had just ended).
      console.error(err);
      transport.state = "stopped";
      roll.stopPlayback();
    }
  } else {
    // Resume from the marker or the playhead: where the pause left it, unless the view was
    // scrolled or a marker clicked in the meantime.
    await playFrom(startPositionS());
  }
}

/** A click on the roll at audio time `t`: seeks while playing, otherwise places the marker the
 * next Play starts from (also while capture runs, where it only marks the time). */
async function seekTo(t: number): Promise<void> {
  if (roll && pipeline.status === "running") {
    setMarker(t);
    return;
  }
  // Not while capture is starting up or stopping (the roll still shows the previous recording
  // until `Started`), and not without audio.
  if (!roll || pipeline.status !== "idle" || !recording.hasAudio) return;
  const target = clampToRecording(t);
  if (transport.state === "playing") await playFrom(target);
  else setMarker(target);
}

/** Stops playback (if any), leaving the roll where it is. */
export async function stopPlayback(): Promise<void> {
  const wasPlaying = transport.state !== "stopped";
  transport.state = "stopped";
  gen++;
  setMarker(null);
  if (!wasPlaying) return;
  roll?.stopPlayback();
  try {
    await invoke("playback_stop");
  } catch (err) {
    console.error(err);
  }
}

/** The legend's part visibility: hidden parts' notes are not sounded either. */
export function setPlaybackPartVisible(part: NotePart, visible: boolean): void {
  if (visible) hiddenParts.delete(part);
  else hiddenParts.add(part);
  pushPlaybackParams();
}

/** Sends the volumes, waves and hidden parts to a playback in progress. */
export function pushPlaybackParams(): void {
  if (transport.state === "stopped") return;
  invoke("playback_set_params", { params: params() }).catch((err) => console.error(err));
}
