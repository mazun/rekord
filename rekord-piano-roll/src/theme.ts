// Every color in the app, per theme. The page reads `ui` as CSS custom properties on :root
// (applyTheme); the Pixi piano roll reads `roll` (PianoRoll.setColors). Nothing else holds a color
// literal: styles use var(--token), and a new look is one more entry in THEMES.

export type ThemeId = "dark" | "light" | "sky" | "rose";
export const THEME_IDS: readonly ThemeId[] = ["dark", "light", "sky", "rose"];
export const DEFAULT_THEME: ThemeId = "dark";

/** CSS custom properties (the key `bgRaised` becomes `--bg-raised`). */
export interface UiColors {
  bg: string;
  surface: string;
  surfaceRaised: string;
  /** Hovered/pressed controls over `surface`. */
  surfaceHover: string;
  border: string;
  borderStrong: string;
  text: string;
  textDim: string;
  textFaint: string;
  /** Sliders, focus rings and other interactive highlights. */
  accent: string;
  accentSoft: string;
  /** A selected segment of a Segmented control. */
  selected: string;
  /** Fill of the call-to-action button in a notice. */
  notice: string;
  noticeText: string;
  danger: string;
  play: string;
  /** The tension grid's lit cells. */
  tension: string;
  /** The base of the translucent controls floating over the roll (see `--overlay`). */
  overlayBase: string;
  shadow: string;
}

/** The note colors of one part: shaded from `min` (quiet) to `max` (loud) by velocity. */
export interface PartColors {
  min: number;
  max: number;
}

/** 0xRRGGBB colors of the Pixi piano roll. */
export interface RollColors {
  background: number;
  parts: { vocal: PartColors; bass: PartColors; other: PartColors };
  /** Guide lines at each C, and the tint behind white-key pitches. */
  guide: number;
  whiteRow: number;
  whiteRowAlpha: number;
  beatLine: number;
  barLine: number;
  chordBandA: number;
  chordBandB: number;
  chordBoundary: number;
  chordText: number;
  chordRoman: number;
  chordSilence: number;
  whiteKey: number;
  whiteKeyActive: number;
  blackKey: number;
  blackKeyActive: number;
  keyBorder: number;
  /** The plate behind the keyboard: the lighter gap beside the shorter black keys. */
  keyboardBacking: number;
  octaveLabel: number;
  /** Top to bottom, as DRUM_ROWS in pianoroll.ts. */
  drums: { cymbal: number; hi_hat: number; tom: number; snare: number; kick: number };
  drumRowLine: number;
  drumLane: number;
  drumLabel: number;
  scrollbarTrack: number;
  scrollbarThumb: number;
  scrollbarThumbActive: number;
  marker: number;
  selection: number;
  cursor: number;
}

export interface Theme {
  /** The browser's `color-scheme`: native controls, scrollbars. */
  scheme: "dark" | "light";
  ui: UiColors;
  roll: RollColors;
}

const DARK_UI: UiColors = {
  bg: "#14161c",
  surface: "#1b1e27",
  surfaceRaised: "#262b36",
  surfaceHover: "#313747",
  border: "#2a2e3a",
  borderStrong: "#3a3f4d",
  text: "#e8e8ec",
  textDim: "#8b93a7",
  textFaint: "#565d6f",
  accent: "#5b8cff",
  accentSoft: "#8fb0ff",
  selected: "#3d4a66",
  notice: "#2f5fa8",
  noticeText: "#ffffff",
  danger: "#e5484d",
  play: "#5ab0ff",
  tension: "#e3ab4f",
  overlayBase: "#14161c",
  shadow: "#000000",
};

const LIGHT_UI: UiColors = {
  bg: "#f3f4f7",
  surface: "#ffffff",
  surfaceRaised: "#e6e9f0",
  surfaceHover: "#dde1ea",
  border: "#d5d9e2",
  borderStrong: "#b9bfcc",
  text: "#1c1f27",
  textDim: "#5d6578",
  textFaint: "#9aa1b2",
  accent: "#3b6cf0",
  accentSoft: "#2f5fd0",
  selected: "#cdd8f5",
  notice: "#2f5fa8",
  noticeText: "#ffffff",
  danger: "#d12d33",
  play: "#1f78d1",
  tension: "#b97a12",
  overlayBase: "#f3f4f7",
  shadow: "#1c1f27",
};

