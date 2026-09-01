import { Application, Container, Graphics } from "pixi.js";

const MIN_PITCH = 21; // A0
const MAX_PITCH = 108; // C8
const PIXELS_PER_SECOND = 120;
const PLAYHEAD_FRACTION = 0.97;
// The pipeline's confirm delay is a known, fixed design parameter, not
// something to discover empirically: one analysis window (HOP_SIZE /
// AUDIO_SAMPLE_RATE = 36164/22050 ~= 1.64s) plus NoteStitcher's confirm
// margin (0.5s) ~= 2.14s, before a note or chord can be reported at all.
// Trying to *estimate* this from observed event timestamps (an earlier
// approach) was fragile -- NoteConfirmed's end_s is capped at
// (covered - confirm_margin), which is a little behind the freshest data
// a ChordChange event can report, so "take the smallest observed gap"
// didn't reliably converge to the true figure. Since we already know the
// figure, just use it directly, plus a small safety margin for IPC/
// processing overhead and jitter, rather than estimating it at runtime.
const PIPELINE_LATENCY_S = 2.14;
const LATENCY_SAFETY_MARGIN_S = 0.3;
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
 * The pipeline's confirm delay (one analysis window + NoteStitcher's
 * confirm margin) is a fixed length, not something that needs continuous
 * correction or runtime estimation -- three earlier approaches all treated
 * it as something to discover/track dynamically and all looked wrong:
 * snapping the displayed clock to each new event read as a visible "warp";
 * continuously adjusting its playback rate to chase the latest event read
 * as a visible, repeating acceleration; and estimating the delay from the
 * smallest observed (wall-clock-since-reset minus event timestamp) gap
 * left the same dead zone the rate-based version had, because
 * NoteConfirmed's end_s is itself capped at (covered - confirm_margin) --
 * a little behind the freshest data a ChordChange event reports -- so
 * "smallest observed gap" doesn't reliably converge on the true figure.
 * Since the delay is a known constant of the pipeline's own design
 * (PIPELINE_LATENCY_S), just use it directly plus a small safety margin,
 * and run the displayed clock at a constant, exact 1x rate offset by that
 * fixed figure -- never sped up, slowed down, or re-anchored again. */
export class PianoRoll {
  private app: Application;
  private noteLayer: Container;
  private notes: NoteSprite[] = [];
  private running = false;
  private resetAtMs = 0;

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

  /** Clears the view and starts the scroll clock. Call when a session starts. */
  reset(): void {
    this.running = true;
    this.resetAtMs = performance.now();
    for (const n of this.notes) n.graphic.destroy();
    this.notes = [];
  }

  private wallClockS(): number {
    return (performance.now() - this.resetAtMs) / 1000;
  }

  private displayS(): number {
    return this.wallClockS() - (PIPELINE_LATENCY_S + LATENCY_SAFETY_MARGIN_S);
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
