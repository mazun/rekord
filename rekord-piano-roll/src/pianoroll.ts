import { Application, Container, Graphics, Text } from "pixi.js";
import { planChordUpdate, type LaneSegment, type LaneUpdate } from "./chordlane";
import { HeldClock, LiveClock, PlaybackClock, type RollClock } from "./clock";
import { HISTORY_S } from "./recording";

const MIN_PITCH = 21; // A0
const MAX_PITCH = 108; // C8
const PIXELS_PER_SECOND = 120; // at zoom 1; see setZoom
// Notes are shaded between these two by velocity (the model's own
// per-pitch activation strength for that note, in [0, 1]) rather than
// drawn in one fixed color -- so within a chord, a note that sounded
// weaker actually reads as dimmer, not just as "a note" with no
// indication of how hard it was played relative to its neighbors.
const NOTE_COLOR_MIN = 0x2c3550; // quiet: closer to the guide/background grey-blue
const NOTE_COLOR_MAX = 0xaecbff; // loud: bright, near-white blue
// The model's parts get one palette each, all with the same velocity shading: vocal is drawn
// in green, bass in orange, other in the default blue (see noteColors).
const MELODY_COLOR_MIN = 0x1f3d2c;
const MELODY_COLOR_MAX = 0x6be3a0;
const BASS_COLOR_MIN = 0x3d2c1f;
const BASS_COLOR_MAX = 0xff9f5b;
const GUIDE_COLOR = 0x262b36;
const WHITE_ROW_COLOR = 0xffffff; // tint behind white-key pitches (untuned)
const WHITE_ROW_ALPHA = 0.08;

// Beat/bar lines (rekord_core::beat_tracker, via PipelineEvent::BeatLines) come in three
// layers of the same beats: predicted (extrapolated, no delay), provisional (the network's
// peaks, ~0.6s behind the audio) and committed (the DBN's final decision, ~4.5s behind). With
// the display running total_latency_s (~0.9s at the default 0.26s look-ahead, ~1.1s at the
// 0.51s look-ahead) behind the audio, predicted lines are still drawn before they reach the
// playhead, but provisional ones mostly firm up just after passing it, and a line turns
// committed a few seconds after that. All three layers look the same so a line firming up
// doesn't pull the eye away from the newest notes. A bar line (downbeat) is stronger and
// wider than a beat.
type BeatLayer = "predicted" | "provisional" | "committed";
const BEAT_LINE_COLOR = 0x5a6580;
const BAR_LINE_COLOR = 0x9aaad8;
const BEAT_LINE_ALPHA: Record<BeatLayer, number> = { predicted: 0.4, provisional: 0.4, committed: 0.4 };
const BAR_LINE_WIDTH = 2;
// A new snapshot's line within this of a line already drawn is the same beat moving to its
// next layer (its time can shift a little when it does); beats are >= 0.28s apart (215 BPM).
const BEAT_MATCH_TOLERANCE_S = 0.1;
// Time constant of every beat-line transition (layer, bar/beat, position, fade in/out).
const BEAT_ANIM_TAU_S = 0.15;

// Chord history lane (rekord_core::chord_layers, composed by chordlane.ts): a strip along the top
// showing each chord as it scrolls past, like the notes below it. The chord layers decide at
// different delays and a later one corrects an earlier one, so the lane is not append-only: an
// update (LaneUpdate) says "from here on, the lane is these segments", and each segment is
// matched to the one already drawn with the same chord (planChordUpdate). A chord that is still
// there only slides to its new span; one that appeared or disappeared fades in or out. So where the
// layers agree nothing moves, and the look carries no trace of which layer a chord came from. The
// still-current chord's right edge is redrawn at the playhead every tick instead of a fixed value.
// Bands alternate tint solely so consecutive chords read as distinct spans; it carries no other
// meaning. The roman numeral is repeated here (already shown big in ChordDisplay.svelte) because the
// point of putting chords on the roll at all is seeing the *progression* scroll past, and the
// numeral is the part of that worth reading after the chord itself has scrolled away from the
// always-current ChordDisplay.
// How fast a chord that kept its identity slides to its corrected span, and how fast a chord the
// correction added fades in / one it took away fades out (all untuned).
const CHORD_ANIM_TAU_S = 0.15;
const CHORD_FADE_IN_TAU_S = 0.12;
const CHORD_FADE_OUT_TAU_S = 0.08;
const CHORD_LANE_HEIGHT = 34;
const CHORD_FONT_FAMILY = '"Accidentals", "Segoe UI", sans-serif'; // same stack as styles.css
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
const SILENCE_LABEL = "--";

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
// "C4"-style octave labels on the C keys; hidden when the rows are too short for the minimum size.
const OCTAVE_LABEL_COLOR = 0x3a3f4d;
const OCTAVE_LABEL_BOTTOM_PADDING = 1;
const OCTAVE_LABEL_MAX_FONT_SIZE = 9; // "C4" must fit the 13.6px gap right of the black keys
const OCTAVE_LABEL_MIN_FONT_SIZE = 7;

// While stopped, the wheel/trackpad scrolls this many times faster than the content moves under
// the pointer when dragging, since a wheel notch is only ~100 px and the history is minutes long.
const WHEEL_SCROLL_SPEED = 4;
const WHEEL_LINE_PX = 16; // deltaMode "line" -> pixels
// Scrollbar along the bottom of the note area (stopped and scrollable only): the thumb is the
// visible window within the retained history, its right end = the stop position.
const SCROLLBAR_HEIGHT = 10;
const SCROLLBAR_BOTTOM_PADDING = 4; // gap between the scrollbar and the bottom edge
const SCROLLBAR_HIT_HEIGHT = 24; // clickable strip above the bottom edge, wider than what's drawn
const SCROLLBAR_MIN_THUMB_PX = 24;
const SCROLLBAR_TRACK_COLOR = 0x1a1c22;
const SCROLLBAR_THUMB_COLOR = 0x5a6580;
const SCROLLBAR_THUMB_ACTIVE_COLOR = 0x9aaad8;
// A press that moves less than this (px) and is released within CLICK_MAX_MS is a click -- it sets
// the playback position -- not a drag that scrolls.
const CLICK_SLOP_PX = 4;
// A select drag with the pointer outside the note area scrolls the view (px of roll per second).
const AUTOSCROLL_BASE_PX_PER_S = 200;
const AUTOSCROLL_PX_PER_S_PER_PX = 8;
const AUTOSCROLL_MAX_PX_PER_S = 2500;
const CLICK_MAX_MS = 500;
// The playback position marker: a line the full height of the roll, with a small flag on the chord
// lane.
const MARKER_COLOR = 0xffd166;
const MARKER_LINE_WIDTH = 2;
const MARKER_FLAG_SIZE = 6;
// The range selected by dragging (debug builds export it): a translucent band the full height
// of the roll with an edge line at each end. Anything shorter than this is a stray drag.
const SELECTION_COLOR = 0x8fb0ff;
const SELECTION_FILL_ALPHA = 0.16;
const SELECTION_EDGE_ALPHA = 0.9;
const SELECTION_EDGE_WIDTH = 2;
const SELECTION_MIN_S = 0.05;
/** How close (px) a press must be to a selection edge to drag it. */
const SELECTION_GRAB_PX = 6;
/** Beats further apart than this (a gap in the detected beats) have no eighth-note grid between them. */
const SNAP_MAX_BEAT_GAP_S = 2;
// The playback cursor while it travels along the roll (before it reaches the keyboard edge).
const CURSOR_COLOR = 0xffffff;
const CURSOR_LINE_WIDTH = 2;
const CURSOR_LINE_ALPHA = 0.85;

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

