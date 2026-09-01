import { Application, Container, Graphics } from "pixi.js";

const MIN_PITCH = 21; // A0
const MAX_PITCH = 108; // C8
const PIXELS_PER_SECOND = 120;
const PLAYHEAD_FRACTION = 0.97;
const NOTE_COLOR = 0x5b8cff;
const GUIDE_COLOR = 0x262b36;

export interface ConfirmedNote {
  start_s: number;
  end_s: number;
  pitch_midi: number;
}

interface NoteSprite extends ConfirmedNote {
  graphic: Graphics;
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
    roll.drawKeyGuides();
    app.stage.addChild(noteLayer);
    app.ticker.add(() => roll.onTick());
    return roll;
  }

  private drawKeyGuides(): void {
    const guides = new Graphics();
    const { width, height } = this.app.screen;
    for (let pitch = MIN_PITCH; pitch <= MAX_PITCH; pitch += 1) {
      if (pitch % 12 !== 0) continue; // one guide line per C
      const y = this.pitchToY(pitch, height);
      guides.moveTo(0, y).lineTo(width, y);
    }
    guides.stroke({ width: 1, color: GUIDE_COLOR });
    this.app.stage.addChildAt(guides, 0);
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
  }

  private wallClockS(): number {
    return (performance.now() - this.resetAtMs) / 1000;
  }

  private displayS(): number {
    return this.wallClockS() - this.totalLatencyS;
  }

  private xForTime(t: number): number {
    return this.app.screen.width * PLAYHEAD_FRACTION + (t - this.displayS()) * PIXELS_PER_SECOND;
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
      }
    }
    this.notes = stillVisible;
  }
}
