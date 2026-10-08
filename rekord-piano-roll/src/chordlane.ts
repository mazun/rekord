// The chord lane: what the piano roll's chord strip and the top readout show, composed from the
// chord layers (docs/chord-layers.md). No Svelte, no Pixi: a plain model fed with the backend's
// `ChordLayer` events.
//
// Each layer (instant, settled, ...) reports where its chord changes and how far it has decided
// (`decided_s`), and never revises what it reported. The lane takes, of each layer, the changes
// inside the span only that layer has decided -- the slowest layer up to its `decided_s`, the next
// from there up to its own, the instant layer from there on -- and applies them in time order. So
// a later layer corrects an earlier one exactly where it has caught up, and where the layers
// agree nothing changes. (Composing by "the state of the slowest layer that has decided time t"
// instead leaves slivers and false changes where one layer hands over to the next: a layer that
// puts the same change 0.1 s earlier shows the old chord, then the new one, then the old one again.)

export type ChordLayerId = "instant" | "settled" | "confirmed";

/** The layers in increasing order of delay. */
export const LAYER_ORDER: ChordLayerId[] = ["instant", "settled", "confirmed"];

/** A chord a layer decided, as the backend sends it (`LayerChord`). */
export interface LayerChord {
  t: number;
  emit_s: number;
  label: string;
  roman_numeral: string | null;
  /** null for silence */
  root_pc: number | null;
  quality: string | null;
}

export interface ChordLayerEvent {
  layer: ChordLayerId;
  changes: LayerChord[];
  decided_s: number;
}

/** What the lane shows over one stretch of time. */
export interface LaneSegment {
  startS: number;
  /** null: the newest segment, which runs up to the present */
  endS: number | null;
  chord: LayerChord;
  /** The index in `LAYER_ORDER` of the least-settled layer this segment is made of. For debugging. */
  layer: number;
}

/** What changed in the lane since the last `takeUpdate`: from `fromS` on, the segments are these. */
export interface LaneUpdate {
  fromS: number;
  segments: LaneSegment[];
  /** The newest time the lane has chords for (the instant layer's `decided_s`). */
  headS: number;
}

interface LayerLog {
  entries: LayerChord[];
  decidedS: number;
}

const EPS = 1e-6;

// A layer that is waiting on a challenger (the hold) reports "decided" up to the earliest moment
// that challenger's change could start, but has not reported the change itself yet -- while a faster
// layer may already show it, a few tens of milliseconds earlier. Taking the faster layer's changes
// only after the slower layer's frontier would drop that change for the whole of the hold and bring
// it back when the slower layer commits. So the faster layer's changes just before the frontier stay
// (those later than anything the slower layer reported), and give way to the slower layer's own as
// soon as it reports them. About the largest gap between two layers' times for the same change.
const HANDOVER_GRACE_S = 0.25;

export class ChordLane {
  private logs: Record<ChordLayerId, LayerLog> = {
    instant: { entries: [], decidedS: -Infinity },
    settled: { entries: [], decidedS: -Infinity },
    confirmed: { entries: [], decidedS: -Infinity },
  };
  private cache: LaneSegment[] | null = null;
  /** The earliest time whose lane content may differ from what `takeUpdate` last returned. */
  private dirtyFromS = Infinity;
  private sentHeadS = -Infinity;

  /** Takes one layer's new decisions. A change at the same time as the layer's latest replaces it
   * (the chord respelled after the sharp/flat setting changed). */
  apply(event: ChordLayerEvent): void {
    const log = this.logs[event.layer];
    for (const c of event.changes) {
      const last = log.entries[log.entries.length - 1];
      if (last && Math.abs(last.t - c.t) < EPS) log.entries[log.entries.length - 1] = c;
      else log.entries.push(last && c.t < last.t ? { ...c, t: last.t } : c);
      this.dirtyFromS = Math.min(this.dirtyFromS, c.t);
    }
    // The span the layer has newly decided now shows its changes instead of a faster layer's.
    if (event.decided_s > log.decidedS) {
      this.dirtyFromS = Math.min(this.dirtyFromS, log.decidedS);
      log.decidedS = event.decided_s;
    }
    this.cache = null;
  }

  /** Forgets what ended before `t` (keeping what is in effect at `t`). */
  pruneBefore(t: number): void {
    for (const log of Object.values(this.logs)) {
      let keepFrom = 0;
      while (keepFrom + 1 < log.entries.length && log.entries[keepFrom + 1].t < t) keepFrom++;
      if (keepFrom > 0) log.entries.splice(0, keepFrom);
    }
    this.cache = null;
  }

  clear(): void {
    for (const log of Object.values(this.logs)) {
      log.entries = [];
      log.decidedS = -Infinity;
    }
    this.cache = null;
    this.dirtyFromS = Infinity;
    this.sentHeadS = -Infinity;
  }

  /** The newest time any layer has decided. */
  get headS(): number {
    return this.logs.instant.decidedS;
  }

