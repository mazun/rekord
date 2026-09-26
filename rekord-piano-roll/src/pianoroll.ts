import { Application, Container, Graphics } from "pixi.js";

const MIN_PITCH = 21; // A0
const MAX_PITCH = 108; // C8
const PIXELS_PER_SECOND = 120;
// Notes are shaded between these two by velocity (the model's own
// per-pitch activation strength for that note, in [0, 1]) rather than
// drawn in one fixed color -- so within a chord, a note that sounded
// weaker actually reads as dimmer, not just as "a note" with no
// indication of how hard it was played relative to its neighbors.
const NOTE_COLOR_MIN = 0x2c3550; // quiet: closer to the guide/background grey-blue
const NOTE_COLOR_MAX = 0xaecbff; // loud: bright, near-white blue
// Melody/bass line labels (see rekord_core::melody_bass -- a skyline rule
// over the live note stream, no separation model) get their own color
// pairs, same velocity-shading treatment as the default. An inner voice
// (neither) keeps the original blue. The backend's melody/bass flags
// aren't mutually exclusive (a lone note low enough to be "lowest
// sounding" but still within melody's register can be both) -- melody
// wins that case here rather than a third blended color, since live
// testing (2026-09-23) found those ambiguous notes were disproportionately
// real melody with no bass actually sounding.
const MELODY_COLOR_MIN = 0x1f3d2c;
const MELODY_COLOR_MAX = 0x6be3a0;
const BASS_COLOR_MIN = 0x3d2c1f;
const BASS_COLOR_MAX = 0xff9f5b;
const GUIDE_COLOR = 0x262b36;

// Beat/bar lines (rekord_core::beat_tracker, via PipelineEvent::BeatLines) come in three
// layers of the same beats: predicted (extrapolated, no delay), provisional (the network's
// peaks, ~0.6s behind the audio) and committed (the DBN's final decision, ~4.5s behind). With
// the display running total_latency_s (~1.3s) behind the audio, predicted and provisional
// lines are already there when they reach the playhead, and a line turns committed a few
// seconds after passing it. Each layer is drawn a step stronger than the one before, and a
// bar line (downbeat) stronger and wider than a beat.
type BeatLayer = "predicted" | "provisional" | "committed";
const BEAT_LINE_COLOR = 0x5a6580;
const BAR_LINE_COLOR = 0x9aaad8;
const BEAT_LINE_ALPHA: Record<BeatLayer, number> = { predicted: 0.2, provisional: 0.4, committed: 0.65 };
const BAR_LINE_WIDTH = 2;
// A new snapshot's line within this of a line already drawn is the same beat moving to its
// next layer (its time can shift a little when it does); beats are >= 0.28s apart (215 BPM).
const BEAT_MATCH_TOLERANCE_S = 0.1;
// Time constant of every beat-line transition (layer, bar/beat, position, fade in/out).
const BEAT_ANIM_TAU_S = 0.15;

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

/** Linearly blends two 0xRRGGBB colors by `t` in [0, 1]. */
function lerpColor(from: number, to: number, t: number): number {
  const clamped = Math.min(1, Math.max(0, t));
  const channel = (shift: number) => {
    const a = (from >> shift) & 0xff;
    const b = (to >> shift) & 0xff;
    return Math.round(a + (b - a) * clamped);
  };
  return (channel(16) << 16) | (channel(8) << 8) | channel(0);
}

export interface ConfirmedNote {
  start_s: number;
  end_s: number;
  pitch_midi: number;
  velocity: number;
  melody: boolean;
  bass: boolean;
}

interface NoteSprite extends ConfirmedNote {
  graphic: Graphics;
}

export interface BeatLine {
  t: number;
  bar: boolean;
}

export interface BeatLines {
  from_s: number;
  committed: BeatLine[];
  provisional: BeatLine[];
  predicted: BeatLine[];
}

// Drawn as a 1x1 white rect scaled to (width, screen height) and tinted, so every animated
// property is a plain field update -- no geometry rebuild per frame.
interface BeatSprite {
  graphic: Graphics;
  t: number;
  targetT: number;
  alpha: number;
  targetAlpha: number;
  bar: number; // 0 = beat, 1 = bar; animated between for color and width
  targetBar: number;
  dying: boolean; // absent from the latest snapshot: fading out, then destroyed
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
  private beatLayer = new Container();
  private beatLines: BeatSprite[] = [];
  private guides = new Graphics();
  private keyboardBacking = new Graphics();
  private keys = new Map<number, KeySprite>();
  private notes: NoteSprite[] = [];
  private running = false;
  private resetAtMs = 0;
  private totalLatencyS = 0;
  // Whether newly-added notes get the melody/bass color treatment (see
  // addNote) or just the default coloring, as if every note were an inner
  // voice. Toggling only affects notes added from here on -- notes already
  // on screen keep whatever color they were drawn with, same as changing
  // the latency slider mid-session not affecting the running session.
  private roleColoringEnabled = true;

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
    app.stage.addChild(roll.beatLayer);
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

