// Checks of the chord lane's composition (src/chordlane.ts) with layers that disagree about the
// chords, the cases of docs/chord-layers.md section 4.5. No test runner in this project: run
//   node rekord-piano-roll/dev/chordlane.test.ts
// (Node 22.18+ strips the types itself). Prints one line per scenario and exits non-zero on a failure.
import assert from "node:assert/strict";
import { ChordLane, planChordUpdate, type ChordLayerId, type LayerChord, type SegmentSpan } from "../src/chordlane.ts";
import { tensionsFor, type ChordVocabulary } from "../src/tensions.ts";

const BLOCK = 0.256;
const LAG: Record<ChordLayerId, number> = { instant: 0, settled: 0.55, confirmed: 2.0 };
const LAYERS: ChordLayerId[] = ["instant", "settled", "confirmed"];

const chord = (t: number, emit_s: number, label: string): LayerChord => ({
  t, emit_s, label, roman_numeral: null, root_pc: label === "(silence)" ? null : 0, quality: label === "(silence)" ? null : "maj",
});

type Opinion = (tau: number) => string;
const seq = (list: [number, string][]): Opinion => (tau) => {
  let r = list[0][1];
  for (const [t, l] of list) if (tau >= t) r = l;
  return r;
};
const truth = seq([[0, "C"], [2, "Am"], [4, "F"], [6, "G"], [8, "C"]]);

interface Drawn extends SegmentSpan {
  dying?: boolean;
}

/** Drives a lane block by block with layers that each decide `opinion[layer]` with their lag, and
 * counts the corrections the drawing would show. */
function simulate(opinion: Record<ChordLayerId, Opinion>, until = 11, layers: ChordLayerId[] = LAYERS) {
  const lane = new ChordLane();
  const lastLabel: Partial<Record<ChordLayerId, string>> = {};
  const lastDecided: Record<ChordLayerId, number> = { instant: 0, settled: 0, confirmed: 0 };
  let drawn: Drawn[] = [];
  let prevHead = -Infinity;
  let fadeOut = 0;
  let fadeIn = 0;
  let shortest = Infinity;
  const log: string[] = [];
  for (let c = BLOCK; c <= until + 1e-9; c += BLOCK) {
    for (const id of layers) {
      const decided = c - LAG[id];
      if (decided <= lastDecided[id]) continue;
      const changes: LayerChord[] = [];
      for (let tau = lastDecided[id] + 0.05; tau <= decided + 1e-9; tau += 0.05) {
        const label = opinion[id](tau);
        if (label !== lastLabel[id]) {
          changes.push(chord(+tau.toFixed(3), c, label));
          lastLabel[id] = label;
        }
      }
      lastDecided[id] = decided;
      lane.apply({ layer: id, changes, decided_s: decided });
    }
    const update = lane.takeUpdate();
    if (!update) continue;
    const plan = planChordUpdate(drawn, update, prevHead);
    // what is drawn afterwards: what ended before the update's start, then the update's segments
    const untouched = drawn.filter((d, i) => !d.dying && !(d.endS === null || d.endS > update.fromS + 1e-6) && !plan.removed.includes(i));
    const rebuilt: Drawn[] = update.segments.map((s) => ({ startS: s.startS, endS: s.endS, label: s.chord.label }));
    fadeOut += plan.removed.length;
    const corrections = plan.created.filter((x) => x.kind === "fade");
    fadeIn += corrections.length;
    if (plan.removed.length || corrections.length) {
      log.push(`c=${c.toFixed(2)} out: ${plan.removed.map((i) => drawn[i].label).join(" ") || "-"}  in: ${corrections.map((x) => update.segments[x.index].chord.label).join(" ") || "-"}`);
    }
    drawn = [...untouched, ...rebuilt];
    prevHead = update.headS;
    for (const s of lane.segments()) if (s.endS !== null && s.startS < c - 1) shortest = Math.min(shortest, s.endS - s.startS);
  }
  return { fadeOut, fadeIn, shortest, lane, log };
}

const clean = (extra: Partial<Record<ChordLayerId, Opinion>> = {}): Record<ChordLayerId, Opinion> => ({
  instant: (t) => truth(t - 0.15),
  settled: (t) => truth(t - 0.03),
  confirmed: (t) => truth(t),
  ...extra,
});

