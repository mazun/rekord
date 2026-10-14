<script lang="ts">
  import { onMount } from "svelte";
  import {
    debugExport,
    exportSelection,
    initDebugExport,
    setExportRange,
  } from "../debugExport.svelte";
  import { t } from "../i18n.svelte";
  import { dragSelectionAsMidi } from "../midiCopy.svelte";
  import { type NotePart, PianoRoll } from "../pianoroll";
  import {
    connectPipeline,
    startCapture,
    stopCapture,
  } from "../pipeline.svelte";
  import { PARTS, saveRollZoom, settings, zoomScale } from "../settings.svelte";
  import {
    connectTransport,
    setPlaybackPartVisible,
  } from "../transport.svelte";
  import PlayButton from "./PlayButton.svelte";
  import PlaybackVolume from "./PlaybackVolume.svelte";

  let container: HTMLElement;
  let roll: PianoRoll | null = null;
  let hiddenParts = $state({ vocal: false, bass: false, other: false });

  function togglePart(part: NotePart) {
    hiddenParts[part] = !hiddenParts[part];
    roll?.setPartVisible(part, !hiddenParts[part]);
    // A part hidden on the roll is not sounded in playback either.
    setPlaybackPartVisible(part, !hiddenParts[part]);
  }

  // The keyboard and the drum lane are shown independently (the window collapses with neither).
  function applyVisibility() {
    // Read both first: with no roll yet, `roll?.` would skip reading them and the effect would
    // track nothing.
    const { pianorollVisible, drumsVisible } = settings;
    roll?.setPianoVisible(pianorollVisible);
    roll?.setDrumsVisible(drumsVisible);
  }

  $effect(applyVisibility);

  const ZOOM_STEP = 0.2;
  const ZOOM_SNAP = 0.08; // the slider sticks to the centre (1x) within this distance

  function setZoom(position: number) {
    const clamped = Math.min(1, Math.max(-1, position));
    settings.rollZoom = Math.abs(clamped) < ZOOM_SNAP ? 0 : clamped;
    applyZoom();
    saveRollZoom();
  }

  let zoomOpen = $state(false);

  function applyZoom() {
    roll?.setZoom(zoomScale(settings.rollZoom));
  }

  // Esc closes the context menu, else drops the selection.
  function clearSelectionOnEscape(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (zoomOpen) {
      zoomOpen = false;
      return;
    }
    if (menu) menu = null;
    else roll?.clearSelection();
  }

  // The selection's context menu (debug export), at the client position of the right-click.
  let menu = $state<{ x: number; y: number } | null>(null);

  function closeMenuOnOutside(event: PointerEvent) {
    if (menu && !(event.target as HTMLElement).closest(".context-menu"))
      menu = null;
  }

  async function exportFromMenu() {
    await exportSelection();
    // Leave the result readable for a moment, then close.
    setTimeout(() => (menu = null), 1200);
  }

  // The parts hidden with the legend are left out of a MIDI drag.
  const isPartShown = (part: NotePart) => !hiddenParts[part];

  const exportLabel = $derived(
    debugExport.status === "busy"
      ? t("debugExport.busy")
      : debugExport.status === "done"
        ? t("debugExport.done")
        : debugExport.status === "failed"
          ? t("debugExport.failed")
          : `${t("debugExport.export")} (${debugExport.range ? (debugExport.range.endS - debugExport.range.startS).toFixed(1) : 0} s)`,
  );

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    let unlistenTransport: (() => void) | null = null;
    (async () => {
      const created = await PianoRoll.create(container);
      if (disposed) {
        created.destroy();
        return;
      }
      roll = created;
      applyZoom();
      applyVisibility();
      roll.onSelectionChange = (range) => {
        menu = null;
        setExportRange(range);
      };
      // Right-clicking the selection offers the debug export (only in builds that have it).
      roll.onSelectionContextMenu = (x, y) => {
        if (!debugExport.enabled) return false;
        menu = { x, y };
        return true;
      };
      roll.onSelectionDragOut = (part) => {
        menu = null;
        void dragSelectionAsMidi(part, isPartShown);
      };
      void initDebugExport();
      const off = await connectPipeline({
        onStarted: (totalLatencyS) => roll?.reset(totalLatencyS),
        onNoteStarted: (note) => roll?.startNote(note),
        onNote: (note) => roll?.addNote(note),
        onBeatLines: (lines) => roll?.setBeatLines(lines),
        onDrumHits: (event) => roll?.setDrumHits(event),
        onStopped: (info) => {
          roll?.stop();
          if (info) roll?.setPlayableEnd(info.end_s);
        },
        onChordLane: (update) => roll?.setChordSegments(update),
      });
      if (disposed) {
        off();
        return;
      }
      unlisten = off;
      const offTransport = await connectTransport(roll);
      if (disposed) {
        offTransport();
        return;
      }
      unlistenTransport = offTransport;

      // Capture starts on launch rather than waiting for a click. Stop
      // first: if the webview reloaded while capture kept running on the
      // backend, start_capture alone is a no-op there and would never emit
      // a fresh Started event to anchor this new roll's clock.
      await stopCapture();
      if (!disposed) await startCapture();
    })();

    return () => {
      disposed = true;
      unlisten?.();
      unlistenTransport?.();
      roll?.destroy();
      roll = null;
    };
  });