  // Tears down the renderer and removes its canvas -- only needed so a
  // Svelte HMR remount doesn't leave a second canvas/ticker running.
  destroy(): void {
    this.app.destroy(true, { children: true });
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
    for (const b of this.beatLines) b.graphic.destroy();
    this.beatLines = [];
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

  setRoleColoringEnabled(enabled: boolean): void {
    this.roleColoringEnabled = enabled;
  }

  addNote(note: ConfirmedNote): void {
    const height = this.app.screen.height;
    const noteHeightPx = Math.max(3, height / (MAX_PITCH - MIN_PITCH) - 1);
    const widthPx = Math.max(2, (note.end_s - note.start_s) * PIXELS_PER_SECOND);

    const [colorMin, colorMax] =
      this.roleColoringEnabled && note.melody
        ? [MELODY_COLOR_MIN, MELODY_COLOR_MAX]
        : this.roleColoringEnabled && note.bass
          ? [BASS_COLOR_MIN, BASS_COLOR_MAX]
          : [NOTE_COLOR_MIN, NOTE_COLOR_MAX];
    const color = lerpColor(colorMin, colorMax, note.velocity);
    const graphic = new Graphics();
    graphic.roundRect(0, 0, widthPx, noteHeightPx, 2).fill({ color });
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

  /** Applies a BeatLines snapshot: it replaces every line at or after `from_s`. A line
   * matching one already drawn animates to its new layer/time; the rest fade in or out. */
  setBeatLines(lines: BeatLines): void {
    const incoming: [BeatLine, BeatLayer][] = [
      ...lines.committed.map((l): [BeatLine, BeatLayer] => [l, "committed"]),
      ...lines.provisional.map((l): [BeatLine, BeatLayer] => [l, "provisional"]),
      ...lines.predicted.map((l): [BeatLine, BeatLayer] => [l, "predicted"]),
    ];
    const open = this.beatLines.filter((b) => !b.dying && b.targetT >= lines.from_s - BEAT_MATCH_TOLERANCE_S);
    const matched = new Set<BeatSprite>();
    for (const [line, layer] of incoming) {
      let best: BeatSprite | null = null;
      for (const b of open) {
        const d = Math.abs(b.targetT - line.t);
        if (!matched.has(b) && d <= BEAT_MATCH_TOLERANCE_S && (!best || d < Math.abs(best.targetT - line.t))) {
          best = b;
        }
      }
      if (!best) {
        const graphic = new Graphics().rect(0, 0, 1, 1).fill({ color: 0xffffff });
        graphic.alpha = 0;
        this.beatLayer.addChild(graphic);
        best = { graphic, t: line.t, targetT: line.t, alpha: 0, targetAlpha: 0, bar: +line.bar, targetBar: 0, dying: false };
        this.beatLines.push(best);
      }
      matched.add(best);
      best.targetT = line.t;
      best.targetAlpha = BEAT_LINE_ALPHA[layer];
      best.targetBar = +line.bar;
    }
    for (const b of open) {
      if (!matched.has(b) && b.targetT >= lines.from_s) {
        b.dying = true;
        b.targetAlpha = 0;
      }
    }
  }

  private tickBeatLines(dtS: number): void {
    const k = 1 - Math.exp(-dtS / BEAT_ANIM_TAU_S);
    const height = this.app.screen.height;
    const kept: BeatSprite[] = [];
    for (const b of this.beatLines) {
      b.t += (b.targetT - b.t) * k;
      b.alpha += (b.targetAlpha - b.alpha) * k;
      b.bar += (b.targetBar - b.bar) * k;
      const width = 1 + (BAR_LINE_WIDTH - 1) * b.bar;
      const x = this.xForTime(b.t) - width / 2;
      if ((b.dying && b.alpha < 0.01) || x + width < 0) {
        b.graphic.destroy();
        continue;
      }
      b.graphic.x = x;
      b.graphic.scale.set(width, height);
      b.graphic.alpha = b.alpha * (1 + b.bar * 0.4);
      b.graphic.tint = lerpColor(BEAT_LINE_COLOR, BAR_LINE_COLOR, b.bar);
      kept.push(b);
    }
    this.beatLines = kept;
  }

  private onTick(): void {
    if (!this.running) return;
    this.tickBeatLines(this.app.ticker.deltaMS / 1000);
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
