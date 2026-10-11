// The tensions over the shown chord: the pitch classes sounding outside the chord's own template, at
// the seven offsets from the root the UI has a slot for -- the computation of
// rekord_core::chord_templates::extra_tensions, here on the frontend because the chord that is shown
// is the lane's (corrected afterwards), not any one layer's. The table it reads (the chord
// templates and the tension names) comes from the backend on `Started`, so it lives in one place.

export interface ChordVocabulary {
  templates: { quality: string; intervals: number[] }[];
  tensions: { interval: number; name: string }[];
}

const mod12 = (n: number) => ((n % 12) + 12) % 12;

/** The tensions (names, in the vocabulary's canonical order) of the pitch classes in `pcs` (bit `pc` set)
 * over the chord `rootPc` + `quality`. */
export function tensionsFor(
  vocabulary: ChordVocabulary,
  pcs: number,
  rootPc: number,
  quality: string,
): string[] {
  const template = vocabulary.templates.find((t) => t.quality === quality);
  if (!template) return [];
  let templateMask = 0;
  for (const interval of template.intervals) templateMask |= 1 << mod12(rootPc + interval);
  const extra = pcs & ~templateMask;
  return vocabulary.tensions
    .filter(({ interval }) => extra & (1 << mod12(rootPc + interval)))
    .map((t) => t.name);
}
