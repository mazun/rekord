// Dev-only (see CLAUDE.md, "Beat lines: checking timing and rendering"): stands in for
// Tauri's IPC so the real frontend runs in a plain browser (served by `npm run dev`), and
// replays a BeatLines log recorded by rust/core/examples/beat_replay.rs at the wall times it
// was produced. Only beat lines are replayed -- no notes, chords or key.
//
// URL parameters (paths relative to /dev/):
//   events   JSONL from beat_replay (default replay-events.jsonl)
//   ref      optional JSON {beats: [...], downbeats: [...]}: reference beats, drawn as
//            ticks along the top/bottom edges (downbeats yellow and taller)
//   latency  total_latency_s sent on Started (default 1.3 = the app's default 250 ms setting)
//   model    note model size reported on Started (default large; "mix" shows the 3-part legend)
const cbs = new Map();
const listeners = [];
let nextId = 1;
let timers = [];
const params = new URLSearchParams(location.search);
const LATENCY = Number(params.get("latency") ?? 1.3);

function emit(event, payload) {
  for (const l of listeners) if (l.event === event) cbs.get(l.handler)?.({ event, id: 0, payload });
}

const lines = (v) => v.map(([t, b]) => ({ t, bar: !!b }));

async function startReplay() {
  const text = await (await fetch(`/dev/${params.get("events") ?? "replay-events.jsonl"}`)).text();
  const events = text
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l));
  const ref = params.get("ref") ? await (await fetch(`/dev/${params.get("ref")}`)).json() : null;
  window.__replay = { t0: performance.now(), events, ref };
  emit("pipeline-event", {
    type: "Started",
    total_latency_s: LATENCY,
    note_model: { size: params.get("model") ?? "large", device: "directml", future_frames: 8 },
  });
  for (const e of events) {
    timers.push(
      setTimeout(
        () =>
          emit("pipeline-event", {
            type: "BeatLines",
            from_s: e.from_s,
            committed: lines(e.committed),
            provisional: lines(e.provisional),
            predicted: lines(e.predicted),
          }),
        e.wall * 1000,
      ),
    );
  }
  if (ref) requestAnimationFrame(drawRef);
}

function drawRef() {
  const host = document.getElementById("pianoroll-container");
  let c = document.getElementById("replay-ref");
  if (!c) {
    c = document.createElement("canvas");
    c.id = "replay-ref";
    c.style.cssText = "position:absolute;inset:0;pointer-events:none;z-index:2";
    host.appendChild(c);
  }
  c.width = host.clientWidth;
  c.height = host.clientHeight;
  const g = c.getContext("2d");
  const { t0, ref } = window.__replay;
  const disp = (performance.now() - t0) / 1000 - LATENCY;
  const playheadX = c.width - 34; // pianoroll.ts KEYBOARD_WIDTH
  const down = new Set(ref.downbeats.map((x) => x.toFixed(3)));
  for (const t of ref.beats) {
    const x = playheadX + (t - disp) * 120; // pianoroll.ts PIXELS_PER_SECOND
    if (x < 0 || x > playheadX) continue;
    const h = down.has(t.toFixed(3)) ? 14 : 8;
    g.fillStyle = h === 14 ? "#ffd166" : "#e07a5f";
    g.fillRect(x - 1, 0, 3, h);
    g.fillRect(x - 1, c.height - h, 3, h);
  }
  g.fillStyle = "#fff";
  g.font = "12px monospace";
  g.fillText(`t=${disp.toFixed(2)}s`, 8, c.height - 20);
  requestAnimationFrame(drawRef);
}

window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
window.__TAURI_INTERNALS__ = {
  metadata: {
    currentWindow: { label: "main" },
    currentWebview: { label: "main", windowLabel: "main" },
  },
  transformCallback(cb) {
    const id = nextId++;
    cbs.set(id, cb);
    return id;
  },
  unregisterCallback(id) {
    cbs.delete(id);
  },
  convertFileSrc: (x) => x,
  async invoke(cmd, args) {
    if (cmd === "plugin:event|listen") {
      listeners.push({ event: args.event, handler: args.handler });
      return listeners.length;
    }
    if (cmd === "start_capture") {
      startReplay();
      return null;
    }
    if (cmd === "stop_capture") {
      timers.forEach(clearTimeout);
      timers = [];
      return null;
    }
    return null; // window size etc.: no-ops
  },
};
