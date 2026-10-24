// How a chord quality is written (Cmaj7, CM7, C△7, ...). The backend names every chord with the
// canonical quality strings of rekord_core::chord_templates; this turns that name into the user's
// notation just before it is shown, so changing the setting also rewrites what is already on the
// piano roll. No Svelte or Pixi dependency; check with `node dev/chordnotation.test.ts`.

/** The qualities that can be written more than one way, each with the ways to choose from (the
 * default notation's first). Every other quality, and a plain major triad, is always written as the
 * backend names it. */
export const NOTATION_OPTIONS = {
  m: ["m", "−", "mi"],
  dim: ["dim", "°"],
  aug: ["aug", "+"],
  m6: ["m6", "−6", "mi6"],
  maj7: ["maj7", "M7", "△7", "Δ7"],
  m7: ["m7", "−7", "mi7"],
  m7b5: ["m7♭5", "m7(♭5)", "ø7", "ø"],
  dim7: ["dim7", "°7"],
  "sus4(7)": ["sus4(7)", "7sus4"],
  madd9: ["madd9", "m(add9)", "−(add9)"],
  maj9: ["maj9", "M9", "△9"],
  m9: ["m9", "−9", "mi9"],
  m11: ["m11", "−11", "mi11"],
  maj13: ["maj13", "M13", "△13"],
} as const satisfies Record<string, readonly string[]>;

export type NotationQuality = keyof typeof NOTATION_OPTIONS;

/** The qualities with a choice, in the order of the settings page. */
export const NOTATION_QUALITIES = Object.keys(NOTATION_OPTIONS) as NotationQuality[];

/** What the user has chosen: the default notation (the backend's names), or their own pick per quality
 * (a quality they have not picked for keeps the default). */
export interface Notation {
  custom: boolean;
  choices: Partial<Record<NotationQuality, string>>;
}

export const DEFAULT_NOTATION: Notation = { custom: false, choices: {} };

/** Whether `suffix` is one of the ways offered for `quality`. */
export function isNotationChoice(quality: NotationQuality, suffix: unknown): suffix is string {
  return (NOTATION_OPTIONS[quality] as readonly unknown[]).includes(suffix);
}

function suffixFor(notation: Notation, quality: string): string {
  if (quality === "maj") return "";
  if (!(quality in NOTATION_OPTIONS)) return quality;
  const options = NOTATION_OPTIONS[quality as NotationQuality];
  return (notation.custom && notation.choices[quality as NotationQuality]) || options[0];
}

// The root as the backend spells it: a letter and at most one accidental.
const ROOT = /^[A-G][♯♭]?/;

/** `label` ("C♯m7b5/G♯", as the backend names it) with its quality written per `notation`. The label
 * is left alone when it is not root + the canonical suffix of `quality` + an optional bass. */
export function formatChord(label: string, quality: string | null, notation: Notation): string {
  if (quality === null) return label;
  const root = ROOT.exec(label)?.[0];
  if (root === undefined) return label;
  const canonical = quality === "maj" ? "" : quality;
  const rest = label.slice(root.length);
  if (!rest.startsWith(canonical)) return label;
  return root + suffixFor(notation, quality) + rest.slice(canonical.length);
}

/** `roman` ("bVIm7", degree + canonical quality suffix) with its quality written per `notation`. */
export function formatRoman(roman: string, quality: string | null, notation: Notation): string {
  if (quality === null || quality === "maj" || !roman.endsWith(quality)) return roman;
  return roman.slice(0, roman.length - quality.length) + suffixFor(notation, quality);
}
