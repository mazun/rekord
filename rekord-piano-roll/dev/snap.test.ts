// Checks of the selection's snapping to the beat grid (src/snap.ts) against the plain scan over every
// beat it replaced. No test runner in this project: run
//   node rekord-piano-roll/dev/snap.test.ts
// (Node 22.18+ strips the types itself). Exits non-zero on a failure.
import assert from "node:assert/strict";
import { SNAP_MAX_BEAT_GAP_S, snapToBeats } from "../src/snap.ts";

/** The snapping as it was written inside the piano roll: a scan from the first beat. */
function reference(beats: number[], t: number): number {
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
    if (i + 1 < beats.length && beats[i + 1] - beats[i] <= SNAP_MAX_BEAT_GAP_S)
      consider((beats[i] + beats[i + 1]) / 2);
  }
  return best;
}

let seed = 42;
const random = () => {
  seed = (seed * 1664525 + 1013904223) >>> 0;
  return seed / 2 ** 32;
};

// steady beats with jitter, a gap in the middle, and a duplicate
const beats: number[] = [];
for (let t = 1, i = 0; i < 80; i++) {
  beats.push(t);
  t += i === 40 ? 3.5 : 0.45 + random() * 0.1;
}
beats.splice(10, 0, beats[10]);

let checked = 0;
for (let t = -1; t < beats[beats.length - 1] + 2; t += 0.0137) {
  assert.equal(snapToBeats(beats, t), reference(beats, t), `t ${t}`);
  checked++;
}
// exactly on beats, on midpoints, and halfway between a beat and a midpoint (ties)
for (let i = 0; i + 1 < beats.length; i++) {
  for (const t of [
    beats[i],
    (beats[i] + beats[i + 1]) / 2,
    (3 * beats[i] + beats[i + 1]) / 4,
    (beats[i] + 3 * beats[i + 1]) / 4,
  ]) {
    assert.equal(snapToBeats(beats, t), reference(beats, t), `t ${t}`);
    checked++;
  }
}
console.log(`ok   snapping matches the full scan at ${checked} times`);

assert.equal(snapToBeats([], 3), 3);
assert.equal(snapToBeats([2], 2.1), 2.1);
assert.equal(snapToBeats([1, 2], 0.5), 0.5, "before the first beat");
assert.equal(snapToBeats([1, 2], 2.5), 2.5, "after the last beat");
assert.equal(
  snapToBeats([1, 2, 10, 11], 5),
  2,
  "inside a gap: the nearest beat in reach, no midpoint across the gap",
);
assert.equal(
  snapToBeats([1, 2, 10, 11], 8),
  10,
  "a beat within reach after t wins over a far one before it",
);
console.log("ok   edges: no beats, outside the beats, across a gap");
