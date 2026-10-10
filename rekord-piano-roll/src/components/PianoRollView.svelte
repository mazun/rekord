<script lang="ts">
  import { onMount } from "svelte";
  import { debugExport, exportSelection, initDebugExport, setExportRange } from "../debugExport.svelte";
  import { PianoRoll } from "../pianoroll";
  import { connectPipeline, pipeline, startCapture, stopCapture } from "../pipeline.svelte";
  import { settings } from "../settings.svelte";
  import { connectTransport, setPlaybackPartVisible } from "../transport.svelte";
  import { t } from "../i18n.svelte";
  import PlaybackVolume from "./PlaybackVolume.svelte";
  import PlayButton from "./PlayButton.svelte";

  const PARTS = ["vocal", "bass", "other"] as const;

  let container: HTMLElement;
  let roll: PianoRoll | null = null;
  let rolesEnabled = $state(true);
  let hiddenParts = $state({ vocal: false, bass: false, other: false });

  function togglePart(part: (typeof PARTS)[number]) {
    hiddenParts[part] = !hiddenParts[part];
    roll?.setPartVisible(part, !hiddenParts[part]);
    // A part hidden on the roll is not sounded in playback either.
    setPlaybackPartVisible(part, !hiddenParts[part]);
  }

  // No separate checkbox -- the legend row itself is the button, and its
  // on/off state reads via dimming alone (see .roles-toggle.off).
  function toggleRoles() {
    rolesEnabled = !rolesEnabled;
    roll?.setRoleColoringEnabled(rolesEnabled);
  }

  // Esc closes the context menu, else drops the selection.
  function clearSelectionOnEscape(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (menu) menu = null;
    else roll?.clearSelection();
  }

  // The selection's context menu (debug export), at the client position of the right-click.
  let menu = $state<{ x: number; y: number } | null>(null);

  function closeMenuOnOutside(event: PointerEvent) {
    if (menu && !(event.target as HTMLElement).closest(".context-menu")) menu = null;
  }

  async function exportFromMenu() {
    await exportSelection();
    // Leave the result readable for a moment, then close.
    setTimeout(() => (menu = null), 1200);
  }

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
      roll.setRoleColoringEnabled(rolesEnabled);
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
      void initDebugExport();
      const off = await connectPipeline({
        onStarted: (totalLatencyS) => roll?.reset(totalLatencyS),
        onNoteStarted: (note) => roll?.startNote(note),
        onNote: (note) => roll?.addNote(note),
        onBeatLines: (lines) => roll?.setBeatLines(lines),
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

<svelte:window onkeydown={clearSelectionOnEscape} onpointerdown={closeMenuOnOutside} />

<!-- Hidden via a class, never {#if} -- unmounting would tear down the
     Pixi renderer and the roll's clock/notes along with it. -->
<div id="pianoroll-container" class:hidden={!settings.pianorollVisible} bind:this={container}>
  <PlayButton />
  <div class="legend">
    <PlaybackVolume />
    {#if pipeline.noteModel?.size === "mix"}
      <!-- The 3-part model labels every note itself: one button per part, each
           showing or hiding that part's notes. -->
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
    {:else}
      <button
        id="roles-toggle"
        class="roles-toggle"
        class:off={!rolesEnabled}
        type="button"
        title={t("pianoRollView.rolesToggle")}
        onclick={toggleRoles}
      >
        <span class="legend-swatch melody"></span>{t("pianoRollView.melody")}
        <span class="legend-swatch bass"></span>{t("pianoRollView.bass")}
      </button>
    {/if}
  </div>
</div>

{#if menu && debugExport.range}
  <div class="context-menu" role="menu" style:left="{menu.x}px" style:top="{menu.y}px">
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
    bottom: calc(0.6rem + 14px); /* clear of the scrollbar along the bottom edge (SCROLLBAR_HEIGHT + SCROLLBAR_BOTTOM_PADDING) */
    left: 0.6rem;
    z-index: 1;
    display: flex;
    gap: 0.4rem;
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

  /* Off reads via dimming alone (no separate checkbox) -- bright means the
     coloring is currently applied, dim means notes are drawn in the default
     color regardless of role. */
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

  /* Mirrors MELODY_COLOR_MAX/BASS_COLOR_MAX/NOTE_COLOR_MAX in pianoroll.ts; vocal shares the
     melody color, other the default note color. */
  .legend-swatch.melody,
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
