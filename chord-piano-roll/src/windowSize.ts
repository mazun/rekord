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
// window the same way an actual click would.
export async function setPianorollVisible(
  visible: boolean,
  appShell: HTMLElement,
  toggleBtn: HTMLElement,
): Promise<void> {
  settings.pianorollVisible = visible;
  savePianorollVisible();
  // Let Svelte apply the container's hidden class before measuring.
  await tick();

  try {
    const win = getCurrentWindow();
    const scaleFactor = await win.scaleFactor();
    const currentSize = await win.innerSize();
    const currentLogicalWidth = currentSize.width / scaleFactor;
    const currentLogicalHeight = currentSize.height / scaleFactor;

    if (!visible) {
      widthBeforeCollapse = currentLogicalWidth;
      heightBeforeCollapse = currentLogicalHeight;
      const collapsed = measureCollapsedSize(appShell, toggleBtn);
      await win.setSize(new LogicalSize(collapsed.width, collapsed.height));
    } else {
      const restoredWidth = widthBeforeCollapse ?? DEFAULT_EXPANDED_WIDTH;
      const restoredHeight = heightBeforeCollapse ?? DEFAULT_EXPANDED_HEIGHT;
      await win.setSize(new LogicalSize(restoredWidth, restoredHeight));
    }
  } catch (err) {
    console.error(err);
  }
}
