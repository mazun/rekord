import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { PianoRoll } from "./pianoroll";
import pianorollToggleIconSvg from "./assets/pianoroll-toggle-icon.svg?raw";

type PipelineEvent =
  | { type: "Started"; total_latency_s: number }
  | {
      type: "NoteConfirmed";
      start_s: number;
      end_s: number;
      pitch_midi: number;
      velocity: number;
      melody: boolean;
      bass: boolean;
    }
  | { type: "ChordChange"; t: number; label: string; roman_numeral: string | null }
  | { type: "TensionsChanged"; tensions: string[] }
  | { type: "KeyChange"; key_name: string };

const LATENCY_STORAGE_KEY = "chord.latencyMs";

// Mirrors chord_core::chord_templates::NOTE_NAMES and
// KeyEstimate::name()'s "X Major (Y Minor)" formatting -- the manual-key
// dropdown offers the same 12 relative major/minor pairs the automatic
// estimator can ever display, so switching to a manual pick never shows a
// label the Auto mode couldn't have shown itself.
const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
function keyPairLabel(relativeMajorPc: number): string {
  const minorPc = (relativeMajorPc + 9) % 12; // -3 semitones, wrapped
  return `${NOTE_NAMES[relativeMajorPc]} Major (${NOTE_NAMES[minorPc]} Minor)`;
}

