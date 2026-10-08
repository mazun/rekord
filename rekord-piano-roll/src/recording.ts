import { BpmTracker } from "./bpm";
import { ChordLane, type ChordLayerEvent } from "./chordlane";
import type { BeatLines, ConfirmedNote, NotePart, StartedNote } from "./pianoroll";
import { tensionsFor, type ChordVocabulary } from "./tensions";

// How much of the past is kept (notes, chords, beat lines, and on the backend the audio itself:
// RECORDING_HISTORY_S in audio_ring.rs) so a stopped roll can be scrolled back through it and
// played; anything that ended longer ago than this is dropped. Untuned.
export const HISTORY_S = 300;

/** A time-ordered log of values: what was current at time `t`. */
export class Timeline<T> {
  private entries: { t: number; v: T }[] = [];

  /** Appends `v` from time `t` on. Out-of-order times (a chord snapped back to a beat) are
   * clamped to the previous entry's, so the log stays sorted. */
  append(t: number, v: T): void {
    const last = this.entries[this.entries.length - 1];
    this.entries.push({ t: last ? Math.max(t, last.t) : t, v });
  }

  /** The value in effect at `t`, or undefined before the first entry. */
  at(t: number): T | undefined {
    let lo = 0;
    let hi = this.entries.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.entries[mid].t <= t) lo = mid + 1;
      else hi = mid;
    }
    return lo > 0 ? this.entries[lo - 1].v : undefined;
  }

  last(): T | undefined {
    return this.entries[this.entries.length - 1]?.v;
  }

  replaceLast(v: T): void {
    const last = this.entries[this.entries.length - 1];
    if (last) last.v = v;
  }

  /** Drops what happened before `t`, keeping the one entry that is still in effect at `t`. */
  pruneBefore(t: number): void {
    let keepFrom = 0;
    while (keepFrom + 1 < this.entries.length && this.entries[keepFrom + 1].t < t) keepFrom++;
    if (keepFrom > 0) this.entries.splice(0, keepFrom);
  }

  clear(): void {
    this.entries = [];
  }
}

/** The pipeline events a recording is made of (see `PipelineEvent` in pipeline.svelte.ts). */
export type RecordableEvent =
  | { type: "Started"; chord_vocabulary: ChordVocabulary }
  | ({ type: "NoteStarted" } & StartedNote)
  | ({ type: "NoteConfirmed" } & ConfirmedNote)
  | ({ type: "ChordLayer" } & ChordLayerEvent)
  | { type: "ActiveNotes"; changes: { t: number; pcs: number }[] }
  | { type: "KeyChange"; t: number; key_name: string }
  | ({ type: "BeatLines" } & BeatLines);

/** What the span of the audio recorded on the backend covers (`RecordingInfo` in audio_ring.rs). */
export interface RecordingInfo {
  sample_rate: number;
  start_s: number;
  end_s: number;
}

/** The chord/key/tension/tempo readouts at one moment; null where nothing had been reported yet. */
export interface DisplayState {
  chord: string | null;
  roman: string | null;
  keyName: string | null;
  tensions: string[];
  bpm: number | null;
}

/** A note as the synth plays it (`SynthNote` in playback/synth.rs). */
export interface SynthNote {
  start_s: number;
  end_s: number;
  pitch: number;
  velocity: number;
  part: NotePart;
}

interface RecordedNote {
  start_s: number;
  /** null while the note is still sounding as far as the pipeline knows. */
  end_s: number | null;
  pitch_midi: number;
  velocity: number;
  part: NotePart;
}

// A recording shorter than this isn't offered for playback.
const MIN_PLAYABLE_S = 0.2;

// Pruning the notes costs a pass over them; do it every so many events, not every one.
const PRUNE_EVERY = 256;

/** Everything a session reported, with times, so it can be replayed: the notes (what the
 * piano roll draws and the synth plays) and the chord/key/tension/tempo readouts at any time. The
 * chords are the lane composed from the chord layers (`ChordLane`); the readout shows its newest
 * segment live and the one at the playhead when replaying.
 * The one reducer for all of it is `apply`; the live display and the replay read the same
 * logs through `stateAt`. (The beat lines and the drawn notes themselves stay in the roll.) */
