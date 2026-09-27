<script lang="ts">
  import { onMount } from "svelte";
  import { PianoRoll } from "../pianoroll";
  import { connectPipeline, startCapture, stopCapture } from "../pipeline.svelte";
  import { settings } from "../settings.svelte";
  import { t } from "../i18n.svelte";

  let container: HTMLElement;
  let roll: PianoRoll | null = null;
  let rolesEnabled = $state(true);

  // No separate checkbox -- the legend row itself is the button, and its
  // on/off state reads via dimming alone (see .roles-toggle.off).
  function toggleRoles() {
    rolesEnabled = !rolesEnabled;
    roll?.setRoleColoringEnabled(rolesEnabled);
  }

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    (async () => {
      const created = await PianoRoll.create(container);
      if (disposed) {
        created.destroy();
        return;
      }
      roll = created;
      roll.setRoleColoringEnabled(rolesEnabled);
      const off = await connectPipeline({
        onStarted: (totalLatencyS) => roll?.reset(totalLatencyS),
        onNote: (note) => roll?.addNote(note),
        onBeatLines: (lines) => roll?.setBeatLines(lines),
      });
      if (disposed) {
        off();
        return;
      }
      unlisten = off;

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
      roll?.destroy();
      roll = null;
    };
  });
</script>

<!-- Hidden via a class, never {#if} -- unmounting would tear down the
     Pixi renderer and the roll's clock/notes along with it. -->
<div id="pianoroll-container" class:hidden={!settings.pianorollVisible} bind:this={container}>
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
</div>

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
     else there once that container is display:none. */
  .roles-toggle {
    position: absolute;
    top: 0.6rem;
    left: 0.6rem;
    z-index: 1;
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

  .legend-swatch {
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 2px;
  }

  /* Mirrors MELODY_COLOR_MAX/BASS_COLOR_MAX in pianoroll.ts. */
  .legend-swatch.melody {
    background: #6be3a0;
  }

  .legend-swatch.bass {
    background: #ff9f5b;
  }
</style>