window.addEventListener("DOMContentLoaded", async () => {
  const container = document.querySelector<HTMLDivElement>("#pianoroll-container");
  const keyLabel = document.querySelector<HTMLDivElement>("#key-label");
  const chordLabel = document.querySelector<HTMLDivElement>("#chord-label");
  const romanLabel = document.querySelector<HTMLDivElement>("#roman-label");
  const startBtn = document.querySelector<HTMLButtonElement>("#start-btn");
  const stopBtn = document.querySelector<HTMLButtonElement>("#stop-btn");
  const latencyInput = document.querySelector<HTMLInputElement>("#latency-input");
  const latencyValue = document.querySelector<HTMLSpanElement>("#latency-value");
  const keyMenu = document.querySelector<HTMLDivElement>("#key-menu");
  const rolesToggle = document.querySelector<HTMLButtonElement>("#roles-toggle");
  const mainView = document.querySelector<HTMLDivElement>("#main-view");
  const settingsView = document.querySelector<HTMLDivElement>("#settings-view");
  const settingsBtn = document.querySelector<HTMLButtonElement>("#settings-btn");
  const settingsBackBtn = document.querySelector<HTMLButtonElement>("#settings-back-btn");
  const pianorollToggleBtn = document.querySelector<HTMLButtonElement>("#pianoroll-toggle-btn");
  const appShell = document.querySelector<HTMLDivElement>(".app");
  if (
    !container ||
    !keyLabel ||
    !chordLabel ||
    !romanLabel ||
    !startBtn ||
    !stopBtn ||
    !latencyInput ||
    !latencyValue ||
    !keyMenu ||
    !rolesToggle ||
    !mainView ||
    !settingsView ||
    !settingsBtn ||
    !settingsBackBtn ||
    !pianorollToggleBtn ||
    !appShell
  )
    return;

  // Injected as raw markup (via Vite's ?raw import) rather than an <img> --
  // an <img> can't be recolored with currentColor, and this icon needs to
  // dim/brighten on hover and turn active-blue when the piano roll is
  // hidden, same as every other icon button in this toolbar.
  pianorollToggleBtn.innerHTML = pianorollToggleIconSvg;

  // Stopping capture (if running) before navigating away keeps the piano
  // roll's clock/state consistent with what "Start" will set up fresh next
  // time -- the settings page is a separate view, not an overlay on top of
  // a still-running session.
  const stopCapture = async () => {
    stopBtn.disabled = true;
    try {
      await invoke("stop_capture");
    } finally {
      startBtn.disabled = false;
      latencyInput.disabled = false;
    }
  };

  settingsBtn.addEventListener("click", async () => {
    if (!stopBtn.disabled) {
      await stopCapture();
    }
    mainView.classList.add("hidden");
    settingsView.classList.remove("hidden");
  });
  settingsBackBtn.addEventListener("click", () => {
    settingsView.classList.add("hidden");
    mainView.classList.remove("hidden");
  });

  // The piano roll is the only reason this window needs to be wide or
  // tall -- the toolbar/key/chord rows above it are happy at their
  // natural content size, and #main-view's flex:1 would otherwise just
  // stretch to fill whatever leftover height the window still has once
  // the piano roll itself is display:none, leaving a pointless blank
  // strip. Hiding it shrinks the OS window down to that natural size
  // (measured live via a temporary shrink-to-fit layout pass, not a
  // hardcoded guess, since it depends on font metrics and the current
  // chord/tension labels); showing it again restores whatever size the
  // window was at before it was hidden, falling back to the
  // tauri.conf.json defaults only on the very first collapse.
  const DEFAULT_EXPANDED_WIDTH = 800;
  const DEFAULT_EXPANDED_HEIGHT = 600;
  const PIANOROLL_VISIBLE_STORAGE_KEY = "chord.pianorollVisible";
  // Persisted like the latency slider above -- session-only convenience,
  // not correctness-critical, so a missing/garbled value just falls back
  // to the shipped default (visible) rather than needing validation.
  const storedPianorollVisible = localStorage.getItem(PIANOROLL_VISIBLE_STORAGE_KEY);
  let pianorollVisible = storedPianorollVisible !== "false";
  let widthBeforeCollapse: number | null = null;
  let heightBeforeCollapse: number | null = null;

  const measureCollapsedSize = (): { width: number; height: number } => {
    appShell.classList.add("measuring");
    const rect = appShell.getBoundingClientRect();
    // The toggle button is position:absolute (so it can hang off the
    // keyboard strip's would-be position even once that strip is gone),
    // which means it's out-of-flow and never counted in this shrink-to-fit
    // measurement -- without adding its own footprint back in, the
    // collapsed window comes out just wide enough for the tension grid
    // alone, and the button ends up drawn on top of it instead of past
    // its right edge.
    const btnWidth = pianorollToggleBtn.getBoundingClientRect().width;
    const size = { width: Math.ceil(rect.width + btnWidth), height: Math.ceil(rect.height) };
    appShell.classList.remove("measuring");
    return size;
  };

  // Shared by the click handler and the startup replay below, so opening
  // the app in whichever state it was last left in resizes the window the
  // same way an actual click would, rather than duplicating this logic.
  const setPianorollVisible = async (visible: boolean) => {
    pianorollVisible = visible;
    container.classList.toggle("hidden", !visible);
    pianorollToggleBtn.classList.toggle("active", !visible);
    pianorollToggleBtn.setAttribute("aria-pressed", String(!visible));
    localStorage.setItem(PIANOROLL_VISIBLE_STORAGE_KEY, String(visible));

    try {
      const win = getCurrentWindow();
      const scaleFactor = await win.scaleFactor();
      const currentSize = await win.innerSize();
      const currentLogicalWidth = currentSize.width / scaleFactor;
      const currentLogicalHeight = currentSize.height / scaleFactor;

      if (!visible) {
        widthBeforeCollapse = currentLogicalWidth;
        heightBeforeCollapse = currentLogicalHeight;
        const collapsed = measureCollapsedSize();
        await win.setSize(new LogicalSize(collapsed.width, collapsed.height));
      } else {
        const restoredWidth = widthBeforeCollapse ?? DEFAULT_EXPANDED_WIDTH;
        const restoredHeight = heightBeforeCollapse ?? DEFAULT_EXPANDED_HEIGHT;
        await win.setSize(new LogicalSize(restoredWidth, restoredHeight));
      }
    } catch (err) {
      console.error(err);
    }
  };

  pianorollToggleBtn.addEventListener("click", () => {
    setPianorollVisible(!pianorollVisible);
  });

  // Reproduce a collapsed state from a previous session -- the window
  // itself always starts at tauri.conf.json's expanded default (800x600),
  // so this both shrinks it down to match and captures that starting size
  // as what "show" should restore to later, exactly like a real collapse
  // click would from here.
  if (!pianorollVisible) {
    setPianorollVisible(false);
  }

  // Clicking the key label opens a small popup menu (Auto + the 12
  // relative major/minor pairs) instead of a permanently-visible
  // dropdown -- the picker is only on screen while actually choosing.
  const keyMenuItems = new Map<string, HTMLButtonElement>();
  const addKeyMenuItem = (value: string, label: string) => {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "key-menu-item";
    item.role = "option";
    item.dataset.tonic = value;
    item.textContent = label;
    keyMenu.appendChild(item);
    keyMenuItems.set(value, item);
  };
  addKeyMenuItem("auto", "Auto");
  for (let pc = 0; pc < 12; pc++) {
    addKeyMenuItem(String(pc), keyPairLabel(pc));
  }

  const closeKeyMenu = () => {
    keyMenu.classList.add("hidden");
    keyLabel.setAttribute("aria-expanded", "false");
  };
  const openKeyMenu = () => {
    keyMenu.classList.remove("hidden");
    keyLabel.setAttribute("aria-expanded", "true");
  };

  keyLabel.addEventListener("click", (e) => {
    e.stopPropagation();
    if (keyMenu.classList.contains("hidden")) {
      openKeyMenu();
    } else {
      closeKeyMenu();
    }
  });
  keyLabel.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      openKeyMenu();
    }
  });
  document.addEventListener("click", (e) => {
    if (!keyMenu.contains(e.target as Node) && e.target !== keyLabel) {
      closeKeyMenu();
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeKeyMenu();
  });

  keyMenu.addEventListener("click", async (e) => {
    const item = (e.target as HTMLElement).closest<HTMLButtonElement>(".key-menu-item");
    if (!item || !item.dataset.tonic) return;
    const value = item.dataset.tonic;
    const tonicPc = value === "auto" ? null : Number(value);
    keyMenuItems.forEach((el, key) => el.classList.toggle("selected", key === value));
    closeKeyMenu();
    // Auto mode's label only refreshes on the next KeyChange event (i.e.
    // once capture is running and a checkpoint fires); a manual pick's
    // label is already fully known, so show it immediately instead of
    // waiting on that same round-trip.
    if (tonicPc !== null) {
      keyLabel.textContent = keyPairLabel(tonicPc);
    }
    try {
      await invoke("set_key_mode", { tonicPc });
    } catch (err) {
      console.error(err);
    }
  });
  keyMenuItems.get("auto")?.classList.add("selected");

  // Restore a previously-chosen latency, if any -- session-only convenience,
  // not a correctness-critical value (the backend clamps regardless).
  const storedMs = localStorage.getItem(LATENCY_STORAGE_KEY);
  if (storedMs) {
    latencyInput.value = storedMs;
  }
  latencyValue.textContent = latencyInput.value;

  latencyInput.addEventListener("input", () => {
    latencyValue.textContent = latencyInput.value;
  });
  latencyInput.addEventListener("change", () => {
    localStorage.setItem(LATENCY_STORAGE_KEY, latencyInput.value);
  });

  const roll = await PianoRoll.create(container);
  // No separate checkbox -- the legend row itself is the button, and its
  // on/off state reads via dimming alone (see .roles-toggle.off).
  let rolesEnabled = true;
  roll.setRoleColoringEnabled(rolesEnabled);
  rolesToggle.addEventListener("click", () => {
    rolesEnabled = !rolesEnabled;
    rolesToggle.classList.toggle("off", !rolesEnabled);
    roll.setRoleColoringEnabled(rolesEnabled);
  });

  const tensionChips = new Map<string, HTMLElement>();
  document.querySelectorAll<HTMLElement>(".chip[data-tension]").forEach((chip) => {
    const key = chip.dataset.tension;
    if (key) tensionChips.set(key, chip);
  });

  await listen<PipelineEvent>("pipeline-event", (event) => {
    const payload = event.payload;
    if (payload.type === "Started") {
      // Anchor the piano roll's clock here, once model loading and audio
      // device setup are actually done -- not on the button click, which
      // is before that (sometimes noticeably before it, e.g. on a cold
      // DirectML session the first time). Anchoring too early left extra,
      // unaccounted-for delay baked into every note's position, which
      // reappeared as a gap near the playhead no fixed pipeline-latency
      // constant could account for.
      roll.reset(payload.total_latency_s);
    } else if (payload.type === "NoteConfirmed") {
      roll.addNote({
        start_s: payload.start_s,
        end_s: payload.end_s,
        pitch_midi: payload.pitch_midi,
        velocity: payload.velocity,
        melody: payload.melody,
        bass: payload.bass,
      });
    } else if (payload.type === "ChordChange") {
      chordLabel.textContent = payload.label;
      // A non-breaking space, not "" -- an empty element's rendered
      // height depends on min-height alone, which doesn't exactly match
      // a real text line's height (governed by line-height/font metrics
      // instead), so toggling between the two shifted this row's height
      // by a couple of px every time silence was reported, visibly
      // jittering the piano roll below it. Real (if invisible) inline
      // content keeps the row's height identical either way.
      romanLabel.textContent = payload.roman_numeral ?? " ";
    } else if (payload.type === "TensionsChanged") {
      const active = new Set(payload.tensions);
      tensionChips.forEach((chip, key) => {
        chip.classList.toggle("lit", active.has(key));
      });
    } else {
      keyLabel.textContent = payload.key_name;
    }
  });

  await listen<string>("pipeline-error", (event) => {
    console.error(event.payload);
  });

  startBtn.addEventListener("click", async () => {
    chordLabel.textContent = "--";
    keyLabel.textContent = "--";
    romanLabel.textContent = " ";
    tensionChips.forEach((chip) => chip.classList.remove("lit"));
    startBtn.disabled = true;
    stopBtn.disabled = false;
    latencyInput.disabled = true;
    try {
      const latencySeconds = Number(latencyInput.value) / 1000;
      await invoke("start_capture", { latencySeconds });
      // roll.reset() happens on the "Started" event above, once the
      // pipeline has actually begun, not here.
    } catch (err) {
      console.error(err);
      startBtn.disabled = false;
      stopBtn.disabled = true;
      latencyInput.disabled = false;
    }
  });

  stopBtn.addEventListener("click", stopCapture);
});
