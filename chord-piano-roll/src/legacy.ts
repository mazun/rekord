import { tick } from "svelte";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { keyPairLabel, setKeyMode } from "./pipeline.svelte";
import { settings, savePianorollVisible } from "./settings.svelte";

// What's left of the pre-Svelte imperative wiring, pending its own
// migration phase: the key popup menu (to be replaced by a Bits UI
// component) and the piano-roll collapse's OS-window resize.

// Clicking the key label opens a small popup menu (Auto + the 12
// relative major/minor pairs) instead of a permanently-visible
// dropdown -- the picker is only on screen while actually choosing.
export function initKeyMenu(keyLabel: HTMLElement, keyMenu: HTMLElement): () => void {
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

  const onLabelClick = (e: MouseEvent) => {
    e.stopPropagation();
    if (keyMenu.classList.contains("hidden")) {
      openKeyMenu();
    } else {
      closeKeyMenu();
    }
  };
  const onLabelKeydown = (e: KeyboardEvent) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      openKeyMenu();
    }
  };
  const onDocumentClick = (e: MouseEvent) => {
    if (!keyMenu.contains(e.target as Node) && e.target !== keyLabel) {
      closeKeyMenu();
    }
  };
  const onDocumentKeydown = (e: KeyboardEvent) => {
    if (e.key === "Escape") closeKeyMenu();
  };
  const onMenuClick = (e: MouseEvent) => {
    const item = (e.target as HTMLElement).closest<HTMLButtonElement>(".key-menu-item");
    if (!item || !item.dataset.tonic) return;
    const value = item.dataset.tonic;
    keyMenuItems.forEach((el, key) => el.classList.toggle("selected", key === value));
    closeKeyMenu();
    setKeyMode(value === "auto" ? null : Number(value));
  };

  keyLabel.addEventListener("click", onLabelClick);
  keyLabel.addEventListener("keydown", onLabelKeydown);
  document.addEventListener("click", onDocumentClick);
  document.addEventListener("keydown", onDocumentKeydown);
  keyMenu.addEventListener("click", onMenuClick);
  keyMenuItems.get("auto")?.classList.add("selected");

  return () => {
    keyLabel.removeEventListener("click", onLabelClick);
    keyLabel.removeEventListener("keydown", onLabelKeydown);
    document.removeEventListener("click", onDocumentClick);
    document.removeEventListener("keydown", onDocumentKeydown);
    keyMenu.removeEventListener("click", onMenuClick);
    keyMenu.replaceChildren();
  };
}

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

// Shared by the toggle button and the startup replay in App.svelte, so
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
