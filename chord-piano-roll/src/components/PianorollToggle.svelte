<script lang="ts">
  import { onMount } from "svelte";
  import { settings } from "../settings.svelte";
  import { setPianorollVisible } from "../windowSize";
  import pianorollToggleIconSvg from "../assets/pianoroll-toggle-icon.svg?raw";

  let button: HTMLButtonElement;

  // The collapsed-size measurement runs over the whole app shell, not just
  // this button -- found from here rather than threaded down as a prop,
  // since there's only ever the one.
  function toggle(visible: boolean) {
    const appShell = button.closest<HTMLElement>(".app");
    if (appShell) setPianorollVisible(visible, appShell, button);
  }

  onMount(() => {
    // Reproduce a collapsed state from a previous session -- the window
    // itself always starts at tauri.conf.json's expanded default (800x600),
    // so this both shrinks it down to match and captures that starting
    // size as what "show" should restore to later, exactly like a real
    // collapse click would from here.
    if (!settings.pianorollVisible) toggle(false);
  });
</script>

<!-- Injected as raw markup (via Vite's ?raw import) rather than an <img> --
     an <img> can't be recolored with currentColor, and this icon needs to
     dim/brighten on hover and turn active-blue when the piano roll is
     hidden, same as every other icon button here. -->
<button
  id="pianoroll-toggle-btn"
  class="icon-btn pianoroll-toggle-btn"
  class:active={!settings.pianorollVisible}
  type="button"
  title="ピアノロールの表示切替"
  aria-label="ピアノロールの表示切替"
  aria-pressed={!settings.pianorollVisible}
  bind:this={button}
  onclick={() => toggle(!settings.pianorollVisible)}
>
  {@html pianorollToggleIconSvg}
</button>