const scenarios: { name: string; opinion: Record<ChordLayerId, Opinion>; check: (r: ReturnType<typeof simulate>) => void }[] = [
  {
    name: "all layers agree, boundaries differ by up to 0.15 s",
    opinion: clean(),
    check: (r) => assert.deepEqual([r.fadeOut, r.fadeIn], [0, 0]),
  },
  {
    name: "instant blip (Dm for 0.12 s) removed by settled",
    opinion: clean({ instant: (t) => (t >= 3.0 && t < 3.12 ? "Dm" : truth(t - 0.15)) }),
    check: (r) => assert.deepEqual([r.fadeOut, r.fadeIn], [2, 0]),
  },
  {
    name: "settled adds a chord instant missed (Em)",
    opinion: clean({ settled: (t) => (t >= 3.0 && t < 3.5 ? "Em" : truth(t - 0.03)), confirmed: (t) => (t >= 3.0 && t < 3.5 ? "Em" : truth(t)) }),
    check: (r) => assert.deepEqual([r.fadeOut, r.fadeIn], [0, 2]),
  },
  {
    name: "instant mislabels the first 0.25 s of Am as C",
    opinion: clean({ instant: (t) => (t >= 2.0 && t < 2.25 ? "C" : truth(t - 0.05)) }),
    check: (r) => assert.deepEqual([r.fadeOut, r.fadeIn], [0, 0]),
  },
  {
    name: "settled splits F with a Dm blip, confirmed merges",
    opinion: clean({ settled: (t) => (t >= 4.8 && t < 5.2 ? "Dm" : truth(t - 0.03)) }),
    check: (r) => assert.deepEqual([r.fadeOut, r.fadeIn], [2, 2]),
  },
  {
    name: "settled calls F Dm for 2 s, confirmed corrects it",
    opinion: clean({ settled: (t) => (t >= 4 && t < 6 ? "Dm" : truth(t - 0.03)) }),
    check: (r) => assert.deepEqual([r.fadeOut, r.fadeIn], [2, 2]),
  },
  {
    name: "noisy instant (a blip every 0.7 s)",
    opinion: clean({ instant: (t) => (t % 0.7 < 0.1 && t > 0.5 ? "X" : truth(t - 0.15)) }),
    check: (r) => {
      // the blips come and go; a chord a blip happened to hide at its start may be added back once
      assert.ok(r.fadeIn <= 2, `${r.fadeIn}`);
      assert.ok(r.fadeOut >= 10, `${r.fadeOut}`);
    },
  },
  {
    name: "instant calls the whole of Am \"C\"",
    opinion: clean({ instant: (t) => (t >= 2.0 && t < 4.0 ? "C" : truth(t - 0.15)) }),
    check: (r) => assert.deepEqual([r.fadeOut, r.fadeIn], [0, 1]),
  },
];

let failed = 0;
for (const s of scenarios) {
  const result = simulate(s.opinion);
  try {
    s.check(result);
    // The finished lane is the confirmed layer's answer, whatever the faster ones said.
    const end = result.lane.segments().filter((x) => x.startS < 8);
    assert.deepEqual(end.map((x) => x.chord.label).filter((l) => ["Em", "Dm", "X"].includes(l)), s.name.includes("adds a chord") ? ["Em"] : []);
    console.log(`ok   ${s.name.padEnd(58)} fade-outs ${result.fadeOut}, fade-ins ${result.fadeIn}`);
  } catch (e) {
    failed++;
    console.log(`FAIL ${s.name}: ${(e as Error).message}`);
    console.log(result.log.join("\n"));
  }
}

// Only the instant and the settled layer (the second step of the roadmap): a chord stays what the instant
// layer said until the settled layer catches up, and what it says then wins.
{
  const r = simulate(clean({ settled: (t) => (t >= 4 ? "Dm" : truth(t - 0.03)) }), 11, ["instant", "settled"]);
  const at = (t: number) => r.lane.at(t)?.chord.label;
  assert.equal(at(1), "C");
  assert.equal(at(5), "Dm");
  assert.equal(at(Infinity), "Dm");
  console.log("ok   two layers only: settled's answer replaces instant's where it has decided");
}

// A respelled chord replaces the entry at the same time instead of adding a segment.
{
  const lane = new ChordLane();
  lane.apply({ layer: "instant", changes: [chord(1, 1.2, "A♯")], decided_s: 1.2 });
  lane.apply({ layer: "instant", changes: [chord(1, 3, "B♭")], decided_s: 3 });
  assert.deepEqual(lane.segments().map((s) => s.chord.label), ["B♭"]);
  console.log("ok   a change at the same time replaces the earlier one");
}

// takeUpdate: only what changed.
{
  const lane = new ChordLane();
  lane.apply({ layer: "instant", changes: [chord(0, 0.1, "C"), chord(2, 2.2, "Am")], decided_s: 2.5 });
  assert.equal(lane.takeUpdate()?.segments.length, 2);
  lane.apply({ layer: "instant", changes: [], decided_s: 3 });
  const u = lane.takeUpdate();
  assert.deepEqual(u?.segments.map((s) => s.chord.label), ["Am"]); // only the newest moved
  assert.equal(lane.takeUpdate(), null);
  lane.apply({ layer: "settled", changes: [chord(0, 0.6, "C"), chord(2.1, 2.7, "Am")], decided_s: 2.9 });
  assert.ok((lane.takeUpdate()?.fromS ?? 99) <= 0); // the settled layer's first decision may change anything from its start
  console.log("ok   takeUpdate returns only the changed tail");
}

// Tensions over the shown chord (the vocabulary comes from the backend; this is a slice of it).
{
  const vocabulary: ChordVocabulary = {
    templates: [{ quality: "maj", intervals: [0, 4, 7] }, { quality: "m7", intervals: [0, 3, 7, 10] }],
    tensions: [{ interval: 1, name: "b9" }, { interval: 2, name: "9" }, { interval: 5, name: "11" }, { interval: 9, name: "13" }],
  };
  const pcs = (...p: number[]) => p.reduce((m, x) => m | (1 << x), 0);
  assert.deepEqual(tensionsFor(vocabulary, pcs(0, 4, 7), 0, "maj"), []); // C E G over C
  assert.deepEqual(tensionsFor(vocabulary, pcs(0, 4, 7, 2), 0, "maj"), ["9"]); // + D
  assert.deepEqual(tensionsFor(vocabulary, pcs(0, 4, 7, 2, 9), 0, "maj"), ["9", "13"]); // + A: in the table's order
  assert.deepEqual(tensionsFor(vocabulary, pcs(9, 0, 4, 7, 2), 9, "m7"), ["11"]); // Am7 + D = 11; the C E G are its own
  assert.deepEqual(tensionsFor(vocabulary, pcs(2), 0, "sus9"), []); // a quality the table doesn't have
  console.log("ok   tensions over the shown chord");
}

process.exit(failed ? 1 : 0);
