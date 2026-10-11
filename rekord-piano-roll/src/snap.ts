/** Beats further apart than this (a gap in the detected beats) have no eighth-note grid between them. */
export const SNAP_MAX_BEAT_GAP_S = 2;

/** `t` snapped to the nearest eighth note of `beats` (sorted): a beat or the middle between two
 * neighbouring ones. Not snapped outside the beats, or across a gap in them. */
export function snapToBeats(beats: readonly number[], t: number): number {
  if (beats.length < 2 || t < beats[0] || t > beats[beats.length - 1]) return t;
  let best = t;
  let bestD = Infinity;
  const consider = (c: number) => {
    const d = Math.abs(c - t);
    if (d < bestD) [best, bestD] = [c, d];
  };
  // Nothing before the last beat that is not after `t` can be nearer than that beat itself.
  let lo = 0;
  for (let hi = beats.length; lo < hi; ) {
    const mid = (lo + hi) >> 1;
    if (beats[mid] < t) lo = mid + 1;
    else hi = mid;
  }
  for (let i = Math.max(0, lo - 1); i < beats.length; i++) {
    if (beats[i] > t + SNAP_MAX_BEAT_GAP_S) break;
    consider(beats[i]);
    if (i + 1 < beats.length && beats[i + 1] - beats[i] <= SNAP_MAX_BEAT_GAP_S)
      consider((beats[i] + beats[i + 1]) / 2);
  }
  return best;
}
