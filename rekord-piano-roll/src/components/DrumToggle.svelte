<script lang="ts">
  import drumToggleIconSvg from "../assets/drum-toggle-icon.svg?raw";
  import { t } from "../i18n.svelte";
  import { settings } from "../settings.svelte";
  import { setRollPartVisible } from "../windowSize";

  let button: HTMLButtonElement;

  function toggle(visible: boolean) {
    const appShell = button.closest<HTMLElement>(".app");
    if (appShell) setRollPartVisible("drums", visible, appShell);
  }
</script>

<!-- Same look as PianorollToggle.svelte, which sits just left of this one. -->
<button
  id="drum-toggle-btn"
  class="icon-btn drum-toggle-btn"
  class:active={!settings.drumsVisible}
  type="button"
  title={t("drumToggle.toggle")}
  aria-label={t("drumToggle.toggle")}
  aria-pressed={!settings.drumsVisible}
  bind:this={button}
  onclick={() => toggle(!settings.drumsVisible)}
>
  {@html drumToggleIconSvg}
</button>

<style>
  .drum-toggle-btn {
    position: absolute;
    right: 0;
    bottom: 0.7rem;
    /* Narrower than .icon-btn's own right padding -- centers the icon on the keyboard strip
       below (KEYBOARD_WIDTH, 34px, in pianoroll.ts). */
    padding-right: 4px;
    border: none;
    background: none;
    color: var(--text);
    opacity: 0.35;
  }

  .drum-toggle-btn:hover {
    background: none;
  }

  /* Injected via {@html}, so outside Svelte's scoping. */
  .drum-toggle-btn :global(svg) {
    display: block;
    width: 26px;
    height: 26px;
  }

  .drum-toggle-btn.active {
    opacity: 1;
  }
</style>