export const THEMES: Record<ThemeId, Theme> = {
  dark: {
    scheme: "dark",
    ui: DARK_UI,
    roll: {
      background: 0x0c0d12,
      parts: {
        // other: quiet is close to the guide/background grey-blue, loud a near-white blue.
        other: { min: 0x2c3550, max: 0xaecbff },
        vocal: { min: 0x1f3d2c, max: 0x6be3a0 },
        bass: { min: 0x3d2c1f, max: 0xff9f5b },
      },
      guide: 0x262b36,
      whiteRow: 0xffffff,
      whiteRowAlpha: 0.08,
      beatLine: 0x5a6580,
      barLine: 0x9aaad8,
      chordBandA: 0x1c2136,
      chordBandB: 0x242b47,
      chordBoundary: 0x5a6580,
      chordText: 0xe8e8ec,
      chordRoman: 0x8b93a7,
      chordSilence: 0x555f78,
      whiteKey: 0xe8e8ec,
      whiteKeyActive: 0x5b8cff,
      blackKey: 0x1a1c22,
      blackKeyActive: 0x86a6ff,
      keyBorder: 0x0c0d12,
      keyboardBacking: 0x2a2e3a,
      octaveLabel: 0x3a3f4d,
      drums: {
        cymbal: 0xc9a7ff,
        hi_hat: 0x7fe0e8,
        tom: 0x8be08b,
        snare: 0xffe27a,
        kick: 0xff7d6e,
      },
      drumRowLine: 0x262b36,
      drumLane: 0x10121a,
      drumLabel: 0x8b93a7,
      scrollbarTrack: 0x1a1c22,
      scrollbarThumb: 0x5a6580,
      scrollbarThumbActive: 0x9aaad8,
      marker: 0xffd166,
      selection: 0x8fb0ff,
      cursor: 0xffffff,
    },
  },
  light: {
    scheme: "light",
    ui: LIGHT_UI,
    roll: {
      background: 0xf7f8fb,
      parts: {
        // On a light canvas quiet is the pale end and loud the saturated one.
        other: { min: 0xcfd9f1, max: 0x3a64c8 },
        vocal: { min: 0xc9e8d6, max: 0x1f9d5c },
        bass: { min: 0xf3dcc6, max: 0xd4690f },
      },
      guide: 0xd5d9e2,
      whiteRow: 0xffffff,
      whiteRowAlpha: 0.7,
      beatLine: 0x9aa1b2,
      barLine: 0x5d6578,
      chordBandA: 0xe3e8f5,
      chordBandB: 0xd3dcf0,
      chordBoundary: 0xaab3c8,
      chordText: 0x1c1f27,
      chordRoman: 0x5d6578,
      chordSilence: 0x9aa1b2,
      whiteKey: 0xffffff,
      whiteKeyActive: 0x7ea1ff,
      blackKey: 0x2a2e3a,
      blackKeyActive: 0x4a6fd8,
      keyBorder: 0x8b93a7,
      keyboardBacking: 0xb9bfcc,
      octaveLabel: 0x9aa1b2,
      drums: {
        cymbal: 0x8a5fd6,
        hi_hat: 0x0f9aa6,
        tom: 0x3e9f3e,
        snare: 0xc99a00,
        kick: 0xd9483a,
      },
      drumRowLine: 0xd5d9e2,
      drumLane: 0xe9ecf3,
      drumLabel: 0x5d6578,
      scrollbarTrack: 0xd5d9e2,
      scrollbarThumb: 0x8b93a7,
      scrollbarThumbActive: 0x5d6578,
      marker: 0xd99a00,
      selection: 0x3b6cf0,
      cursor: 0x1c1f27,
    },
  },
  // The pastel themes: no pure white anywhere, and notes softer than in "light".
  sky: {
    scheme: "light",
    ui: {
      bg: "#e1f0f9",
      surface: "#ecf6fc",
      surfaceRaised: "#cce3f3",
      surfaceHover: "#bbd8ee",
      border: "#c3daeb",
      borderStrong: "#a9c6dd",
      text: "#2b3a55",
      textDim: "#62748f",
      textFaint: "#98a7bc",
      accent: "#5dafd6",
      accentSoft: "#3e92bd",
      selected: "#bad4ee",
      notice: "#4a8db8",
      noticeText: "#ffffff",
      danger: "#d9575c",
      play: "#3e92bd",
      tension: "#c98a2b",
      overlayBase: "#e1f0f9",
      shadow: "#2b3a55",
    },
    roll: {
      background: 0xeaf5fb,
      parts: {
        other: { min: 0xdce7f6, max: 0xa3c2ea },
        vocal: { min: 0xd4ebdc, max: 0x94d2ac },
        bass: { min: 0xf8e2dc, max: 0xf5b8ab },
      },
      guide: 0xcbdfee,
      whiteRow: 0xffffff,
      whiteRowAlpha: 0.45,
      beatLine: 0x9fb4cc,
      barLine: 0x62748f,
      chordBandA: 0xd3e6f5,
      chordBandB: 0xc1dbf0,
      chordBoundary: 0x9fb8d0,
      chordText: 0x2b3a55,
      chordRoman: 0x62748f,
      chordSilence: 0x98a7bc,
      whiteKey: 0xeef7fc,
      whiteKeyActive: 0x7ec3e4,
      blackKey: 0x6f85a8,
      blackKeyActive: 0x4f9cc8,
      keyBorder: 0xa9c6dd,
      keyboardBacking: 0xb9d2e6,
      octaveLabel: 0x98a7bc,
      drums: {
        cymbal: 0xb9a3de,
        hi_hat: 0x7ccbd3,
        tom: 0x9cd39c,
        snare: 0xebcb6a,
        kick: 0xf09a8c,
      },
      drumRowLine: 0xcbdfee,
      drumLane: 0xdcebf6,
      drumLabel: 0x62748f,
      scrollbarTrack: 0xcbdfee,
      scrollbarThumb: 0x9fb4cc,
      scrollbarThumbActive: 0x62748f,
      marker: 0xe0a93a,
      selection: 0x4f9cc8,
      cursor: 0x2b3a55,
    },
  },
  rose: {
    scheme: "light",
    ui: {
      bg: "#fbe8f0",
      surface: "#fdf2f6",
      surfaceRaised: "#f3d2e1",
      surfaceHover: "#ebc2d5",
      border: "#ebcfdc",
      borderStrong: "#ddb3c8",
      text: "#4a3a4f",
      textDim: "#80708a",
      textFaint: "#b4a3bc",
      accent: "#d98bb0",
      accentSoft: "#c0709a",
      selected: "#ebd2ee",
      notice: "#b8639a",
      noticeText: "#ffffff",
      danger: "#d9575c",
      play: "#a46bc0",
      tension: "#c98a2b",
      overlayBase: "#fbe8f0",
      shadow: "#4a3a4f",
    },
    roll: {
      background: 0xfceff5,
      parts: {
        other: { min: 0xe9def2, max: 0xbda6d8 },
        vocal: { min: 0xdaede0, max: 0xa3d6b5 },
        bass: { min: 0xf9e1dc, max: 0xf7b5a9 },
      },
      guide: 0xedcfdc,
      whiteRow: 0xffffff,
      whiteRowAlpha: 0.45,
      beatLine: 0xc9aebe,
      barLine: 0x80708a,
      chordBandA: 0xf5dce8,
      chordBandB: 0xebd2ee,
      chordBoundary: 0xd5b6c8,
      chordText: 0x4a3a4f,
      chordRoman: 0x80708a,
      chordSilence: 0xb4a3bc,
      whiteKey: 0xfceef4,
      whiteKeyActive: 0xe39bbe,
      blackKey: 0x8c7396,
      blackKeyActive: 0xb66e9c,
      keyBorder: 0xddb3c8,
      keyboardBacking: 0xe8c6d6,
      octaveLabel: 0xb4a3bc,
      drums: {
        cymbal: 0xb9a3de,
        hi_hat: 0x7ccbd3,
        tom: 0x9cd39c,
        snare: 0xebcb6a,
        kick: 0xf09a8c,
      },
      drumRowLine: 0xedcfdc,
      drumLane: 0xf6dfe9,
      drumLabel: 0x80708a,
      scrollbarTrack: 0xedcfdc,
      scrollbarThumb: 0xc9aebe,
      scrollbarThumbActive: 0x80708a,
      marker: 0xe0a93a,
      selection: 0xb66e9c,
      cursor: 0x4a3a4f,
    },
  },
};

