import type { BeatLines } from "./pianoroll";

// How much of the beat history the tempo is read from: long enough that the beats' 20 ms
// frame quantization averages out (~0.3 BPM at 120 BPM), short enough to follow a new song
// within a few bars.
const BPM_WINDOW_S = 10;
// The committed lines run this far behind the audio (beat_tracker.rs); the window ends there.
const COMMITTED_DELAY_S = 4.5;
// How far a BeatLines snapshot reaches back from the tracker's present: BEAT_LINES_WINDOW_S in
// pipeline.rs.
const BEAT_LINES_WINDOW_S = 30;
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
  const end = lines.from_s + BEAT_LINES_WINDOW_S - COMMITTED_DELAY_S;
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

/** The tempo readout, with its hold and hysteresis, as a function of the BeatLines snapshots
 * seen so far. `observe` returns a value only when the readout changes. */
export class BpmTracker {
  private seenAt = -Infinity;
  private candidate: number | null = null;
  private shown: number | null = null;

  reset(): void {
    this.seenAt = -Infinity;
    this.candidate = null;
    this.shown = null;
  }

  /** `t` is the audio time the new readout describes (the end of the beats it was measured
   * from), so a replay shows it where the tempo was actually heard, not 4.5 s late. */
  observe(lines: BeatLines): { t: number; bpm: number | null } | null {
    const now = lines.from_s;
    const t = lines.from_s + BEAT_LINES_WINDOW_S - COMMITTED_DELAY_S;
    const bpm = estimateBpm(lines);
    if (bpm === null) {
      if (now - this.seenAt > BPM_HOLD_S && this.shown !== null) {
        this.shown = null;
        return { t, bpm: null };
      }
      return null;
    }
    this.seenAt = now;
    const shown = this.shown;
    const agrees = (a: number | null) => a !== null && Math.abs(bpm - a) <= BPM_JUMP * a;
    if (agrees(shown) || agrees(this.candidate)) {
      this.candidate = null;
      // Hysteresis: a tempo hovering around x.5 would otherwise flicker between two values.
      if (shown === null || Math.abs(bpm - shown) >= 0.8) {
        this.shown = Math.round(bpm);
        if (this.shown !== shown) return { t, bpm: this.shown };
      }
    } else {
      this.candidate = bpm;
    }
    return null;
  }
}
