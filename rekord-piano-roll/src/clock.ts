// What time the piano roll is showing. All times are the pipeline's audio time: seconds since the
// session's `Started`, the same clock every note, chord and beat line is stamped with.
//
// Two times matter: the *view* (`displayS`), the time at the keyboard's left edge, which positions
// everything; and the *cursor* (`cursorS`), the time of the audio itself (what the readouts and the
// highlighted keys follow). Usually they are the same -- the roll scrolls under a cursor fixed at the
// keyboard -- but when playback starts or jumps, the view stays where it is and the cursor line
// travels right along the roll, as in a DAW, until it reaches the keyboard edge and the roll
// scrolls from there.
//
// - LiveClock: capture is running. A pure linear function of the wall clock, offset by a constant
//   fixed once per session (see the notes on `displayLagS` in CLAUDE.md).
// - HeldClock: stopped or paused. A fixed position the user can scroll.
// - PlaybackClock: replaying the recording. Follows the audio actually being heard, extrapolated
//   at exactly 1x from the backend's position reports.

export interface RollClock {
  readonly kind: "live" | "held" | "playing";
  /** The time at the keyboard's left edge (the view). */
  displayS(): number;
  /** The time of the audio: at or before `displayS()`, drawn as a line when it is before it. */
  cursorS(): number;
  /** The newest time the live roll got to show. Everything older than `headS() - HISTORY_S` is
   * dropped, and it stays put while stopped, so history pruning doesn't depend on where a
   * stopped or playing roll is looking. */
  headS(): number;
}

// The display lag is measured from the NoteStarted notes of the first LAG_MEASURE_S of a session
// (it locks at the first update with at least one sample, so silence just delays it), plus a small
// safety margin. Untuned.
const LAG_MEASURE_S = 4;
const LAG_MARGIN_S = 0.01;

export class LiveClock implements RollClock {
  readonly kind = "live";
  private resetAtMs = 0;
  private stoppedAtMs: number | null = null;
  private lagS = 0;
  private lagLocked = false;
  private lagMinS = Infinity;

  /** Starts the clock at 0 (call when the pipeline's `Started` arrives). */
  reset(): void {
    this.resetAtMs = performance.now();
    this.stoppedAtMs = null;
    this.lagS = 0;
    this.lagLocked = false;
    this.lagMinS = Infinity;
  }

  /** Freezes the clock where it is; `reset()` starts it again. */
  freeze(): void {
    if (this.stoppedAtMs === null) this.stoppedAtMs = performance.now();
  }

  /** Seconds since `reset()` (frozen by `freeze()`). */
  wallS(): number {
    return ((this.stoppedAtMs ?? performance.now()) - this.resetAtMs) / 1000;
  }

  displayS(): number {
    return this.wallS() - this.lagS;
  }

  cursorS(): number {
    return this.displayS();
  }

  headS(): number {
    return this.displayS();
  }

  /** A note was reported at its onset: the smallest onset-to-arrival delay seen so far lets the
   * freshest notes land exactly on the keyboard edge, later-detected ones a little to its left. */
  sampleOnset(startS: number): void {
    if (!this.lagLocked) this.lagMinS = Math.min(this.lagMinS, this.wallS() - startS);
  }

  /** Locks the display lag once enough has been measured. Call every frame. */
  update(): void {
    if (!this.lagLocked && this.wallS() >= LAG_MEASURE_S && Number.isFinite(this.lagMinS)) {
      this.lagS = this.lagMinS + LAG_MARGIN_S;
      this.lagLocked = true;
    }
  }
}

export class HeldClock implements RollClock {
  readonly kind = "held";
  private viewS: number;
  private cursor: number;

  /** `cursorS` defaults to the view: a paused playback keeps its cursor where it stopped. */
  constructor(
    private readonly head: number,
    viewS: number,
    cursorS = viewS,
  ) {
    this.viewS = viewS;
    this.cursor = cursorS;
  }

  displayS(): number {
    return this.viewS;
  }

  cursorS(): number {
    return this.cursor;
  }

  headS(): number {
    return this.head;
  }

  /** Scrolls: the cursor goes back to the keyboard edge. */
  setView(viewS: number): void {
    this.viewS = viewS;
    this.cursor = viewS;
  }
}

/** The wall clock as the backend reports it (unix epoch, ms): both sides read the same machine's
 * clock, which is what lets a position report be used without caring how long it took to arrive. */
export function epochNowMs(): number {
  return performance.timeOrigin + performance.now();
}

// A report this far from where the extrapolation says it should be, three times in a row, means the
// two clocks drifted (or the device latency changed): re-anchor. Ordinary drift is ~100 ppm, i.e.
// tens of ms over five minutes, so this normally never triggers and the roll never changes speed.
const RESYNC_S = 0.04;
const RESYNC_AFTER = 3;

export class PlaybackClock implements RollClock {
  readonly kind = "playing";
  private anchor: { posS: number; atEpochMs: number } | null = null;
  private offReports = 0;
  private holdS: number | null = null;

  /** `floorS` is where playback was asked to start: until the first report says the audio is
   * actually audible, the cursor waits there, and it never moves backwards. The view stays at
   * `viewFloorS` until the cursor catches up with it. */
  constructor(
    private readonly floorS: number,
    private readonly endS: number,
    private readonly head: number,
    private readonly viewFloorS: number,
  ) {}

  displayS(): number {
    return Math.max(this.viewFloorS, this.cursorS());
  }

  cursorS(): number {
    let pos = this.anchor ? this.anchor.posS + (epochNowMs() - this.anchor.atEpochMs) / 1000 : this.floorS;
    if (this.holdS !== null) pos = Math.min(pos, this.holdS);
    return Math.min(this.endS, Math.max(this.floorS, pos));
  }

  headS(): number {
    return this.head;
  }

  /** `posS` is audible at wall-clock `atEpochMs`. */
  onPosition(posS: number, atEpochMs: number): void {
    if (!this.anchor) {
      this.anchor = { posS, atEpochMs };
      return;
    }
    const predictedS = this.anchor.posS + (atEpochMs - this.anchor.atEpochMs) / 1000;
    if (Math.abs(predictedS - posS) > RESYNC_S) {
      if (++this.offReports >= RESYNC_AFTER) {
        this.anchor = { posS, atEpochMs };
        this.offReports = 0;
      }
    } else {
      this.offReports = 0;
    }
  }

  /** Playback paused with the audio stopping at `posS` (the buffered tail is still heard): keep
   * moving until the display gets there, then stay. */
  holdAt(posS: number): void {
    this.holdS = posS;
  }

  /** The display has reached the position given to `holdAt`. */
  holdReached(): boolean {
    return this.holdS !== null && this.cursorS() >= this.holdS;
  }
}
