import { tick } from "svelte";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { settings, savePianorollVisible } from "./settings.svelte";

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
let widthBeforeCollapse: number | null = null;
let heightBeforeCollapse: number | null = null;
let widthBeforeSettings: number | null = null;
let heightBeforeSettings: number | null = null;

// Shared by setPianorollVisible and the settings enter/leave pair below --
// both need the window's current size in logical (not physical) pixels to
// remember/compare against, and both go through the same
// scaleFactor()+innerSize() dance to get it.
async function currentLogicalWindowSize(
  win: ReturnType<typeof getCurrentWindow>,
): Promise<{ width: number; height: number }> {
  const scaleFactor = await win.scaleFactor();
  const size = await win.innerSize();
  return { width: size.width / scaleFactor, height: size.height / scaleFactor };
}

// Only the expanded main view (piano roll shown) is resizable, with the
// default size as its lower bound: extra height goes to the roll alone and
// extra width widens the roll's visible time span (fixed px/s). The
// collapsed and settings layouts stay fixed-size. The minimum has to be
// lifted before a collapse, or it would stop the window shrinking.
async function setResizableMode(
  win: ReturnType<typeof getCurrentWindow>,
  resizable: boolean,
): Promise<void> {
  if (resizable) {
    await win.setResizable(true);
    await win.setMinSize(new LogicalSize(DEFAULT_EXPANDED_WIDTH, DEFAULT_EXPANDED_HEIGHT));
  } else {
    await win.setMinSize(null);
    await win.setResizable(false);
  }
}

// The expanded (piano roll shown) size is remembered across launches. Only
// that size is saved: the collapsed and settings sizes are derived, and
// both are fixed-size modes the user can't drag.
const SIZE_STORAGE_KEY = "chord.expandedWindowSize";

function loadSavedSize(): { width: number; height: number } | null {
  try {
    const stored = JSON.parse(localStorage.getItem(SIZE_STORAGE_KEY) ?? "null");
    if (stored && Number.isFinite(stored.width) && Number.isFinite(stored.height)) {
      return {
        width: Math.max(DEFAULT_EXPANDED_WIDTH, Math.round(stored.width)),
        height: Math.max(DEFAULT_EXPANDED_HEIGHT, Math.round(stored.height)),
      };
    }
  } catch {
    // Corrupt entry: fall back to the defaults.
  }
  return null;
}

// Call once at startup, before the collapsed-state replay. Applies the saved
// size (or, if the roll starts hidden, arms it as what "show" restores to)
// and starts saving later user resizes.
export async function initWindowSize(): Promise<void> {
  const saved = loadSavedSize();
  if (saved) {
    widthBeforeCollapse = saved.width;
    heightBeforeCollapse = saved.height;
    if (settings.pianorollVisible) {
      try {
        await getCurrentWindow().setSize(new LogicalSize(saved.width, saved.height));
      } catch (err) {
        console.error(err);
      }
    }
  }

  let timer: number | undefined;
  window.addEventListener("resize", () => {
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      // Collapsed and settings sizes are not the user's expanded size.
      if (!settings.pianorollVisible || widthBeforeSettings !== null) return;
      if (window.innerWidth < DEFAULT_EXPANDED_WIDTH || window.innerHeight < DEFAULT_EXPANDED_HEIGHT) return;
      localStorage.setItem(
        SIZE_STORAGE_KEY,
        JSON.stringify({ width: window.innerWidth, height: window.innerHeight }),
      );
    }, 300);
  });
}

function measureCollapsedSize(
  appShell: HTMLElement,
  toggleBtn: HTMLElement,
): { width: number; height: number } {
  appShell.classList.add("measuring");
  const rect = appShell.getBoundingClientRect();
  // The toggle button is position:absolute (so it can hang off the
  // keyboard strip's would-be position even once that strip is gone),
  // which means it's out-of-flow and never counted in this shrink-to-fit
  // measurement -- without adding its own footprint back in, the
  // collapsed window comes out just wide enough for the tension grid
  // alone, and the button ends up drawn on top of it instead of past
  // its right edge.
  const btnWidth = toggleBtn.getBoundingClientRect().width;
  const size = { width: Math.ceil(rect.width + btnWidth), height: Math.ceil(rect.height) };
  appShell.classList.remove("measuring");
  return size;
}

// Shared by the toggle button's click and its startup replay, so
// opening the app in whichever state it was last left in resizes the
// window the same way an actual click would -- except that the replay
// passes rememberSize=false. On a real launch the window is at the
// tauri.conf.json default anyway (which the fallback above already
// covers), but a webview reload (Ctrl+R, Vite full-reload) keeps the OS
// window at whatever size it had -- already collapsed, if the roll was
// hidden -- and remembering that as the "expanded" size left "show"
// restoring the window to a collapsed-height sliver.
export async function setPianorollVisible(
  visible: boolean,
  appShell: HTMLElement,
  toggleBtn: HTMLElement,
  rememberSize = true,
): Promise<void> {
  settings.pianorollVisible = visible;
  savePianorollVisible();
  // Let Svelte apply the container's hidden class before measuring.
  await tick();

  try {
    const win = getCurrentWindow();
    const { width: currentLogicalWidth, height: currentLogicalHeight } =
      await currentLogicalWindowSize(win);

    if (!visible) {
      if (rememberSize) {
        widthBeforeCollapse = currentLogicalWidth;
        heightBeforeCollapse = currentLogicalHeight;
      }
      await setResizableMode(win, false);
      const collapsed = measureCollapsedSize(appShell, toggleBtn);
      await win.setSize(new LogicalSize(collapsed.width, collapsed.height));
    } else {
      const restoredWidth = widthBeforeCollapse ?? DEFAULT_EXPANDED_WIDTH;
      const restoredHeight = heightBeforeCollapse ?? DEFAULT_EXPANDED_HEIGHT;
      await setResizableMode(win, true);
      await win.setSize(new LogicalSize(restoredWidth, restoredHeight));
    }
  } catch (err) {
    console.error(err);
  }
}

// The settings page shares this window with the main view; if the piano
// roll was hidden when settings opened, the window would otherwise still
// be sitting at the collapsed size from setPianorollVisible above, and
// the settings page would render cramped into that sliver. Force the
// window to the fixed expanded size while settings is open -- skipping
// the resize when it's already there, which is the case whenever the
// roll was visible, so nothing visibly changes for that (common) case.
export async function enterSettingsWindowSize(): Promise<void> {
  try {
    const win = getCurrentWindow();
    const { width, height } = await currentLogicalWindowSize(win);
    widthBeforeSettings = width;
    heightBeforeSettings = height;
    await setResizableMode(win, false);
    if (Math.round(width) !== DEFAULT_EXPANDED_WIDTH || Math.round(height) !== DEFAULT_EXPANDED_HEIGHT) {
      await win.setSize(new LogicalSize(DEFAULT_EXPANDED_WIDTH, DEFAULT_EXPANDED_HEIGHT));
    }
  } catch (err) {
    console.error(err);
  }
}

// Restores whatever size the main view actually had before
// enterSettingsWindowSize forced the expanded size -- collapsed, if the
// piano roll was hidden at the time.
export async function leaveSettingsWindowSize(): Promise<void> {
  const width = widthBeforeSettings;
  const height = heightBeforeSettings;
  widthBeforeSettings = null;
  heightBeforeSettings = null;
  if (width === null || height === null) {
    return;
  }
  try {
    const win = getCurrentWindow();
    await setResizableMode(win, settings.pianorollVisible);
    await win.setSize(new LogicalSize(width, height));
  } catch (err) {
    console.error(err);
  }
}
