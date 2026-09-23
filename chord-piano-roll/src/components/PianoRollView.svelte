<script lang="ts">
  import { onMount } from "svelte";
  import { PianoRoll } from "../pianoroll";
  import { connectPipeline } from "../pipeline.svelte";
  import { settings } from "../settings.svelte";

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
      });
      if (disposed) off();
      else unlisten = off;
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
    title="クリックでメロディ/ベースの色分けを切り替え"
    onclick={toggleRoles}
  >
    <span class="legend-swatch melody"></span>Melody
    <span class="legend-swatch bass"></span>Bass
  </button>
</div>