  /** The whole lane, oldest first. */
  segments(): LaneSegment[] {
    if (this.cache) return this.cache;
    const decided = (id: ChordLayerId) => this.logs[id].decidedS;
    // The span each layer owns: confirmed up to its decided_s, settled from there up to its own,
    // instant from there on (a layer that has decided less than a slower one owns nothing).
    const d2 = decided("confirmed");
    const d1 = Math.max(d2, decided("settled"));
    const picked: { chord: LayerChord; layer: number }[] = [];
    const take = (id: ChordLayerId, after: number, upTo: number, laterThan = -Infinity) => {
      const layer = LAYER_ORDER.indexOf(id);
      for (const chord of this.logs[id].entries) {
        if (chord.t > after && chord.t <= upTo && chord.t > laterThan) picked.push({ chord, layer });
      }
    };
    const lastTime = (id: ChordLayerId) => this.logs[id].entries[this.logs[id].entries.length - 1]?.t ?? -Infinity;
    take("confirmed", -Infinity, d2);
    take("settled", d2, d1);
    take("settled", d2 - HANDOVER_GRACE_S, d2, lastTime("confirmed"));
    take("instant", d1, Infinity);
    take("instant", d1 - HANDOVER_GRACE_S, d1, lastTime("settled"));
    // (a layer's changes are in time order; across layers the grace windows interleave them)
    picked.sort((x, y) => x.chord.t - y.chord.t || x.layer - y.layer);
    const out: LaneSegment[] = [];
    for (const { chord, layer } of picked) {
      const prev = out[out.length - 1];
      if (prev && prev.chord.label === chord.label) {
        prev.layer = Math.min(prev.layer, layer);
        continue;
      }
      if (prev) prev.endS = chord.t;
      out.push({ startS: chord.t, endS: null, chord, layer });
    }
    this.cache = out;
    return out;
  }

  /** What changed since the last call (null: nothing): from `fromS` on, the lane is `segments`. */
  takeUpdate(): LaneUpdate | null {
    const all = this.segments();
    const headS = this.headS;
    if (this.dirtyFromS === Infinity && headS === this.sentHeadS) return null;
    // From the segment before the one running at the earliest changed time (its end may have
    // changed, e.g. it was the newest and a new chord has begun); if just the head moved, the
    // newest one and the one before it.
    let from = Math.max(all.length - 1, 0);
    while (from > 0 && all[from].startS > this.dirtyFromS + EPS) from--;
    from = Math.max(from - 1, 0);
    const update: LaneUpdate = { fromS: all[from]?.startS ?? -Infinity, segments: all.slice(from), headS };
    this.dirtyFromS = Infinity;
    this.sentHeadS = headS;
    return update;
  }

  /** The lane's segment at time `t` (`Infinity`: the newest), or undefined before the first chord. */
  at(t: number): LaneSegment | undefined {
    const all = this.segments();
    if (all.length === 0) return undefined;
    if (t === Infinity) return all[all.length - 1];
    let lo = 0;
    let hi = all.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (all[mid].startS <= t) lo = mid + 1;
      else hi = mid;
    }
    return lo > 0 ? all[lo - 1] : undefined;
  }
}

/** A segment as far as the pairing of old and new ones is concerned. */
export interface SegmentSpan {
  startS: number;
  endS: number | null;
  label: string;
}

// A segment of the instant layer that starts no earlier than this before the newest time the lane
// knew of is the progression moving on, not a correction.
const HEAD_APPEARANCE_TOLERANCE_S = 0.4;

export interface ChordUpdatePlan {
  /** [index in `drawn`, index in `update.segments`]: the same chord, which keeps its drawing and
   * only slides to its new span. */
  matched: [number, number][];
  /** Indices in `update.segments` with nothing to continue: `fade` ones are corrections (a chord
   * that was not there before), `head` ones are just the progression moving on. */
  created: { index: number; kind: "fade" | "head" }[];
  /** Indices in `drawn` that nothing continues: a chord the correction took away. */
  removed: number[];
}

/** Decides what an update does to what is drawn: pairs each old segment with the new segment of the
 * same label it overlaps most (greedily, one to one), so a chord that merely moved slides and only
 * the chords that really appeared or disappeared fade. Nothing else is needed to handle layers
 * that disagree about how many chords there are: a blip removed, a chord added, two merged or
 * split are all just unpaired segments. `drawn` is every segment currently drawn (the dying
 * ones marked, they are ignored); `prevHeadS` the head of the previous update. */
export function planChordUpdate(
  drawn: (SegmentSpan & { dying?: boolean })[],
  update: LaneUpdate,
  prevHeadS: number,
): ChordUpdatePlan {
  const end = (s: SegmentSpan) => s.endS ?? Math.max(update.headS, s.startS + 1);
  const oldIdx: number[] = [];
  drawn.forEach((s, i) => {
    if (!s.dying && (s.endS === null || s.endS > update.fromS + EPS)) oldIdx.push(i);
  });
  const pairs: { o: number; n: number; overlap: number }[] = [];
  for (const o of oldIdx) {
    update.segments.forEach((n, j) => {
      if (drawn[o].label !== n.chord.label) return;
      const overlap = Math.min(end(drawn[o]), end({ startS: n.startS, endS: n.endS, label: "" })) - Math.max(drawn[o].startS, n.startS);
      if (overlap > 0) pairs.push({ o, n: j, overlap });
    });
  }
  pairs.sort((a, b) => b.overlap - a.overlap);
  const usedOld = new Set<number>();
  const usedNew = new Set<number>();
  const matched: [number, number][] = [];
  for (const p of pairs) {
    if (usedOld.has(p.o) || usedNew.has(p.n)) continue;
    usedOld.add(p.o);
    usedNew.add(p.n);
    matched.push([p.o, p.n]);
  }
  const created = update.segments
    .map((s, index) => ({
      index,
      kind: (s.layer === 0 && s.startS >= prevHeadS - HEAD_APPEARANCE_TOLERANCE_S ? "head" : "fade") as "fade" | "head",
    }))
    .filter((c) => !usedNew.has(c.index));
  return { matched, created, removed: oldIdx.filter((i) => !usedOld.has(i)) };
}
