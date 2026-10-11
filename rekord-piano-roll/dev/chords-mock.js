// Dev-only (see CLAUDE.md, "Chord layers: checking the lane"): stands in for Tauri's IPC so the real
// frontend runs in a plain browser (served by `npm run dev`) and replays the event log written by
// `chord_replay` (`REKORD_CHORD_EVENTS_OUT=dir`): notes, chord layers, active notes and key, each at
// the wall time it would have arrived. No beat lines, no audio.
//
// URL parameters:
//   events   JSONL file in dev/ (default chords.jsonl), lines {"wall": s, "event": PipelineEvent}
//   until    only replay the events up to this wall time (default: all)
//   model    note model size reported on Started (default mix: shows the 3-part legend)
//   stop     wall time at which to stop capture by itself (default: never; the Stop button works too)
const cbs = new Map();
const listeners = [];
let nextId = 1;
let timers = [];
const params = new URLSearchParams(location.search);
let lastWall = 0;

function emit(event, payload) {
  for (const l of listeners) if (l.event === event) cbs.get(l.handler)?.({ event, id: 0, payload });
}

async function startReplay() {
  const text = await (await fetch("/dev/" + (params.get("events") ?? "chords.jsonl"))).text();
  const until = Number(params.get("until") ?? Infinity);
  const entries = text
    .trim()
    .split("\n")
    .map((l) => JSON.parse(l))
    .filter((e) => e.wall <= until);
  lastWall = entries.length ? entries[entries.length - 1].wall : 0;
  window.__replay = { t0: performance.now(), entries };
  for (const e of entries) {
    if (e.event.type === "Started")
      e.event.note_model = {
        size: params.get("model") ?? "mix",
        device: "directml",
        future_frames: 8,
      };
    timers.push(setTimeout(() => emit("pipeline-event", e.event), e.wall * 1000));
  }
  if (params.get("stop"))
    timers.push(
      setTimeout(
        () => window.__TAURI_INTERNALS__.invoke("stop_capture"),
        Number(params.get("stop")) * 1000,
      ),
    );
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
      // what a real stop returns: the span of audio the backend recorded (none here, but a span lets the roll scroll)
      return window.__replay
        ? { sample_rate: 48000, start_s: 0, end_s: Math.max(lastWall - 0.4, 0) }
        : null;
    }
    // the debug range export: on, and the request is kept for inspection (window.__lastExport)
    if (cmd === "debug_export_enabled") return true;
    if (cmd === "debug_export_range") {
      window.__lastExport = args.range;
      return "C:/mock/export";
    }
    // MIDI out (a drag from the selection): the request is kept for inspection (window.__lastMidi)
    if (cmd === "drag_notes_as_midi") {
      window.__lastMidi = args.range;
      return null;
    }
    return null; // window size, playback and so on: no-ops
  },
};