export class Recording {
  readonly lane = new ChordLane();
  private keys = new Timeline<string>();
  /** The pitch classes sounding (bit `pc` set), from the backend's `ActiveNotes`. */
  private actives = new Timeline<number>();
  /** The chord table the tensions are computed from, sent on `Started`. */
  private vocabulary: ChordVocabulary | null = null;
  private bpms = new Timeline<number | null>();
  private bpmTracker = new BpmTracker();
  private notes: RecordedNote[] = [];
  private openNotes = new Map<string, RecordedNote>();
  private newestS = 0;
  private sincePrune = 0;
  /** The span of recorded audio (known once capture stops), in the same audio seconds. */
  startS = 0;
  endS = 0;
  hasAudio = false;

  apply(event: RecordableEvent): void {
    switch (event.type) {
      case "Started":
        this.clear();
        this.vocabulary = event.chord_vocabulary;
        break;
      case "NoteStarted": {
        const note: RecordedNote = { ...event, end_s: null };
        this.notes.push(note);
        this.openNotes.set(noteKey(event), note);
        this.see(event.start_s);
        break;
      }
      case "NoteConfirmed": {
        const open = this.openNotes.get(noteKey(event));
        if (open) {
          open.end_s = event.end_s;
          this.openNotes.delete(noteKey(event));
        } else {
          // Ended before it was ever reported as started (shorter than one block).
          this.notes.push({ ...event });
        }
        this.see(event.end_s);
        break;
      }
      case "ChordLayer":
        this.lane.apply(event);
        this.see(event.decided_s);
        break;
      case "ActiveNotes":
        for (const c of event.changes) this.actives.append(c.t, c.pcs);
        this.see(event.changes[event.changes.length - 1]?.t ?? 0);
        break;
      case "KeyChange":
        this.keys.append(event.t, event.key_name);
        this.see(event.t);
        break;
      case "BeatLines": {
        const change = this.bpmTracker.observe(event);
        if (change) this.bpms.append(change.t, change.bpm);
        break;
      }
    }
  }

  /** Capture stopped; `info` says what audio the backend kept (null if none). Notes still open
   * are closed at its end (a late NoteConfirmed from the final flush still corrects them). */
  seal(info: RecordingInfo | null): void {
    this.hasAudio = !!info && info.end_s - info.start_s > MIN_PLAYABLE_S;
    this.startS = info?.start_s ?? 0;
    this.endS = info?.end_s ?? 0;
    for (const note of this.openNotes.values()) note.end_s = this.endS;
  }

  private clear(): void {
    this.lane.clear();
    this.keys.clear();
    this.actives.clear();
    this.bpms.clear();
    this.bpmTracker.reset();
    this.notes = [];
    this.openNotes.clear();
    this.newestS = 0;
    this.sincePrune = 0;
    this.startS = 0;
    this.endS = 0;
    this.hasAudio = false;
  }

  /** The readouts as they were at audio time `t` (Infinity: the latest). */
  stateAt(t: number): DisplayState {
    const chord = this.lane.at(t)?.chord;
    const tensions =
      chord && this.vocabulary && chord.root_pc !== null && chord.quality !== null
        ? tensionsFor(this.vocabulary, this.actives.at(t) ?? 0, chord.root_pc, chord.quality)
        : [];
    return {
      chord: chord?.label ?? null,
      roman: chord?.roman_numeral ?? null,
      keyName: this.keys.at(t) ?? null,
      tensions,
      bpm: this.bpms.at(t) ?? null,
    };
  }

  /** The notes to sound, as the backend's synth takes them: all of them (which parts are heard
   * is the backend's setting, so it can change during playback). */
  synthNotes(): SynthNote[] {
    const out: SynthNote[] = [];
    for (const n of this.notes) {
      const end = n.end_s ?? this.endS;
      if (end < this.startS) continue; // older than the audio that was kept
      out.push({ start_s: n.start_s, end_s: end, pitch: n.pitch_midi, velocity: n.velocity, part: n.part });
    }
    return out;
  }

  private see(t: number): void {
    this.newestS = Math.max(this.newestS, t);
    if (++this.sincePrune < PRUNE_EVERY) return;
    this.sincePrune = 0;
    const cut = this.newestS - HISTORY_S;
    this.notes = this.notes.filter((n) => n.end_s === null || n.end_s >= cut);
    this.lane.pruneBefore(cut);
    this.keys.pruneBefore(cut);
    this.actives.pruneBefore(cut);
    this.bpms.pruneBefore(cut);
  }
}

function noteKey(n: { part: NotePart; pitch_midi: number; start_s: number }): string {
  return `${n.part}:${n.pitch_midi}:${n.start_s}`;
}

export const recording = new Recording();