</script>

<svelte:window
  onkeydown={clearSelectionOnEscape}
  onpointerdown={closeMenuOnOutside}
/>

<!-- Hidden via a class, never {#if} -- unmounting would tear down the
     Pixi renderer and the roll's clock/notes along with it. -->
<div
  id="pianoroll-container"
  class:hidden={!settings.pianorollVisible && !settings.drumsVisible}
  bind:this={container}
>
  <PlayButton />
  <div class="legend" class:hidden={!settings.pianorollVisible}>
    <PlaybackVolume />
    <!-- The model labels every note with its part: one button per part, each showing or
         hiding that part's notes. -->
    {#each PARTS as part (part)}
      <button
        class="roles-toggle"
        class:off={hiddenParts[part]}
        type="button"
        title={t("pianoRollView.partToggle")}
        onclick={() => togglePart(part)}
      >
        <span class="legend-swatch {part}"></span>{t(`pianoRollView.${part}`)}
      </button>
    {/each}
  </div>
  <!-- Time-axis zoom: the magnifier sits in the corner above the keyboard (the chord lane's
       height, KEYBOARD_WIDTH x CHORD_LANE_HEIGHT in pianoroll.ts); hovering it opens the controls over
       the chord lane beside it, and they close when the pointer leaves. Minus = zoomed out (faster scroll, longer window), plus = zoomed in. -->
  <div
    class="zoom"
    role="presentation"
    onpointerenter={() => (zoomOpen = true)}
    onpointerleave={() => (zoomOpen = false)}
  >
    <button
      type="button"
      class="zoom-toggle"
      class:open={zoomOpen}
      title={t("pianoRollView.zoom")}
      aria-label={t("pianoRollView.zoom")}
      aria-expanded={zoomOpen}
    >
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="6.5" cy="6.5" r="4.5" />
        <path d="M10 10l4.5 4.5" />
      </svg>
    </button>
    {#if zoomOpen}
      <fieldset class="zoom-panel" aria-label={t("pianoRollView.zoom")}>
        <button
          type="button"
          class="step"
          aria-label="-"
          onclick={() =>
            setZoom(Math.round((settings.rollZoom - ZOOM_STEP) * 100) / 100)}
          >-</button
        >
        <div class="slider">
          <input
            type="range"
            min="-1"
            max="1"
            step="0.02"
            aria-label={t("pianoRollView.zoom")}
            value={settings.rollZoom}
            oninput={(e) => {
              setZoom(e.currentTarget.valueAsNumber);
              e.currentTarget.value = String(settings.rollZoom); // show the snap
            }}
          />
        </div>
        <button
          type="button"
          class="step"
          aria-label="+"
          onclick={() =>
            setZoom(Math.round((settings.rollZoom + ZOOM_STEP) * 100) / 100)}
          >+</button
        >
      </fieldset>
    {/if}
  </div>
</div>

{#if menu && debugExport.range}
  <div
    class="context-menu"
    role="menu"
    style:left="{menu.x}px"
    style:top="{menu.y}px"
  >
    <button
      type="button"
      role="menuitem"
      title={debugExport.status === "done" ? debugExport.path : ""}
      disabled={debugExport.status === "busy"}
      onclick={exportFromMenu}
    >
      {exportLabel}
    </button>
  </div>
{/if}

<style>
  #pianoroll-container {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  #pianoroll-container.hidden {
    display: none;
  }

  /* Created by Pixi at runtime, so outside Svelte's scoping. */
  #pianoroll-container :global(canvas) {
    display: block;
  }

  /* Overlaid on the piano roll itself rather than living in the toolbar --
     the coloring it toggles only means anything while notes are actually
     scrolling past underneath, so it's pointless (and confusing) to still
     show it once the roll is collapsed away. Living inside
     #pianoroll-container means it's naturally hidden along with everything
     else there once that container is display:none. Pinned to the bottom,
     not the top -- the chord lane (pianoroll.ts) now occupies the top-left
     corner, and the two overlapped there. */
  .legend {
    position: absolute;
    /* Clear of the scrollbar along the bottom edge (SCROLLBAR_HEIGHT + SCROLLBAR_BOTTOM_PADDING),
       or of the drum lane under the pitch rows; the roll sets the variable for the current layout. */
    bottom: calc(0.6rem + var(--roll-bottom-reserve, 14px));
    left: 0.6rem;
    z-index: 1;
    display: flex;
    gap: 0.4rem;
  }

  .legend.hidden {
    display: none;
  }

  /* The corner above the keyboard, which the chord lane leaves empty. */
  .zoom {
    position: absolute;
    top: 0;
    right: 0;
    z-index: 2;
    width: 34px; /* KEYBOARD_WIDTH */
    /* CHORD_LANE_HEIGHT, less half a key row: the top key is centred on the lane's lower edge, so
       the free corner ends half a row above it (rows = (height - lane) / (MAX_PITCH - MIN_PITCH)). */
    height: var(--zoom-corner-height, 30px); /* set by the roll (ZOOM_CORNER_HEIGHT in pianoroll.ts) */
  }

  .zoom-toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    padding: 0;
    border: none;
    background: none;
    color: var(--text-dim);
    cursor: pointer;
  }

  .zoom-toggle:hover,
  .zoom-toggle.open {
    color: var(--text);
  }

  .zoom-toggle svg {
    width: 1.1rem;
    height: 1.1rem;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
    stroke-linecap: round;
  }

  /* Laid over the chord lane, to the left of the icon. */
  .zoom-panel {
    position: absolute;
    top: 0;
    right: 100%;
    height: 100%;
    box-sizing: border-box;
    display: flex;
    align-items: center;
    gap: 0.35rem;
    min-width: 0;
    margin: 0;
    padding: 0 0.6rem;
    border: 0;
    background: rgba(20, 22, 28, 0.92);
    color: var(--text-dim);
    font-size: 0.85rem;
    user-select: none;
  }

  .zoom .step {
    width: 1.4rem;
    height: 1.4rem;
    padding: 0;
    border: none;
    border-radius: 4px;
    background: none;
    color: inherit;
    font: inherit;
    line-height: 1;
    cursor: pointer;
  }

  .zoom .step:hover {
    background: #313747;
    color: var(--text);
  }

  .slider {
    display: flex;
  }

  /* Drawn by hand so the hover/active states don't recolor it; the color is the "other" swatch's. */
  .zoom input[type="range"] {
    width: 9rem;
    height: 1rem;
    margin: 0;
    background: none;
    appearance: none;
    cursor: pointer;
  }

  .zoom input[type="range"]::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: 2px;
    background: #aecbff55;
  }

  .zoom input[type="range"]::-webkit-slider-thumb {
    width: 14px;
    height: 14px;
    margin-top: -5px;
    border: none;
    border-radius: 50%;
    background: #aecbff;
    appearance: none;
  }

  .roles-toggle {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.3rem 0.6rem;
    border-radius: 6px;
    border: none;
    background: rgba(20, 22, 28, 0.7);
    color: var(--text-dim);
    font: inherit;
    font-size: 0.85rem;
    cursor: pointer;
    user-select: none;
  }

  .roles-toggle:hover {
    background: rgba(20, 22, 28, 0.7);
  }

  /* Off reads via dimming alone (no separate checkbox): dim means that part is hidden. */
  .roles-toggle.off {
    opacity: 0.35;
  }

  /* The selection's right-click menu: one item, in the look of the settings popovers. */
  .context-menu {
    position: fixed;
    z-index: 10;
    padding: 0.25rem;
    border: 1px solid #3a3f4d;
    border-radius: 8px;
    background: var(--surface-raised);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
  }

  .context-menu button {
    display: block;
    padding: 0.35rem 0.8rem;
    border: none;
    border-radius: 5px;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 0.85rem;
    white-space: nowrap;
    cursor: pointer;
  }

  .context-menu button:hover:not(:disabled) {
    background: #313747;
  }

  .legend-swatch {
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 2px;
  }

  /* Mirrors MELODY_COLOR_MAX/BASS_COLOR_MAX/NOTE_COLOR_MAX in pianoroll.ts (vocal uses the
     "melody" green). */
  .legend-swatch.vocal {
    background: #6be3a0;
  }

  .legend-swatch.bass {
    background: #ff9f5b;
  }

  .legend-swatch.other {
    background: #aecbff;
  }
</style>
