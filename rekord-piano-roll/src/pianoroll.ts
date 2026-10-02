import { Application, Container, Graphics, Text } from "pixi.js";

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
// The Mix model's own parts reuse the same three palettes: vocal is drawn in the melody
// green, bass in the bass orange, other in the default blue (see noteColors).
//
// Melody/bass line labels (see rekord_core::melody_bass -- a skyline rule
// over the live note stream, no separation model; only used for single-part
// models, whose notes have part "all") get their own color
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
// the display running total_latency_s (~0.9s at the default 0.26s look-ahead, ~1.1s at the
// 0.51s look-ahead) behind the audio, predicted lines are still drawn before they reach the
// playhead, but provisional ones mostly firm up just after passing it, and a line turns
// committed a few seconds after that. Each layer is drawn a step stronger than the one before,
// and a bar line (downbeat) stronger and wider than a beat.
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

// Chord history lane (rekord_core::pipeline::ChordStabilizer, via PipelineEvent::ChordChange):
// a strip along the top showing each stabilized chord as it scrolls past, like the notes below
// it -- a chord's rectangle starts at its ChordChange time and, unlike a note, has no known end
// until the *next* ChordChange arrives, so the still-current chord's right edge is redrawn at
// the playhead every tick instead of a fixed value. Bands alternate tint solely so consecutive
// chords read as distinct spans; it carries no other meaning. The roman numeral is repeated here
// (already shown big in ChordDisplay.svelte) because the point of putting chords on the roll at
// all is seeing the *progression* scroll past, and the numeral is the part of that worth reading
// after the chord itself has scrolled away from the always-current ChordDisplay.
const CHORD_LANE_HEIGHT = 34;
const CHORD_LABEL_FONT_SIZE = 13;
const CHORD_ROMAN_FONT_SIZE = 10;
const CHORD_LABEL_PADDING = 6;
const CHORD_BAND_COLOR_A = 0x1c2136;
const CHORD_BAND_COLOR_B = 0x242b47;
const CHORD_BOUNDARY_COLOR = 0x5a6580;
const CHORD_TEXT_COLOR = 0xe8e8ec; // matches --text
const CHORD_ROMAN_COLOR = 0x8b93a7; // matches --text-dim
const CHORD_SILENCE_COLOR = 0x555f78;
// Mirrors ChordAt::Silence's label() in pipeline.rs.
const SILENCE_LABEL = "(silence)";

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

// Which output part of the note model a note came from (rekord_core's Part): "all" for the
// single-part models, vocal/bass/other for the Mix model.
export type NotePart = "all" | "vocal" | "bass" | "other";

export interface ConfirmedNote {
  start_s: number;
  end_s: number;
  pitch_midi: number;
  velocity: number;
  part: NotePart;
  melody: boolean;
  bass: boolean;
}

/** A note whose onset is known but whose end isn't yet (`NoteStarted`). */
export interface StartedNote {
  start_s: number;
  pitch_midi: number;
  velocity: number;
  part: NotePart;
}

interface NoteSprite extends ConfirmedNote {
  graphic: Graphics;
  /** Still sounding as far as the pipeline knows: end_s tracks the clock until the
   * matching NoteConfirmed arrives. */
  open: boolean;
}

// An open note is drawn up to the newest audio the pipeline has seen, which is about the
// capture hop plus compute behind the wall clock (the rest of total_latency_s is the
// structural wait for a note's end, which an open note doesn't have).
const OPEN_NOTE_LAG_S = 0.15;

// The display lag is measured from the NoteStarted notes of the first LAG_MEASURE_S of a session
// (it locks at the first tick after that with at least one sample, so silence just delays it),
// plus a small safety margin. Untuned.
const LAG_MEASURE_S = 4;
const LAG_MARGIN_S = 0.01;

// Bottom to top: when notes of different parts overlap at the same pitch, the vocal is drawn
// over the bass over the rest.
const PART_DRAW_ORDER: NotePart[] = ["all", "other", "bass", "vocal"];