export function isThemeId(value: unknown): value is ThemeId {
  return THEME_IDS.includes(value as ThemeId);
}

/** Sets the theme's CSS custom properties on :root; the page restyles itself. The piano roll
 * takes `THEMES[id].roll` separately (PianoRoll.setColors). */
export function applyTheme(id: ThemeId): void {
  const theme = THEMES[id];
  const root = document.documentElement;
  for (const [name, value] of Object.entries(theme.ui)) {
    root.style.setProperty(`--${name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`, value);
  }
  // The legend and mixer name each part by its loudest note color on the roll.
  for (const [part, { max }] of Object.entries(theme.roll.parts)) {
    root.style.setProperty(`--part-${part}`, hex(max));
  }
  root.style.colorScheme = theme.scheme;
}

/** 0xRRGGBB as `#rrggbb`, for the places a Pixi color goes through CSS. */
function hex(color: number): string {
  return `#${color.toString(16).padStart(6, "0")}`;
}

/** Linearly blends two 0xRRGGBB colors by `t` in [0, 1]. */
export function lerpColor(from: number, to: number, t: number): number {
  const clamped = Math.min(1, Math.max(0, t));
  const channel = (shift: number) => {
    const a = (from >> shift) & 0xff;
    const b = (to >> shift) & 0xff;
    return Math.round(a + (b - a) * clamped);
  };
  return (channel(16) << 16) | (channel(8) << 8) | channel(0);
}
