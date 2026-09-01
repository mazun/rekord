import { Application, Container, Graphics } from "pixi.js";

const MIN_PITCH = 21; // A0
const MAX_PITCH = 108; // C8
const PIXELS_PER_SECOND = 120;
const NOTE_COLOR = 0x5b8cff;
const GUIDE_COLOR = 0x262b36;

// Vertical piano keyboard drawn at the playhead (the "now" line notes
// scroll into). Its left edge doubles as the playhead x, replacing the
// old fixed-fraction-of-width playhead.
const KEYBOARD_WIDTH = 34;
const BLACK_KEY_WIDTH = KEYBOARD_WIDTH * 0.6;
const WHITE_KEY_COLOR = 0xe8e8ec;
const WHITE_KEY_ACTIVE_COLOR = 0x5b8cff;
const BLACK_KEY_COLOR = 0x1a1c22;
const BLACK_KEY_ACTIVE_COLOR = 0x86a6ff;
const KEY_BORDER_COLOR = 0x0c0d12;
// Backing plate behind the whole keyboard strip. Black keys are drawn
// shorter than white keys, but the canvas background (#0c0d12) and the
// black key fill (0x1a1c22) are both near-black -- without this, the
// "shorter" portion has nothing lighter to show through, so it read as
// full-length. The backing gives that gap a visible, slightly lighter
// color, same as a real keyboard's frame between/behind the keys.
const KEYBOARD_BACKING_COLOR = 0x2a2e3a;

function isBlackKey(pitch: number): boolean {
  return [1, 3, 6, 8, 10].includes(pitch % 12);
}

export interface ConfirmedNote {
  start_s: number;
  end_s: number;
  pitch_midi: number;
}

interface NoteSprite extends ConfirmedNote {
  graphic: Graphics;
}

interface KeySprite {
  graphic: Graphics;
  isBlack: boolean;
  active: boolean;
}

/** Falling-note piano roll driven by confirmed note events. Notes arrive
 * already fully formed (start_s and end_s both known -- the pipeline only
 * reports a note once it's finished, in ~1.6s batches), so this doesn't
 * grow notes in real time; it just positions each one along the time axis.
 *
 * The pipeline's confirm delay (window-advance stride + capture hop +
 * NoteStitcher's confirm margin) is a fixed length for a given session, not
 * something that needs continuous correction or runtime estimation --
 * three earlier approaches all treated it as something to discover/track
 * dynamically and all looked wrong: snapping the displayed clock to each
 * new event read as a visible "warp"; continuously adjusting its playback
 * rate to chase the latest event read as a visible, repeating acceleration;
 * and estimating the delay from the smallest observed (wall-clock-since-
 * reset minus event timestamp) gap left the same dead zone the rate-based
 * version had, because NoteConfirmed's end_s is itself capped at
 * (covered - confirm_margin) -- a little behind the freshest data a
 * ChordChange event reports -- so "smallest observed gap" doesn't reliably
 * converge on the true figure. The backend computes this figure itself
 * (it's the one side that actually knows the stride/hop it settled on) and
 * sends it as `total_latency_s` on the pipeline's `Started` event; this
 * class just uses it directly and runs the displayed clock at a constant,
 * exact 1x rate offset by that fixed figure -- never sped up, slowed down,
 * or re-anchored again. */
export class PianoRoll {
  private app: Application;
  private noteLayer: Container;
  private guides = new Graphics();
  private keyboardBacking = new Graphics();
  private keys = new Map<number, KeySprite>();
  private notes: NoteSprite[] = [];
  private running = false;
  private resetAtMs = 0;
  private totalLatencyS = 0;

  private constructor(app: Application, noteLayer: Container) {
    this.app = app;
    this.noteLayer = noteLayer;
  }

  static async create(container: HTMLElement): Promise<PianoRoll> {
    const app = new Application();
    await app.init({
      background: "#0c0d12",
      resizeTo: container,
      antialias: true,
    });
    container.appendChild(app.canvas);

    const noteLayer = new Container();
    const roll = new PianoRoll(app, noteLayer);
    const keyboardLayer = new Container();
    app.stage.addChild(roll.guides);
    app.stage.addChild(noteLayer);
    app.stage.addChild(keyboardLayer);
    keyboardLayer.addChild(roll.keyboardBacking);
    roll.createKeyboard(keyboardLayer);

    // `resizeTo` (above) reads container.clientWidth/Height synchronously
    // during app.init(), but this runs inside the DOMContentLoaded handler
    // before the page's own stylesheet has necessarily applied layout --
    // clientWidth/Height can still be 0 there, leaving the renderer at
    // Pixi's 800x600 default. app.screen is read live (not snapshotted) by
    // every per-tick note/pitch calculation, so notes self-correct once
    // real dimensions land, but the one-time guide/keyboard layout doesn't
    // -- it needs to be redone once true layout is known, and again on any
    // later real window resize (the renderer emits "resize" for both).
    app.renderer.on("resize", () => roll.layout());
    requestAnimationFrame(() => app.resize());

    app.ticker.add(() => roll.onTick());
    return roll;
  }

  private playheadX(): number {
    return this.app.screen.width - KEYBOARD_WIDTH;
  }

