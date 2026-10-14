import { BpmTracker } from "./bpm";
import {
  ChordLane,
  type ChordLayerEvent,
  type ChordLayerId,
  LAYER_ORDER,
  type LayerChord,
} from "./chordlane";
import type { BeatLines, ConfirmedNote, DrumHitsEvent, NotePart, StartedNote } from "./pianoroll";
import { type ChordVocabulary, tensionsFor } from "./tensions";

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

  /** The entries from `fromS` to `toS`, plus the one in effect at `fromS`. */
  between(fromS: number, toS: number): { t: number; v: T }[] {
    let first = 0;
    while (first + 1 < this.entries.length && this.entries[first + 1].t <= fromS) first++;
    return this.entries.slice(first).filter((e) => e.t <= toS);
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
  | { type: "KeyChange"; t: number; key_name: string; tonic_pc: number }
  | ({ type: "BeatLines" } & BeatLines)
  // Drawn by the roll only (not replayed or exported).
  | ({ type: "DrumHits" } & DrumHitsEvent);

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
  /** The relative-major pitch class of the key. */
  keyPc: number | null;
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

// The context a debug export keeps around the selected range: the chord layers read back before it
// (the 2 s layer's Viterbi starts cold otherwise) and decide late after it.
const EXPORT_PAD_BEFORE_S = 4;
const EXPORT_PAD_AFTER_S = 3;

/** What `Recording.exportRange` gathers (sent to the backend's `debug_export_range`). */
export interface RangeExport {
  window_start_s: number;
  window_end_s: number;
  selection_start_s: number;
  selection_end_s: number;
  notes: SynthNote[];
  chord_layers: Record<ChordLayerId, { decided_s: number; changes: LayerChord[] }>;
  lane: { start_s: number; end_s: number | null; label: string; layer: ChordLayerId }[];
  keys: { t: number; key_name: string }[];
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
  private keys = new Timeline<{ name: string; pc: number }>();
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
        this.keys.append(event.t, { name: event.key_name, pc: event.tonic_pc });
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

  /** The tempo at audio time `t` (Infinity: the latest), the one readout the beat lines move. */
  bpmAt(t: number): number | null {
    return this.bpms.at(t) ?? null;
  }

  /** The readouts as they were at audio time `t` (Infinity: the latest). */
  stateAt(t: number): DisplayState {
    const chord = this.lane.at(t)?.chord;
    const tensions =
      chord && this.vocabulary && chord.root_pc !== null && chord.quality !== null
        ? tensionsFor(this.vocabulary, this.actives.at(t) ?? 0, chord.root_pc, chord.quality)
        : [];
    const key = this.keys.at(t);
    return {
      chord: chord?.label ?? null,
      roman: chord?.roman_numeral ?? null,
      keyName: key?.name ?? null,
      keyPc: key?.pc ?? null,
      tensions,
      bpm: this.bpmAt(t),
    };
  }

  /** The notes to sound, as the backend's synth takes them: all of them (which parts are heard
   * is the backend's setting, so it can change during playback). */
  synthNotes(): SynthNote[] {
    const out: SynthNote[] = [];
    for (const n of this.notes) {
      const end = n.end_s ?? this.endS;
      if (end < this.startS) continue; // older than the audio that was kept
      out.push({
        start_s: n.start_s,
        end_s: end,
        pitch: n.pitch_midi,
        velocity: n.velocity,
        part: n.part,
      });
    }
    return out;
  }

  /** The notes of the selected range for "copy as MIDI": clipped to it, times relative to its
   * start, with the beat tracker's tempo there (the backend falls back to 120 without one). Parts
   * hidden on the roll are left out. */
  midiRange(selection: { startS: number; endS: number }, isPartShown: (part: NotePart) => boolean) {
    const notes = this.synthNotes()
      .filter(
        (n) => n.end_s > selection.startS && n.start_s < selection.endS && isPartShown(n.part),
      )
      .map((n) => ({
        ...n,
        start_s: Math.max(n.start_s, selection.startS) - selection.startS,
        end_s: Math.min(n.end_s, selection.endS) - selection.startS,
      }));
    return { notes, bpm: this.bpms.at(selection.startS) ?? null };
  }

  /** What a debug export of the selected range needs besides the audio: the notes, what each chord
   * layer reported and the lane showed, and the keys, over the range widened by the context the
   * chord layers need to reproduce it (the 2 s layer decides late, and reads back before the range). */
  exportRange(selection: { startS: number; endS: number }): RangeExport {
    const windowStartS = Math.max(this.startS, selection.startS - EXPORT_PAD_BEFORE_S);
    const windowEndS = Math.min(this.endS, selection.endS + EXPORT_PAD_AFTER_S);
    return {
      window_start_s: windowStartS,
      window_end_s: windowEndS,
      selection_start_s: selection.startS,
      selection_end_s: selection.endS,
      notes: this.synthNotes().filter((n) => n.end_s > windowStartS && n.start_s < windowEndS),
      chord_layers: this.lane.layerLogs(windowStartS, windowEndS),
      lane: this.lane
        .segments()
        .filter((s) => s.startS < windowEndS && (s.endS ?? Infinity) > windowStartS)
        .map((s) => ({
          start_s: s.startS,
          end_s: s.endS,
          label: s.chord.label,
          layer: LAYER_ORDER[s.layer],
        })),
      keys: this.keys
        .between(windowStartS, windowEndS)
        .map((e) => ({ t: e.t, key_name: e.v.name })),
    };
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