function noteColors(note: ConfirmedNote, roleColoringEnabled: boolean): [number, number] {
  switch (note.part) {
    case "vocal":
      return [MELODY_COLOR_MIN, MELODY_COLOR_MAX];
    case "bass":
      return [BASS_COLOR_MIN, BASS_COLOR_MAX];
    case "other":
      return [NOTE_COLOR_MIN, NOTE_COLOR_MAX];
    case "all":
      if (roleColoringEnabled && note.melody) return [MELODY_COLOR_MIN, MELODY_COLOR_MAX];
      if (roleColoringEnabled && note.bass) return [BASS_COLOR_MIN, BASS_COLOR_MAX];
      return [NOTE_COLOR_MIN, NOTE_COLOR_MAX];
  }
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

// endS is null for the still-current chord -- its right edge is computed fresh from the
// playhead each tick instead (see tickChordSegments).
interface ChordSegment {
  startS: number;
  endS: number | null;
  band: Graphics;
  boundary: Graphics;
  label: Text;
  roman: Text;
}

interface KeySprite {
  graphic: Graphics;
  isBlack: boolean;
  active: boolean;
}

/** Falling-note piano roll driven by note events. A note is first reported
 * when its onset is decided (`NoteStarted`, drawn growing with the clock) and
 * again with its end known (`NoteConfirmed`, which fixes its length); the
 * rest is just positioning each one along the time axis.
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
 * version had, because NoteConfirmed's end_s was itself capped at
 * (covered - confirm_margin) under the basic-pitch note model -- a little
 * behind the freshest data a ChordChange event reports -- so "smallest
 * observed gap" doesn't reliably converge on the true figure. The backend
 * computes this figure itself (it's the one side that actually knows the
 * stride/hop it settled on) and
 * sends it as `total_latency_s` on the pipeline's `Started` event; this
 * class just uses it directly and runs the displayed clock at a constant,
 * exact 1x rate offset by that fixed figure -- never sped up, slowed down,
 * or re-anchored again. */
export class PianoRoll {
  private app: Application;
  // One layer per part inside the note layer, so a part can be hidden as a whole and
  // overlapping notes stack in a fixed order (see PART_DRAW_ORDER).
  private partLayers = {} as Record<NotePart, Container>;
  private beatLayer = new Container();
  private beatLines: BeatSprite[] = [];
  private chordLayer = new Container();
  private chordSegments: ChordSegment[] = [];
  private guides = new Graphics();
  private keyboardBacking = new Graphics();
  private keys = new Map<number, KeySprite>();
  private notes: NoteSprite[] = [];
  private running = false;
  private resetAtMs = 0;
  private displayLagS = 0;
  private displayLagLocked = false;
  private lagSampleMinS = Infinity;
  // Whether newly-added notes get the melody/bass color treatment (see
  // addNote) or just the default coloring, as if every note were an inner
  // voice. Toggling only affects notes added from here on -- notes already
  // on screen keep whatever color they were drawn with, same as changing
  // any setting mid-session not affecting the running session.
  private roleColoringEnabled = true;

  private constructor(app: Application, noteLayer: Container) {
    this.app = app;
    for (const part of PART_DRAW_ORDER) {
      this.partLayers[part] = new Container();
      noteLayer.addChild(this.partLayers[part]);
    }
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
    app.stage.addChild(roll.chordLayer);
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

    const rowHeight = (height - CHORD_LANE_HEIGHT) / (MAX_PITCH - MIN_PITCH);
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
    const rowHeight = (this.app.screen.height - CHORD_LANE_HEIGHT) / (MAX_PITCH - MIN_PITCH);
    this.redrawKey(key, rowHeight);
  }

  // Rows fill [CHORD_LANE_HEIGHT, height], not [0, height] -- the chord lane at the top is
  // reserved space, like KEYBOARD_WIDTH is on the right, not an overlay on top of note rows.
  private pitchToY(pitch: number, height: number): number {
    const t = (pitch - MIN_PITCH) / (MAX_PITCH - MIN_PITCH);
    return height - t * (height - CHORD_LANE_HEIGHT);
  }

  /** Clears the view and starts the scroll clock. Call when a session
   * starts, with the backend-computed confirm-delay figure for that
   * session (from the pipeline's `Started { total_latency_s }` event). */
  reset(_totalLatencyS: number): void {
    this.running = true;
    this.resetAtMs = performance.now();
    this.displayLagS = 0;
    this.displayLagLocked = false;
    this.lagSampleMinS = Infinity;
    for (const n of this.notes) n.graphic.destroy();
    this.notes = [];
    for (const b of this.beatLines) b.graphic.destroy();
    this.beatLines = [];
    for (const c of this.chordSegments) {
      c.band.destroy();
      c.boundary.destroy();
      c.label.destroy();
      c.roman.destroy();
    }
    this.chordSegments = [];
    for (const pitch of this.keys.keys()) this.setKeyActive(pitch, false);
  }

  private wallClockS(): number {
    return (performance.now() - this.resetAtMs) / 1000;
  }

  // The playhead (keyboard's left edge) shows the audio clock minus `displayLagS`, the measured
  // delay between a note's onset and its NoteStarted arriving (see startNote). It is measured once,
  // in the first LAG_MEASURE_S of a session, and then fixed -- never re-estimated -- so the scroll
  // stays a constant-rate function of the wall clock. total_latency_s (the wait for a note's
  // *end*) is not used: a note is drawn from its onset.
  private displayS(): number {
    return this.wallClockS() - this.displayLagS;
  }

  private xForTime(t: number): number {
    return this.playheadX() + (t - this.displayS()) * PIXELS_PER_SECOND;
  }

  setRoleColoringEnabled(enabled: boolean): void {
    this.roleColoringEnabled = enabled;
  }

  /** Shows or hides every note of one model part, past and future. Only affects the drawing:
   * hidden notes still count for the chord, key and the highlighted keys. */
  setPartVisible(part: NotePart, visible: boolean): void {
    this.partLayers[part].visible = visible;
  }

  private drawNote(graphic: Graphics, note: ConfirmedNote): void {
    const noteHeightPx = Math.max(3, (this.app.screen.height - CHORD_LANE_HEIGHT) / (MAX_PITCH - MIN_PITCH) - 1);
    const widthPx = Math.max(2, (note.end_s - note.start_s) * PIXELS_PER_SECOND);
    const [colorMin, colorMax] = noteColors(note, this.roleColoringEnabled);
    const color = lerpColor(colorMin, colorMax, note.velocity);
    graphic.clear().roundRect(0, 0, widthPx, noteHeightPx, 2).fill({ color });
  }

  /** Draws a note from its onset; it grows with the clock until addNote() delivers its end. */
  startNote(note: StartedNote): void {
    // Smallest onset-to-arrival delay seen so far: the freshest notes then land exactly on the
    // keyboard edge, later-detected ones a little to its left.
    if (this.displayLagLocked === false) this.lagSampleMinS = Math.min(this.lagSampleMinS, this.wallClockS() - note.start_s);
    this.addSprite({ ...note, end_s: this.wallClockS() - OPEN_NOTE_LAG_S, melody: false, bass: false }, true);
  }

  addNote(note: ConfirmedNote): void {
    const open = this.notes.find((n) => n.open && n.part === note.part && n.pitch_midi === note.pitch_midi && n.start_s === note.start_s);
    if (open) {
      Object.assign(open, note, { open: false });
      this.drawNote(open.graphic, open);
      return;
    }
    // Ended before it was ever reported as started (shorter than one block).
    this.addSprite(note, false);
  }

  private addSprite(note: ConfirmedNote, open: boolean): void {
    const height = this.app.screen.height;
    const noteHeightPx = Math.max(3, (height - CHORD_LANE_HEIGHT) / (MAX_PITCH - MIN_PITCH) - 1);
    const graphic = new Graphics();
    this.drawNote(graphic, note);
    // Position it correctly right away -- left at the Graphics default
    // (x=0, the far-left edge where the oldest, about-to-be-pruned notes
    // sit) until the next tick, a newly-added note would flash there for
    // a frame, reading as "a new note suddenly appearing where one
    // already is."
    graphic.x = this.xForTime(note.start_s);
    graphic.y = this.pitchToY(note.pitch_midi, height) - noteHeightPx / 2;
    this.partLayers[note.part].addChild(graphic);
    this.notes.push({ ...note, graphic, open });
  }

  /** Appends a newly stabilized chord (rekord_core's ChordStabilizer already debounced it --
   * this never revises or removes a segment, only closes the previous one off at `t` and opens
   * a new one). `t` is the pipeline's own clock, positioned with xForTime like everything else. */
  addChordChange(t: number, label: string, romanNumeral: string | null): void {
    const previous = this.chordSegments[this.chordSegments.length - 1];
    if (previous) previous.endS = t;

    const band = new Graphics().rect(0, 0, 1, 1).fill({ color: 0xffffff });
    band.tint = this.chordSegments.length % 2 === 0 ? CHORD_BAND_COLOR_A : CHORD_BAND_COLOR_B;
    const boundary = new Graphics().rect(0, 0, 1, 1).fill({ color: CHORD_BOUNDARY_COLOR });
    const isSilence = label === SILENCE_LABEL;
    const labelText = new Text({
      text: label,
      style: { fontSize: CHORD_LABEL_FONT_SIZE, fontWeight: "600", fill: isSilence ? CHORD_SILENCE_COLOR : CHORD_TEXT_COLOR },
    });
    const romanText = new Text({
      text: romanNumeral ?? "",
      style: { fontSize: CHORD_ROMAN_FONT_SIZE, fill: CHORD_ROMAN_COLOR },
    });
    band.visible = !isSilence;
    this.chordLayer.addChild(band, boundary, labelText, romanText);
    this.chordSegments.push({ startS: t, endS: null, band, boundary, label: labelText, roman: romanText });
  }

  /** Respells the still-current chord (the accidental setting changed mid-session); chords that
   * already scrolled into the past keep the spelling they were drawn with. */
  relabelCurrentChord(label: string): void {
    const current = this.chordSegments[this.chordSegments.length - 1];
    if (current && current.label.text !== SILENCE_LABEL) current.label.text = label;
  }

  /** Positions every chord segment for the current displayS(), same as onTick does for notes,
   * and prunes ones fully scrolled past the left edge. The label/roman text sticks to the
   * visible left edge of its segment (clamped between the segment's own bounds) rather than
   * scrolling off with its start time, so a long-held chord's name stays readable the whole
   * time it's current -- the same reasoning a sticky table header uses. */
  private tickChordSegments(): void {
    const kept: ChordSegment[] = [];
    for (const seg of this.chordSegments) {
      const leftX = this.xForTime(seg.startS);
      const rightX = seg.endS !== null ? this.xForTime(seg.endS) : this.playheadX();
      if (rightX < 0) {
        seg.band.destroy();
        seg.boundary.destroy();
        seg.label.destroy();
        seg.roman.destroy();
        continue;
      }
      const width = Math.max(0, rightX - leftX);
      seg.band.x = leftX;
      seg.band.scale.set(width, CHORD_LANE_HEIGHT);
      seg.boundary.x = leftX;
      seg.boundary.scale.set(1.5, CHORD_LANE_HEIGHT);

      // Widest of the label/roman text decides how much room the sticky clamp gives itself --
      // a short label like "C" shouldn't get pinned any further right than a wide one would.
      const textWidth = Math.max(seg.label.width, seg.roman.width);
      const maxTextX = rightX - textWidth - CHORD_LABEL_PADDING;
      const textX = Math.max(leftX + CHORD_LABEL_PADDING, Math.min(CHORD_LABEL_PADDING, maxTextX));
      seg.label.x = textX;
      seg.label.y = 2;
      seg.roman.x = textX;
      seg.roman.y = CHORD_LANE_HEIGHT - seg.roman.height - 2;
      kept.push(seg);
    }
    this.chordSegments = kept;
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
    if (!this.displayLagLocked && this.wallClockS() >= LAG_MEASURE_S && Number.isFinite(this.lagSampleMinS)) {
      this.displayLagS = this.lagSampleMinS + LAG_MARGIN_S;
      this.displayLagLocked = true;
    }
    this.tickBeatLines(this.app.ticker.deltaMS / 1000);
    this.tickChordSegments();
    const displayS = this.displayS();
    const activePitches = new Set<number>();
    const stillVisible: NoteSprite[] = [];
    const openEndS = this.wallClockS() - OPEN_NOTE_LAG_S;
    for (const n of this.notes) {
      if (n.open) {
        n.end_s = Math.max(n.end_s, openEndS);
        this.drawNote(n.graphic, n);
      }
      const x = this.xForTime(n.start_s);
      n.graphic.x = x;
      if (x + n.graphic.width < 0) {
        // Scrolled fully off the left edge -- drop it so the note list
        // (and the number of display objects) doesn't grow forever over
        // a long session.
        n.graphic.destroy();
      } else {
        stillVisible.push(n);
        // An open note's end_s only trails the clock; it is still sounding.
        if (displayS >= n.start_s && (n.open || displayS <= n.end_s)) activePitches.add(n.pitch_midi);
      }
    }
    this.notes = stillVisible;
    for (const pitch of this.keys.keys()) {
      this.setKeyActive(pitch, activePitches.has(pitch));
    }
  }
}