  /** Re-lays-out the guides and keyboard for the current app.screen size.
   * Safe to call repeatedly (e.g. on every resize). */
  private layout(): void {
    const { height } = this.app.screen;
    const noteAreaWidth = this.playheadX();

    this.guides.clear();
    for (let pitch = MIN_PITCH; pitch <= MAX_PITCH; pitch += 1) {
      if (pitch % 12 !== 0) continue; // one guide line per C
      const y = this.pitchToY(pitch, height);
      this.guides.moveTo(0, y).lineTo(noteAreaWidth, y);
    }
    this.guides.stroke({ width: 1, color: GUIDE_COLOR });

    this.keyboardBacking.clear().rect(0, 0, KEYBOARD_WIDTH, height).fill({ color: KEYBOARD_BACKING_COLOR });
    this.keyboardBacking.x = noteAreaWidth;
    this.keyboardBacking.y = 0;

    const rowHeight = height / (MAX_PITCH - MIN_PITCH);
    for (const [pitch, key] of this.keys) {
      key.graphic.x = noteAreaWidth;
      key.graphic.y = this.pitchToY(pitch, height) - rowHeight / 2;
      this.redrawKey(key, rowHeight);
    }
  }

  /** Creates the vertical white/black-key strip's Graphics, one per MIDI
   * pitch so each lines up with the same pitchToY row notes and guides
   * use. White keys are drawn full-strip-width, black keys narrower and
   * flush to the note-area side -- mirroring how a real keyboard's black
   * keys are shorter and set back, just rotated 90° to match this roll's
   * horizontal time axis. Actual sizing/position happens in layout(). */
  private createKeyboard(keyboardLayer: Container): void {
    for (let pitch = MIN_PITCH; pitch <= MAX_PITCH; pitch++) {
      const graphic = new Graphics();
      keyboardLayer.addChild(graphic);
      this.keys.set(pitch, { graphic, isBlack: isBlackKey(pitch), active: false });
    }
  }

  private redrawKey(key: KeySprite, rowHeight: number): void {
    const width = key.isBlack ? BLACK_KEY_WIDTH : KEYBOARD_WIDTH;
    const color = key.active
      ? key.isBlack
        ? BLACK_KEY_ACTIVE_COLOR
        : WHITE_KEY_ACTIVE_COLOR
      : key.isBlack
        ? BLACK_KEY_COLOR
        : WHITE_KEY_COLOR;
    key.graphic.clear().rect(0, 0, width, rowHeight).fill({ color });
    if (!key.isBlack) key.graphic.stroke({ width: 1, color: KEY_BORDER_COLOR });
  }

  private setKeyActive(pitch: number, active: boolean): void {
    const key = this.keys.get(pitch);
    if (!key || key.active === active) return;
    key.active = active;
    const rowHeight = this.app.screen.height / (MAX_PITCH - MIN_PITCH);
    this.redrawKey(key, rowHeight);
  }

  private pitchToY(pitch: number, height: number): number {
    const t = (pitch - MIN_PITCH) / (MAX_PITCH - MIN_PITCH);
    return height - t * height;
  }

  /** Clears the view and starts the scroll clock. Call when a session
   * starts, with the backend-computed confirm-delay figure for that
   * session (from the pipeline's `Started { total_latency_s }` event). */
  reset(totalLatencyS: number): void {
    this.running = true;
    this.resetAtMs = performance.now();
    this.totalLatencyS = totalLatencyS;
    for (const n of this.notes) n.graphic.destroy();
    this.notes = [];
    for (const pitch of this.keys.keys()) this.setKeyActive(pitch, false);
  }

  private wallClockS(): number {
    return (performance.now() - this.resetAtMs) / 1000;
  }

  private displayS(): number {
    return this.wallClockS() - this.totalLatencyS;
  }

  private xForTime(t: number): number {
    return this.playheadX() + (t - this.displayS()) * PIXELS_PER_SECOND;
  }

  addNote(note: ConfirmedNote): void {
    const height = this.app.screen.height;
    const noteHeightPx = Math.max(3, height / (MAX_PITCH - MIN_PITCH) - 1);
    const widthPx = Math.max(2, (note.end_s - note.start_s) * PIXELS_PER_SECOND);

    const graphic = new Graphics();
    graphic.roundRect(0, 0, widthPx, noteHeightPx, 2).fill({ color: NOTE_COLOR });
    // Position it correctly right away -- left at the Graphics default
    // (x=0, the far-left edge where the oldest, about-to-be-pruned notes
    // sit) until the next tick, a newly-added note would flash there for
    // a frame, reading as "a new note suddenly appearing where one
    // already is."
    graphic.x = this.xForTime(note.start_s);
    graphic.y = this.pitchToY(note.pitch_midi, height) - noteHeightPx / 2;
    this.noteLayer.addChild(graphic);
    this.notes.push({ ...note, graphic });
  }

  private onTick(): void {
    if (!this.running) return;
    const displayS = this.displayS();
    const activePitches = new Set<number>();
    const stillVisible: NoteSprite[] = [];
    for (const n of this.notes) {
      const x = this.xForTime(n.start_s);
      n.graphic.x = x;
      if (x + n.graphic.width < 0) {
        // Scrolled fully off the left edge -- drop it so the note list
        // (and the number of display objects) doesn't grow forever over
        // a long session.
        n.graphic.destroy();
      } else {
        stillVisible.push(n);
        if (displayS >= n.start_s && displayS <= n.end_s) activePitches.add(n.pitch_midi);
      }
    }
    this.notes = stillVisible;
    for (const pitch of this.keys.keys()) {
      this.setKeyActive(pitch, activePitches.has(pitch));
    }
  }
}
