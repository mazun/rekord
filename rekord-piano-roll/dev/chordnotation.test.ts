// Checks of the chord notation setting (src/chordnotation.ts). No test runner in this project: run
//   node rekord-piano-roll/dev/chordnotation.test.ts
// (Node 22.18+ strips the types itself). Exits non-zero on a failure.
import assert from "node:assert/strict";
import {
  DEFAULT_NOTATION,
  formatChord,
  formatRoman,
  isNotationChoice,
  NOTATION_OPTIONS,
  NOTATION_QUALITIES,
  type Notation,
} from "../src/chordnotation.ts";

const custom: Notation = {
  custom: true,
  choices: { maj7: "△7", m7b5: "ø7", dim: "°", aug: "+", m7: "−7" },
};

// The default notation writes each quality as its first option (the backend's name, but for ♭), and
// so does custom with nothing picked.
const untouched: Notation = { custom: true, choices: {} };
for (const q of NOTATION_QUALITIES) {
  const first = NOTATION_OPTIONS[q][0];
  for (const n of [DEFAULT_NOTATION, untouched]) {
    assert.equal(formatChord(`C♯${q}/G♯`, q, n), `C♯${first}/G♯`);
    assert.equal(formatRoman(`bVI${q}`, q, n), `bVI${first}`);
  }
}
assert.equal(formatChord("Dm7b5", "m7b5", DEFAULT_NOTATION), "Dm7♭5");

// Picks apply only while custom is on.
assert.equal(formatChord("Cmaj7", "maj7", custom), "C△7");
assert.equal(formatChord("Cmaj7", "maj7", { ...custom, custom: false }), "Cmaj7");
assert.equal(formatChord("B♭m7b5/E", "m7b5", custom), "B♭ø7/E");
assert.equal(formatChord("F♯dim", "dim", custom), "F♯°");
assert.equal(formatChord("Caug", "aug", custom), "C+");
assert.equal(formatChord("Am7", "m7", custom), "A−7");
assert.equal(formatChord("Dm9", "m9", custom), "Dm9");

// Qualities without a choice, major triads and slash chords.
assert.equal(formatChord("C7", "7", custom), "C7");
assert.equal(formatChord("C", "maj", custom), "C");
assert.equal(formatChord("C/E", "maj", custom), "C/E");

// Silence and unparsable names pass through.
assert.equal(formatChord("--", null, custom), "--");
assert.equal(formatChord("--", "maj7", custom), "--");
assert.equal(formatChord("Cm7", "maj7", custom), "Cm7");

// Roman numerals carry the same suffix.
assert.equal(formatRoman("Imaj7", "maj7", custom), "I△7");
assert.equal(formatRoman("bVIIm7b5", "m7b5", custom), "bVIIø7");
assert.equal(formatRoman("V", "maj", custom), "V");
assert.equal(formatRoman("IV7", "7", custom), "IV7");
assert.equal(formatRoman("II", null, custom), "II");

assert.ok(isNotationChoice("maj7", "M7"));
assert.ok(!isNotationChoice("maj7", "x"));

console.log("chordnotation: ok");