// Which output part of the note model a note came from (rekord_core's Part).
export type NotePart = "vocal" | "bass" | "other";

export interface ConfirmedNote {
  start_s: number;
  end_s: number;
  pitch_midi: number;
  velocity: number;
  part: NotePart;
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

// Bottom to top: when notes of different parts overlap at the same pitch, the vocal is drawn
// over the bass over the rest.
const PART_DRAW_ORDER: NotePart[] = ["other", "bass", "vocal"];

function noteColors(note: ConfirmedNote): [number, number] {
  switch (note.part) {
    case "vocal":
      return [MELODY_COLOR_MIN, MELODY_COLOR_MAX];
    case "bass":
      return [BASS_COLOR_MIN, BASS_COLOR_MAX];
    case "other":
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

// startS/endS are where the chord should be (endS is null for the still-current chord -- its right
// edge is computed fresh from the playhead each tick instead, see tickChordSegments); shownStartS/
// shownEndS ease towards them, alpha towards targetAlpha, tint towards targetTint.
interface ChordSprite {
  label: string;
  startS: number;
  endS: number | null;
  shownStartS: number;
  shownEndS: number | null;
  alpha: number;
  targetAlpha: number;
  fadeTau: number;
  tint: number;
  targetTint: number;
  /** Taken away by a correction: fading out, then destroyed. */
  dying: boolean;
  /** Not yet given its tint. */
  fresh: boolean;
  band: Graphics;
  boundary: Graphics;
  labelText: Text;
  romanText: Text;
}

interface KeySprite {
  graphic: Graphics;
  isBlack: boolean;
  active: boolean;
}

/** Scrolling piano roll driven by note events. A note is first reported
 * when its onset is decided (`NoteStarted`, drawn growing with the clock) and
 * again with its end known (`NoteConfirmed`, which fixes its length); the
 * rest is just positioning each one along the time axis.
 *
 * What time the playhead is at comes from a `RollClock` (clock.ts): while capture
 * runs, a constant-rate function of the wall clock offset by a display lag that is
 * measured once per session and never re-estimated (chasing each event instead
 * read as visible warping; see CLAUDE.md); once stopped, a fixed position the user
 * can scroll and click; during playback, the position of the audio being heard. */
export class PianoRoll {
  private app: Application;
  // One layer per part inside the note layer, so a part can be hidden as a whole and
  // overlapping notes stack in a fixed order (see PART_DRAW_ORDER).
  private partLayers = {} as Record<NotePart, Container>;
  private beatLayer = new Container();
  private beatLines: BeatSprite[] = [];
  private chordLayer = new Container();
  // Chords fading out, under the ones that replace them.
  private chordGhostLayer = new Container();
  private chordSegments: ChordSprite[] = [];
  // The newest time the lane knew of at the previous update (a new chord starting near it is the
  // progression moving on, see planChordUpdate).
  private chordPrevHeadS = -Infinity;
  private guides = new Graphics();
  private keyboardBacking = new Graphics();
  private octaveLabels = new Map<number, Text>();
  private keys = new Map<number, KeySprite>();
  private notes: NoteSprite[] = [];
  // Whether a session has ever started (nothing is drawn before that).
  private running = false;
  // What time the playhead is at (see clock.ts): the live clock while capture runs, a held one
  // once it stops (scrollable), a playback one while replaying the recording.
  private live = new LiveClock();
  // Horizontal scale (how fast the roll scrolls past), set by the zoom slider.
  private pixelsPerSecond = PIXELS_PER_SECOND;
  private clock: RollClock = this.live;
  // The range a held view can be scrolled over, recomputed every tick: from where the oldest
  // retained item reaches the left edge to the end of the recorded audio.
  private minViewS = 0;
  private maxViewS = 0;
  // The end of the audio the backend recorded (set when capture stops); playback and the held view
  // can reach it.
  private rangeEndS = 0;
  // What a pointer drag moves: the notes themselves, or the scrollbar thumb.
  private dragMode: "none" | "content" | "thumb" | "select" | "dragout" = "none";
  /** The part of the note a drag out of the selection was pressed on (null: pressed on empty roll). */
  private dragOutPart: NotePart | null = null;
  // The press in progress, until it moves too far or is released; released in time it is a click.
  private press: { x: number; clientX: number; clientY: number; ms: number; tS: number; moved: boolean } | null = null;
  private scrollbarLayer = new Container();
  private scrollbarTrack = new Graphics();
  private scrollbarThumb = new Graphics();
  private thumbX = 0;
  private thumbW = 0;
  private drawnThumb = { w: -1, active: false };
  // Start time of the oldest retained note/chord/beat line, recomputed every tick.
  private oldestS = Infinity;
  private markerS: number | null = null;
  private markerLayer = new Container();
  private markerLine = new Graphics().rect(0, 0, 1, 1).fill({ color: 0xffffff });
  private markerFlag = new Graphics();
  // The dragged range of audio time, until a click elsewhere or a new session clears it.
  private selection: { startS: number; endS: number } | null = null;
  private selectAnchorS = 0;
  private selectPointerX = 0;
  /** The current select drag started on an edge of the existing selection (adjusting it). */
  private edgeGrab = false;
  /** Alt is held during the select drag or click: no snapping to eighth notes. */
  private freeSelect = false;
  // Space is held: dragging scrolls instead of selecting.
  private spaceHeld = false;
  private removeKeyListeners: (() => void) | null = null;
  private selectionLayer = new Container();
  private selectionFill = new Graphics().rect(0, 0, 1, 1).fill({ color: SELECTION_COLOR });
  private selectionEdges = [0, 1].map(() => new Graphics().rect(0, 0, 1, 1).fill({ color: SELECTION_COLOR }));
  private cursorLayer = new Container();
  private cursorLine = new Graphics().rect(0, 0, 1, 1).fill({ color: CURSOR_COLOR });
  /** The user clicked the roll at audio time `t` (stopped or playing, not while capturing). */
  onSeekRequest: ((t: number) => void) | null = null;
  /** The user right-clicked inside the selection at client position (x, y); true if that was
   * handled (the browser's own menu is then suppressed). */
  onSelectionContextMenu: ((clientX: number, clientY: number) => boolean) | null = null;
  /** A press inside the selection moved: the user drags the selection out of the window. `part` is
   * that of the note the press was on, null when it was on empty roll. */
  onSelectionDragOut: ((part: NotePart | null) => void) | null = null;
  /** The selected range changed (null: cleared). */
  onSelectionChange: ((range: { startS: number; endS: number } | null) => void) | null = null;
  /** Called every frame while the roll is not live, with the time at the playhead. */
  onPlayheadTime: ((t: number) => void) | null = null;

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
    app.stage.addChild(roll.chordGhostLayer);
    app.stage.addChild(roll.chordLayer);
    app.stage.addChild(roll.selectionLayer);
    roll.selectionLayer.addChild(roll.selectionFill, ...roll.selectionEdges);
    roll.selectionFill.alpha = SELECTION_FILL_ALPHA;
    for (const edge of roll.selectionEdges) edge.alpha = SELECTION_EDGE_ALPHA;
    roll.selectionLayer.visible = false;
    app.stage.addChild(roll.markerLayer);
    roll.markerLayer.addChild(roll.markerLine, roll.markerFlag);
    roll.markerLine.tint = MARKER_COLOR;
    roll.markerFlag.poly([-MARKER_FLAG_SIZE, 0, MARKER_FLAG_SIZE, 0, 0, MARKER_FLAG_SIZE * 1.2]).fill({ color: MARKER_COLOR });
    roll.markerLayer.visible = false;
    app.stage.addChild(roll.cursorLayer);
    roll.cursorLayer.addChild(roll.cursorLine);
    roll.cursorLine.alpha = CURSOR_LINE_ALPHA;
    roll.cursorLayer.visible = false;
    app.stage.addChild(roll.scrollbarLayer);
    roll.scrollbarLayer.addChild(roll.scrollbarTrack, roll.scrollbarThumb);
    roll.scrollbarLayer.visible = false;
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

    roll.attachScrollInput(app.canvas);
    app.ticker.add(() => roll.onTick());
    return roll;
  }

  /** Lets a stopped roll be scrolled back through its history: the wheel/trackpad (down = later,
   * like any scrolled view), Space+drag, or the scrollbar. A click (a press that hardly moves) asks
   * for the playback position (`onSeekRequest`), also while playing. A plain drag selects a range
   * (`onSelectionChange`); a click outside it clears it. While capture runs only the selection and
   * the click work (the roll can't be scrolled, the export is off). */
  private attachScrollInput(canvas: HTMLCanvasElement): void {
    canvas.style.touchAction = "none";
    // Space switches the drag from selecting to scrolling (like a DAW's/Photoshop's hand tool).
    const setSpace = (held: boolean) => {
      if (this.spaceHeld === held) return;
      this.spaceHeld = held;
      canvas.style.cursor = held ? (this.dragMode === "content" ? "grabbing" : "grab") : "";
    };
    // A toolbar button that was just clicked keeps the focus, and would take the Space (a click on it)
    // instead of the roll: pointing at the roll takes the focus off it.
    const releaseButtonFocus = () => {
      if (document.activeElement instanceof HTMLButtonElement) document.activeElement.blur();
    };
    canvas.addEventListener("pointerenter", releaseButtonFocus);
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.code !== "Space" || e.repeat || this.clock.kind === "live") return;
      const target = e.target as HTMLElement | null;
      if (target && target !== document.body && target !== canvas) return; // a focused button or input keeps Space
      e.preventDefault();
      setSpace(true);
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code === "Space") setSpace(false);
    };
    const onBlur = () => setSpace(false);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);
    this.removeKeyListeners = () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
    };
    canvas.addEventListener(
      "wheel",
      (e) => {
        if (!this.held) return;
        e.preventDefault();
        const unit = e.deltaMode === WheelEvent.DOM_DELTA_LINE ? WHEEL_LINE_PX : 1;
        this.scrollBy(-((e.deltaY + e.deltaX) * unit * WHEEL_SCROLL_SPEED) / this.pixelsPerSecond);
      },
      { passive: false },
    );
    canvas.addEventListener("pointerdown", (e) => {
      const rect = canvas.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;
      canvas.setPointerCapture(e.pointerId);
      this.press =
        e.button === 0
          ? { x, clientX: e.clientX, clientY: e.clientY, ms: performance.now(), tS: this.timeAtX(x), moved: false }
          : null;
      if (e.button !== 0) return;
      if (this.held && this.scrollbarLayer.visible && y >= this.app.screen.height - SCROLLBAR_HIT_HEIGHT && x < this.playheadX()) {
        // Clicking the track outside the thumb centers the thumb there first, then drags it.
        if (x < this.thumbX || x > this.thumbX + this.thumbW) this.setThumbX(x - this.thumbW / 2);
        this.dragMode = "thumb";
      } else if (this.spaceHeld) {
        if (this.held) this.dragMode = "content";
        canvas.style.cursor = "grabbing";
      } else if (x < this.playheadX()) {
        // The selection starts once the press has moved (a click is a click, not an empty range).
        // Grabbing one of its edges instead drags that edge, the other one staying as the anchor.
        const edge = this.edgeAtX(x);
        if (edge === null && this.insideSelection(this.press?.tS ?? this.timeAtX(x))) {
          // A drag from inside the selection takes it out instead of making a new one; a click
          // there still only moves the position.
          this.dragMode = "dragout";
          this.dragOutPart = this.noteAt(x, y)?.part ?? null;
          return;
        }
        this.dragMode = "select";
        this.edgeGrab = edge !== null;
        if (edge !== null && this.selection) {
          this.selectAnchorS = edge === "start" ? this.selection.endS : this.selection.startS;
        } else {
          this.freeSelect = e.altKey;
          this.selectAnchorS = this.snapTime(this.press?.tS ?? this.timeAtX(x));
        }
        this.selectPointerX = x;
      }
    });
    canvas.addEventListener("pointermove", (e) => {
      if (this.dragMode === "none" && !this.spaceHeld) {
        const x = e.clientX - canvas.getBoundingClientRect().left;
        const onRoll = x < this.playheadX();
        canvas.style.cursor = onRoll && this.edgeAtX(x) !== null ? "ew-resize" : onRoll && this.insideSelection(this.timeAtX(x)) ? "grab" : "";
      }
      if (this.press && !this.press.moved && Math.hypot(e.clientX - this.press.clientX, e.clientY - this.press.clientY) > CLICK_SLOP_PX) {
        this.press.moved = true;
      }
      if (this.dragMode === "dragout") {
        if (this.press?.moved) {
          // The OS drag takes over the pointer from here: this press is over.
          const part = this.dragOutPart;
          this.press = null;
          this.dragMode = "none";
          if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId);
          this.onSelectionDragOut?.(part);
        }
      } else if (this.dragMode === "select") {
        this.freeSelect = e.altKey;
        this.selectPointerX = e.clientX - canvas.getBoundingClientRect().left;
        if (this.press?.moved) this.updateSelection();
      } else if (this.dragMode === "content") {
        this.scrollBy(e.movementX / this.pixelsPerSecond);
      } else if (this.dragMode === "thumb") {
        const span = this.playheadX() - this.thumbW;
        if (span > 0) this.scrollBy((-e.movementX * this.scrollSpanS()) / span);
      }
    });
    canvas.addEventListener("pointerup", (e) => {
      this.freeSelect = e.altKey;
      const press = this.press;
      const mode = this.dragMode;
      this.press = null;
      this.dragMode = "none";
      if (this.spaceHeld) canvas.style.cursor = "grab";
      if (mode === "select" && press?.moved) {
        const range = this.selection;
        if (!range || range.endS - range.startS < SELECTION_MIN_S) this.setSelection(null);
        // The playhead goes to the range's left edge.
        else this.onSeekRequest?.(range.startS);
        return;
      }
      if (mode === "select" && this.edgeGrab) return; // a click on an edge changes nothing
      const isClick = press && !press.moved && mode !== "thumb" && performance.now() - press.ms < CLICK_MAX_MS;
      // Not on the keyboard: its keys aren't positions.
      if (isClick && press.x < this.playheadX()) {
        // A click inside the selection only moves the position (to play or look at it).
        const range = this.selection;
        if (range && (press.tS < range.startS || press.tS > range.endS)) this.setSelection(null);
        this.onSeekRequest?.(this.snapTime(press.tS));
      }
    });
    canvas.addEventListener("pointercancel", () => {
      this.press = null;
      this.dragMode = "none";
    });
    canvas.addEventListener("contextmenu", (e) => {
      const range = this.selection;
      const x = e.clientX - canvas.getBoundingClientRect().left;
      if (this.clock.kind === "live" || !range || x >= this.playheadX()) return;
      const t = this.timeAtX(x);
      if (t >= range.startS && t <= range.endS && this.onSelectionContextMenu?.(e.clientX, e.clientY)) e.preventDefault();
    });
  }

  /** Scrolls the held view while a select drag has the pointer outside the note area, faster the
   * further out it is. */
  private autoScrollForSelect(dtS: number): void {
    if (!this.held) return;
    const x = this.selectPointerX;
    const right = this.playheadX();
    const over = x < 0 ? x : x > right ? x - right : 0; // negative: past the left edge
    if (over === 0) return;
    const pxPerS = Math.min(AUTOSCROLL_MAX_PX_PER_S, AUTOSCROLL_BASE_PX_PER_S + Math.abs(over) * AUTOSCROLL_PX_PER_S_PER_PX);
    this.scrollBy(Math.sign(-over) * (pxPerS * dtS) / this.pixelsPerSecond);
  }

  /** The selection between the anchor and the pointer's current time (it follows the pointer while
   * the view scrolls under it, so this is also called every frame during the drag). */
  private updateSelection(): void {
    const x = Math.min(this.playheadX(), Math.max(0, this.selectPointerX));
    const t = this.snapTime(this.timeAtX(x));
    this.setSelection({ startS: Math.min(this.selectAnchorS, t), endS: Math.max(this.selectAnchorS, t) });
  }

  /** `t` snapped to the nearest eighth note: a beat line or the middle between two neighbouring ones.
   * Not snapped while Alt is held (the DAW convention for free movement), outside the detected beats,
   * or across a gap in them. */
  private snapTime(t: number): number {
    if (this.freeSelect) return t;
    const beats = this.beatLines.filter((b) => !b.dying).map((b) => b.targetT).sort((a, b) => a - b);
    if (beats.length < 2 || t < beats[0] || t > beats[beats.length - 1]) return t;
    let best = t;
    let bestD = Infinity;
    const consider = (c: number) => {
      const d = Math.abs(c - t);
      if (d < bestD) [best, bestD] = [c, d];
    };
    for (let i = 0; i < beats.length; i++) {
      if (beats[i] > t + SNAP_MAX_BEAT_GAP_S) break;
      consider(beats[i]);
      if (i + 1 < beats.length && beats[i + 1] - beats[i] <= SNAP_MAX_BEAT_GAP_S) consider((beats[i] + beats[i + 1]) / 2);
    }
    return best;
  }

  /** Time `t` lies in the selection, which can be dragged out (not while capturing: the roll is live). */
  private insideSelection(t: number): boolean {
    const range = this.selection;
    return range !== null && this.clock.kind !== "live" && t >= range.startS && t <= range.endS;
  }

  /** The visible note drawn at canvas position (x, y), the newest one when several overlap. */
  private noteAt(x: number, y: number): NoteSprite | null {
    const noteHeightPx = this.noteHeightPx(this.app.screen.height);
    const pad = 2; // thin notes are hard to hit exactly
    for (let i = this.notes.length - 1; i >= 0; i--) {
      const n = this.notes[i];
      if (!this.partLayers[n.part].visible) continue;
      const w = Math.max(2, (n.end_s - n.start_s) * this.pixelsPerSecond);
      if (x >= n.graphic.x - pad && x <= n.graphic.x + w + pad && y >= n.graphic.y - pad && y <= n.graphic.y + noteHeightPx + pad) return n;
    }
    return null;
  }

  /** Which edge of the selection lies within grabbing distance of canvas position `x`. */
  private edgeAtX(x: number): "start" | "end" | null {
    const range = this.selection;
    if (!range) return null;
    const dStart = Math.abs(x - this.xForTime(range.startS));
    const dEnd = Math.abs(x - this.xForTime(range.endS));
    if (Math.min(dStart, dEnd) > SELECTION_GRAB_PX) return null;
    return dStart <= dEnd ? "start" : "end";
  }

  private setSelection(range:{ startS: number; endS: number } | null): void {
    const same = range === null ? this.selection === null : this.selection?.startS === range.startS && this.selection.endS === range.endS;
    if (same) return;
    this.selection = range;
    this.onSelectionChange?.(range);
  }

  /** The range selected with Shift+drag, in audio seconds (null: none). */
  get selectedRange(): { startS: number; endS: number } | null {
    return this.selection;
  }

  clearSelection(): void {
    if (this.dragMode === "select") this.dragMode = "none";
    this.setSelection(null);
  }

  /** The audio time at horizontal position `x` of the canvas. */
  private timeAtX(x: number): number {
    return this.displayS() + (x - this.playheadX()) / this.pixelsPerSecond;
  }

  private get held(): HeldClock | null {
    return this.clock instanceof HeldClock ? this.clock : null;
  }

  /** How far back from the stop position a held view can be scrolled: until the oldest retained
   * item reaches the left edge. */
  private scrollSpanS(): number {
    return Math.max(0, this.clock.headS() - this.minViewS);
  }

  /** Scrolls so the scrollbar thumb's left edge sits at `x` (clamped to the track). */
  private setThumbX(x: number): void {
    const span = this.playheadX() - this.thumbW;
    if (span <= 0) return;
    const frac = Math.min(1, Math.max(0, x / span));
    this.held?.setView(this.clock.headS() - (1 - frac) * this.scrollSpanS());
  }

  /** Moves the held view `deltaS` further into the past (negative: towards the end). It stays within
   * the scrollable range, except that a view already outside it (playback paused before the oldest
   * note, say) can only move back in. */
  private scrollBy(deltaS: number): void {
    const held = this.held;
    if (!held) return;
    const before = held.displayS();
    const lo = Math.min(this.minViewS, before);
    const hi = Math.max(this.maxViewS, before);
    held.setView(Math.min(hi, Math.max(lo, before - deltaS)));
  }

  // Tears down the renderer and removes its canvas -- only needed so a
  // Svelte HMR remount doesn't leave a second canvas/ticker running.
  destroy(): void {
    this.removeKeyListeners?.();
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
    // White-key rows are tinted lighter than black-key rows so pitches are easy to read.
    const guideRowHeight = (height - CHORD_LANE_HEIGHT) / (MAX_PITCH - MIN_PITCH);
    for (let pitch = MIN_PITCH; pitch <= MAX_PITCH; pitch += 1) {
      if (isBlackKey(pitch)) continue;
      const y = this.pitchToY(pitch, height) - guideRowHeight / 2;
      this.guides.rect(0, y, noteAreaWidth, guideRowHeight);
    }
    this.guides.fill({ color: WHITE_ROW_COLOR, alpha: WHITE_ROW_ALPHA });
    for (let pitch = MIN_PITCH; pitch <= MAX_PITCH; pitch += 1) {
      if (pitch % 12 !== 0) continue; // one guide line per C
      const y = this.pitchToY(pitch, height);
      this.guides.moveTo(0, y).lineTo(noteAreaWidth, y);
    }
    this.guides.stroke({ width: 1, color: GUIDE_COLOR });

    this.scrollbarTrack.clear().rect(0, 0, noteAreaWidth, SCROLLBAR_HEIGHT).fill({ color: SCROLLBAR_TRACK_COLOR, alpha: 0.85 });
    this.scrollbarLayer.y = height - SCROLLBAR_HEIGHT - SCROLLBAR_BOTTOM_PADDING;
    this.drawnThumb.w = -1; // thumb height follows the layer; redraw it next tick

    this.keyboardBacking.clear().rect(0, 0, KEYBOARD_WIDTH, height).fill({ color: KEYBOARD_BACKING_COLOR });
    this.keyboardBacking.x = noteAreaWidth;
    this.keyboardBacking.y = 0;

    for (const [pitch, key] of this.keys) {
      key.graphic.x = noteAreaWidth;
      this.redrawKey(pitch);
    }
    // Octave labels sit on each C key, only when they fit inside the key's row at a readable size.
    const rowHeight = (height - CHORD_LANE_HEIGHT) / (MAX_PITCH - MIN_PITCH);
    // Right of the black key the C key is 1.5 rows tall (its own row plus the lower half of the C#
    // row), and the label goes there for the extra height.
    const fontSize = Math.min(OCTAVE_LABEL_MAX_FONT_SIZE, rowHeight * 1.5 * 0.8);
    const showOctaves = fontSize >= OCTAVE_LABEL_MIN_FONT_SIZE;
    for (const [pitch, text] of this.octaveLabels) {
      text.visible = showOctaves;
      if (!showOctaves) continue;
      text.style.fontSize = fontSize;
      text.x = noteAreaWidth + KEYBOARD_WIDTH - 1;
      text.y = this.pitchToY(pitch, height) + rowHeight / 2 - OCTAVE_LABEL_BOTTOM_PADDING; // bottom of the key
    }
    this.relayoutNotes();
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
    for (let pitch = MIN_PITCH; pitch <= MAX_PITCH; pitch++) {
      if (pitch % 12 !== 0) continue;
      // Scientific pitch notation: MIDI 60 is C4.
      const text = new Text({
        text: `C${pitch / 12 - 1}`,
        style: { fontFamily: CHORD_FONT_FAMILY, fontWeight: "600", fill: OCTAVE_LABEL_COLOR },
      });
      text.anchor.set(1, 1);
      keyboardLayer.addChild(text);
      this.octaveLabels.set(pitch, text);
    }
  }

  /** Draws one key's row. All edges are snapped to device pixels, with neighboring rows sharing
   * their boundary exactly: fractional edges antialiased into faint seams (extra lines, ragged
   * edges) and 1px lines at fractional y smeared over two pixel rows. */
  private redrawKey(pitch: number): void {
    const key = this.keys.get(pitch);
    if (!key) return;
    const { height } = this.app.screen;
    const res = this.app.renderer.resolution;
    const snap = (v: number) => Math.round(v * res) / res;
    const line = 1;
    const rowHeight = (height - CHORD_LANE_HEIGHT) / (MAX_PITCH - MIN_PITCH);
    const top = snap(this.pitchToY(pitch, height) - rowHeight / 2);
    const bottom = snap(this.pitchToY(pitch, height) + rowHeight / 2);
    key.graphic.y = top;
    const h = bottom - top;
    const g = key.graphic.clear();
    if (!key.isBlack) {
      g.rect(0, 0, KEYBOARD_WIDTH, h).fill({ color: key.active ? WHITE_KEY_ACTIVE_COLOR : WHITE_KEY_COLOR });
      // A line only toward a neighboring white key (E|F, B|C); next to a black key the white key
      // continues and the line is drawn in the black key's row instead.
      if (!isBlackKey(pitch + 1)) g.rect(0, 0, KEYBOARD_WIDTH, line);
      if (!isBlackKey(pitch - 1)) g.rect(0, h - line, KEYBOARD_WIDTH, line);
      g.fill({ color: KEY_BORDER_COLOR });
      return;
    }
    // Like a real piano, the white keys continue beside the black key: the part of this row right
    // of the black key belongs to the white key above (upper half) and below (lower half), with
    // a single boundary line through the row's center.
    const mid = snap(h / 2 - line / 2);
    const gapX = BLACK_KEY_WIDTH;
    const gapW = KEYBOARD_WIDTH - BLACK_KEY_WIDTH;
    const whiteColor = (p: number) => (this.keys.get(p)?.active ? WHITE_KEY_ACTIVE_COLOR : WHITE_KEY_COLOR);
    g.rect(gapX, 0, gapW, mid).fill({ color: whiteColor(pitch + 1) });
    g.rect(gapX, mid + line, gapW, h - mid - line).fill({ color: whiteColor(pitch - 1) });
    g.rect(gapX, mid, gapW, line).fill({ color: KEY_BORDER_COLOR });
    g.rect(0, 0, BLACK_KEY_WIDTH, h).fill({ color: key.active ? BLACK_KEY_ACTIVE_COLOR : BLACK_KEY_COLOR });
  }

  private setKeyActive(pitch: number, active: boolean): void {
    const key = this.keys.get(pitch);
    if (!key || key.active === active) return;
    key.active = active;
    this.redrawKey(pitch);
    // A white key's color also shows in the gaps of its neighboring black-key rows.
    if (!key.isBlack) {
      this.redrawKey(pitch - 1);
      this.redrawKey(pitch + 1);
    }
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
    this.live.reset();
    this.clock = this.live;
    this.rangeEndS = 0;
    this.markerS = null;
    this.dragMode = "none";
    this.press = null;
    this.setSelection(null);
    this.selectionLayer.visible = false;
    this.scrollbarLayer.visible = false;
    this.markerLayer.visible = false;
    this.cursorLayer.visible = false;
    for (const n of this.notes) n.graphic.destroy();
    this.notes = [];
    for (const b of this.beatLines) b.graphic.destroy();
    this.beatLines = [];
    for (const c of this.chordSegments) this.destroyChordSprite(c);
    this.chordSegments = [];
    this.chordPrevHeadS = -Infinity;
    for (const pitch of this.keys.keys()) this.setKeyActive(pitch, false);
  }

  /** Freezes the scroll where it is (capture stopped) and makes the history scrollable; the next
   * reset() resumes it. */
  stop(): void {
    if (this.clock.kind !== "live") return;
    this.live.freeze();
    const stopS = this.live.displayS();
    this.clock = new HeldClock(stopS, stopS);
  }

  /** What the backend recorded (once capture has stopped): playback and the view can reach its end. */
  setPlayableEnd(endS: number): void {
    this.rangeEndS = endS;
  }

  /** The playhead's time: where the audio is (the cursor), which is the keyboard's left edge
   * except while a playback that has just started or jumped moves across the roll towards it. */
  playheadS(): number {
    return this.clock.cursorS();
  }

  /** The time at the roll's left edge: the oldest thing currently on screen. */
  leftEdgeS(): number {
    return this.timeAtX(0);
  }

  /** The playback position marker (null: none). Drawn while the roll isn't playing. */
  setMarker(t: number | null): void {
    this.markerS = t;
  }

  get marker(): number | null {
    return this.markerS;
  }

  /** Starts following the audio being played from `fromS` (see PlaybackClock): the view stays
   * where it is and the cursor moves from `fromS` towards the keyboard edge. */
  startPlayback(fromS: number): void {
    if (this.clock.kind === "live") return;
    this.clock = new PlaybackClock(fromS, this.rangeEndS, this.clock.headS(), this.clock.displayS());
  }

  onPlaybackPosition(posS: number, atEpochMs: number): void {
    if (this.clock instanceof PlaybackClock) this.clock.onPosition(posS, atEpochMs);
  }

  /** Playback paused with the audio stopping at `posS`: the roll runs on to there, then holds. */
  holdPlayback(posS: number): void {
    if (this.clock instanceof PlaybackClock) this.clock.holdAt(posS);
  }

  /** Playback is over (or stopped): the view stays where it is (at least up to `posS`), the cursor
   * where it is or at `posS`, and the view can be scrolled again. */
  stopPlayback(posS?: number): void {
    if (this.clock.kind === "live") return;
    const view = this.clock.displayS();
    this.clock = new HeldClock(this.clock.headS(), posS === undefined ? view : Math.max(view, posS), posS ?? this.clock.cursorS());
  }

  // The live playhead (keyboard's left edge) shows the audio clock minus the display lag, the
  // measured delay between a note's onset and its NoteStarted arriving (see startNote). It is
  // measured once, in the first seconds of a session, and then fixed -- never re-estimated -- so
  // the scroll stays a constant-rate function of the wall clock. total_latency_s (the wait for a
  // note's *end*) is not used: a note is drawn from its onset. A held clock is a fixed (scrollable)
  // position, a playback clock follows the audio being heard; all three are in clock.ts.
  private displayS(): number {
    return this.clock.displayS();
  }

  private xForTime(t: number): number {
    return this.playheadX() + (t - this.displayS()) * this.pixelsPerSecond;
  }

  /** Scales the time axis: 2 = twice the pixels per second (slower scroll, a shorter window).
   * Notes bake their width when drawn, so they are redrawn; everything else follows xForTime. */
  setZoom(zoom: number): void {
    this.pixelsPerSecond = PIXELS_PER_SECOND * zoom;
    for (const n of this.notes) this.drawNote(n.graphic, n);
  }

  /** Shows or hides every note of one model part, past and future. Only affects the drawing:
   * hidden notes still count for the chord, key and the highlighted keys. */
  setPartVisible(part: NotePart, visible: boolean): void {
    this.partLayers[part].visible = visible;
  }

  private noteHeightPx(height: number): number {
    return Math.max(3, (height - CHORD_LANE_HEIGHT) / (MAX_PITCH - MIN_PITCH) - 1);
  }

  /** Notes bake their height and y when drawn, so a window height change has to redo them
   * (the per-tick pass only moves x). */
  private relayoutNotes(): void {
    const height = this.app.screen.height;
    const noteHeightPx = this.noteHeightPx(height);
    for (const n of this.notes) {
      this.drawNote(n.graphic, n);
      n.graphic.y = this.pitchToY(n.pitch_midi, height) - noteHeightPx / 2;
    }
  }

  private drawNote(graphic: Graphics, note: ConfirmedNote): void {
    const noteHeightPx = this.noteHeightPx(this.app.screen.height);
    const widthPx = Math.max(2, (note.end_s - note.start_s) * this.pixelsPerSecond);
    const [colorMin, colorMax] = noteColors(note);
    const color = lerpColor(colorMin, colorMax, note.velocity);
    graphic.clear().roundRect(0, 0, widthPx, noteHeightPx, 2).fill({ color });
  }

  /** Draws a note from its onset; it grows with the clock until addNote() delivers its end. */
  startNote(note: StartedNote): void {
    this.live.sampleOnset(note.start_s);
    this.addSprite({ ...note, end_s: this.live.wallS() - OPEN_NOTE_LAG_S }, true);
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
    const noteHeightPx = this.noteHeightPx(height);
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

  private destroyChordSprite(c: ChordSprite): void {
    c.band.destroy();
    c.boundary.destroy();
    c.labelText.destroy();
    c.romanText.destroy();
  }

  private makeChordSprite(seg: LaneSegment, alpha: number): ChordSprite {
    const band = new Graphics().rect(0, 0, 1, 1).fill({ color: 0xffffff });
    const boundary = new Graphics().rect(0, 0, 1, 1).fill({ color: CHORD_BOUNDARY_COLOR });
    const label = seg.chord.label;
    const isSilence = label === SILENCE_LABEL;
    const labelText = new Text({
      text: label,
      style: { fontFamily: CHORD_FONT_FAMILY, fontSize: CHORD_LABEL_FONT_SIZE, fontWeight: "600", fill: isSilence ? CHORD_SILENCE_COLOR : CHORD_TEXT_COLOR },
    });
    const romanText = new Text({
      text: seg.chord.roman_numeral ?? "",
      style: { fontFamily: CHORD_FONT_FAMILY, fontSize: CHORD_ROMAN_FONT_SIZE, fill: CHORD_ROMAN_COLOR },
    });
    this.chordLayer.addChild(band, boundary, labelText, romanText);
    return {
      label,
      startS: seg.startS,
      endS: seg.endS,
      shownStartS: seg.startS,
      shownEndS: seg.endS,
      alpha,
      targetAlpha: 1,
      fadeTau: CHORD_FADE_IN_TAU_S,
      tint: CHORD_BAND_COLOR_A,
      targetTint: CHORD_BAND_COLOR_A,
      dying: false,
      fresh: true,
      band,
      boundary,
      labelText,
      romanText,
    };
  }

  /** Applies a lane update (chordlane.ts): from `update.fromS` on, the chord lane is
   * `update.segments`. A chord that is still there (same name, overlapping) slides to its new
   * span; one the update adds fades in (or just appears, if it is the progression moving on), one
   * it takes away fades out. `t` is the pipeline's own clock, positioned with xForTime like
   * everything else. */
  setChordSegments(update: LaneUpdate): void {
    const segs = update.segments;
    const plan = planChordUpdate(this.chordSegments, update, this.chordPrevHeadS);
    this.chordPrevHeadS = update.headS;
    const openEdgeS = Math.max(this.clock.headS(), this.displayS());
    for (const [o, n] of plan.matched) {
      const sprite = this.chordSegments[o];
      const seg = segs[n];
      sprite.startS = seg.startS;
      sprite.endS = seg.endS;
      // a chord that was open (running to the playhead) and now has an end slides there from the playhead
      if (seg.endS === null) sprite.shownEndS = null;
      else if (sprite.shownEndS === null) sprite.shownEndS = openEdgeS;
      sprite.romanText.text = seg.chord.roman_numeral ?? "";
    }
    for (const o of plan.removed) {
      const sprite = this.chordSegments[o];
      sprite.dying = true;
      sprite.targetAlpha = 0;
      sprite.fadeTau = CHORD_FADE_OUT_TAU_S;
      this.chordGhostLayer.addChild(sprite.band, sprite.boundary, sprite.labelText, sprite.romanText);
    }
    for (const { index, kind } of plan.created) {
      this.chordSegments.push(this.makeChordSprite(segs[index], kind === "head" ? 1 : 0));
    }
    this.chordSegments.sort((x, y) => x.startS - y.startS);
    // Bands alternate in tint along the lane; what a correction changed is recolored, softly.
    let tint = CHORD_BAND_COLOR_B;
    for (const sprite of this.chordSegments) {
      if (sprite.dying) continue;
      tint = tint === CHORD_BAND_COLOR_A ? CHORD_BAND_COLOR_B : CHORD_BAND_COLOR_A;
      sprite.targetTint = tint;
      if (sprite.fresh) {
        sprite.tint = tint; // a new band starts in its color; only a recolor of an existing one eases
        sprite.fresh = false;
      }
    }
  }

  /** Positions every chord for the current displayS(), same as onTick does for notes, and prunes
   * ones fully scrolled past the left edge. The label/roman text sticks to the visible left edge
   * of its segment (clamped between the segment's own bounds) rather than scrolling off with its
   * start time, so a long-held chord's name stays readable the whole time it's current -- the same
   * reasoning a sticky table header uses. */
  private tickChordSegments(dtS: number): void {
    const kept: ChordSprite[] = [];
    const headS = this.clock.headS();
    const openEdgeS = Math.max(headS, this.displayS());
    const slide = 1 - Math.exp(-dtS / CHORD_ANIM_TAU_S);
    for (const seg of this.chordSegments) {
      seg.shownStartS += (seg.startS - seg.shownStartS) * slide;
      if (seg.endS === null) seg.shownEndS = null;
      else seg.shownEndS = (seg.shownEndS ?? openEdgeS) + (seg.endS - (seg.shownEndS ?? openEdgeS)) * slide;
      seg.alpha += (seg.targetAlpha - seg.alpha) * (1 - Math.exp(-dtS / seg.fadeTau));
      seg.tint = lerpColor(seg.tint, seg.targetTint, slide);
      if ((seg.dying && seg.alpha < 0.02) || (!seg.dying && seg.endS !== null && seg.endS < headS - HISTORY_S)) {
        this.destroyChordSprite(seg);
        continue;
      }
      if (!seg.dying) this.oldestS = Math.min(this.oldestS, seg.startS);
      const leftX = this.xForTime(seg.shownStartS);
      // The still-current chord runs up to the newest position, which a scrolled-back stopped roll
      // has moved away from the playhead; replaying the last stretch of the recording goes past it.
      const rightX = this.xForTime(seg.shownEndS ?? openEdgeS);
      const onScreen = rightX >= 0 && leftX <= this.app.screen.width;
      const isSilence = seg.label === SILENCE_LABEL;
      for (const g of [seg.boundary, seg.labelText, seg.romanText]) g.visible = onScreen;
      seg.band.visible = onScreen && !isSilence; // silence has no band
      if (!onScreen) {
        kept.push(seg);
        continue;
      }
      for (const g of [seg.band, seg.boundary, seg.labelText, seg.romanText]) g.alpha = seg.alpha;
      const width = Math.max(0, rightX - leftX);
      seg.band.tint = seg.tint;
      seg.band.x = leftX;
      seg.band.scale.set(width, CHORD_LANE_HEIGHT);
      seg.boundary.x = leftX;
      seg.boundary.scale.set(1.5, CHORD_LANE_HEIGHT);

      // Widest of the label/roman text decides how much room the sticky clamp gives itself --
      // a short label like "C" shouldn't get pinned any further right than a wide one would.
      const textWidth = Math.max(seg.labelText.width, seg.romanText.width);
      const maxTextX = rightX - textWidth - CHORD_LABEL_PADDING;
      const textX = Math.max(leftX + CHORD_LABEL_PADDING, Math.min(CHORD_LABEL_PADDING, maxTextX));
      seg.labelText.x = textX;
      seg.labelText.y = 2;
      seg.romanText.x = textX;
      seg.romanText.y = CHORD_LANE_HEIGHT - seg.romanText.height - 2;
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
      if ((b.dying && b.alpha < 0.01) || b.t < this.clock.headS() - HISTORY_S) {
        b.graphic.destroy();
        continue;
      }
      this.oldestS = Math.min(this.oldestS, b.t);
      b.graphic.visible = x + width >= 0 && x <= this.app.screen.width;
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
    this.live.update();
    // A paused playback holds where the audio stopped once the display gets there.
    if (this.clock instanceof PlaybackClock && this.clock.holdReached()) {
      this.clock = new HeldClock(this.clock.headS(), this.clock.displayS(), this.clock.cursorS());
    }
    this.oldestS = Infinity;
    this.tickBeatLines(this.app.ticker.deltaMS / 1000);
    this.tickChordSegments(this.app.ticker.deltaMS / 1000);
    const cursorS = this.clock.cursorS();
    const headS = this.clock.headS();
    const live = this.clock.kind === "live";
    const activePitches = new Set<number>();
    const retained: NoteSprite[] = [];
    const openEndS = this.live.wallS() - OPEN_NOTE_LAG_S;
    const screenWidth = this.app.screen.width;
    for (const n of this.notes) {
      if (n.open) {
        n.end_s = Math.max(n.end_s, openEndS);
      }
      if (!n.open && n.end_s < headS - HISTORY_S) {
        // Older than the retained history -- drop it so the note list
        // (and the number of display objects) doesn't grow forever over
        // a long session.
        n.graphic.destroy();
        continue;
      }
      retained.push(n);
      this.oldestS = Math.min(this.oldestS, n.start_s);
      const x = this.xForTime(n.start_s);
      const right = this.xForTime(Math.max(n.end_s, n.start_s + 2 / this.pixelsPerSecond));
      // Only what is on screen is redrawn and positioned; the rest of the history just waits.
      const onScreen = right >= 0 && x <= screenWidth;
      n.graphic.visible = onScreen;
      if (!onScreen) continue;
      if (n.open) this.drawNote(n.graphic, n);
      n.graphic.x = x;
      // An open note's end_s only trails the clock; it is still sounding (while capture runs --
      // once it has stopped, an unconfirmed note ends where its end_s froze).
      if (cursorS >= n.start_s && ((n.open && live) || cursorS <= n.end_s)) activePitches.add(n.pitch_midi);
    }
    this.notes = retained;
    // Highlighted at the cursor whatever moves it: the live clock, a scrolled-back stopped roll
    // or playback.
    for (const pitch of this.keys.keys()) {
      this.setKeyActive(pitch, activePitches.has(pitch));
    }

    // A held view can be scrolled back until the oldest retained item reaches the left edge, and
    // forward to the end of the recorded audio (a little past where the live roll stopped, which
    // runs behind the audio by the display lag). The live roll always follows the clock.
    this.minViewS = Number.isFinite(this.oldestS)
      ? Math.min(headS, this.oldestS + this.playheadX() / this.pixelsPerSecond)
      : headS;
    this.maxViewS = Math.max(headS, this.rangeEndS);
    if (this.dragMode === "select" && this.press?.moved) {
      this.autoScrollForSelect(this.app.ticker.deltaMS / 1000);
      this.updateSelection();
    }
    this.tickSelection();
    this.tickMarker();
    this.tickCursor();
    if (!live) this.onPlayheadTime?.(cursorS);
    this.tickScrollbar();
  }

  /** The selected range, wherever it is on screen (clipped to the note area). */
  private tickSelection(): void {
    const range = this.selection;
    const right = this.playheadX();
    const x0 = range ? this.xForTime(range.startS) : NaN;
    const x1 = range ? this.xForTime(range.endS) : NaN;
    const show = range !== null && x1 > 0 && x0 < right; // NaN: none
    this.selectionLayer.visible = show;
    if (!show) return;
    const height = this.app.screen.height;
    const left = Math.max(0, x0);
    this.selectionFill.x = left;
    this.selectionFill.scale.set(Math.min(right, x1) - left, height);
    // An edge is only drawn where the range really ends, not where the screen cuts it off.
    const edges: [Graphics, number][] = [
      [this.selectionEdges[0], x0],
      [this.selectionEdges[1], x1 - SELECTION_EDGE_WIDTH],
    ];
    for (const [edge, x] of edges) {
      edge.visible = x + SELECTION_EDGE_WIDTH >= 0 && x <= right - SELECTION_EDGE_WIDTH;
      edge.x = x;
      edge.scale.set(SELECTION_EDGE_WIDTH, height);
    }
  }

  /** The playback position marker, wherever its time is on screen. */
  private tickMarker(): void {
    const x = this.markerS === null ? NaN : this.xForTime(this.markerS);
    const show = this.clock.kind !== "playing" && x >= 0 && x <= this.playheadX(); // NaN: no marker
    this.markerLayer.visible = show;
    if (!show) return;
    this.markerLine.x = x - MARKER_LINE_WIDTH / 2;
    this.markerLine.scale.set(MARKER_LINE_WIDTH, this.app.screen.height);
    this.markerFlag.x = x;
  }

  /** The cursor line, while it is somewhere along the roll rather than at the keyboard edge. */
  private tickCursor(): void {
    const x = this.xForTime(this.clock.cursorS());
    const show = this.clock.kind !== "live" && x >= 0 && x < this.playheadX() - CURSOR_LINE_WIDTH;
    this.cursorLayer.visible = show;
    if (!show) return;
    this.cursorLine.x = x - CURSOR_LINE_WIDTH / 2;
    this.cursorLine.scale.set(CURSOR_LINE_WIDTH, this.app.screen.height);
  }

  /** The thumb spans the visible window (playheadX / pixelsPerSecond seconds) out of the whole
   * scrollable history; at the stop position it sits at the right end, at the oldest at the left end. */
  private tickScrollbar(): void {
    const span = this.scrollSpanS();
    const show = this.clock.kind === "held" && span > 0;
    this.scrollbarLayer.visible = show;
    if (!show) return;
    const trackW = this.playheadX();
    const visibleS = trackW / this.pixelsPerSecond;
    const offsetS = Math.min(span, Math.max(0, this.clock.headS() - this.clock.displayS()));
    this.thumbW = Math.min(trackW, Math.max(SCROLLBAR_MIN_THUMB_PX, (trackW * visibleS) / (span + visibleS)));
    this.thumbX = (1 - offsetS / span) * (trackW - this.thumbW);
    const active = this.dragMode === "thumb";
    if (this.drawnThumb.w !== this.thumbW || this.drawnThumb.active !== active) {
      this.drawnThumb = { w: this.thumbW, active };
      this.scrollbarThumb
        .clear()
        .roundRect(0, 1, this.thumbW, SCROLLBAR_HEIGHT - 2, (SCROLLBAR_HEIGHT - 2) / 2)
        .fill({ color: active ? SCROLLBAR_THUMB_ACTIVE_COLOR : SCROLLBAR_THUMB_COLOR });
    }
    this.scrollbarThumb.x = this.thumbX;
  }
}
