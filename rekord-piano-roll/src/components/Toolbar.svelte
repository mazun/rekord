<script lang="ts">
  import settingsGearIconSvg from "../assets/settings-gear-icon.svg?raw";
  import { t } from "../i18n.svelte";

  let { onOpenSettings }: { onOpenSettings: () => void } = $props();
</script>

<button
  id="settings-btn"
  class="icon-btn"
  type="button"
  title={t("toolbar.settings")}
  aria-label={t("toolbar.settings")}
  onclick={onOpenSettings}
>
  {@html settingsGearIconSvg}
</button>

<style>
  /* Pinned to .key-toolbar-row's right edge (App.svelte, position:relative)
     and centered vertically on whatever height the key label gives that row,
     the same technique PianorollToggle.svelte uses against .chord-row. */
  button {
    position: absolute;
    /* KeyPicker's own .key-row is also position:relative and comes later
       in the DOM, so without this it paints on top and swallows clicks
       on this button even where .key-row has no visible content. */
    z-index: 1;
    top: 50%;
    transform: translateY(-50%);
    padding: 0.4rem 1.1rem;
    border-radius: 6px;
    color: var(--text);
    cursor: pointer;
    font-size: 0.95rem;
  }

  #settings-btn {
    right: 0;
    /* Matches PianorollToggle.svelte's own right-inset override exactly,
       so the gear's glyph lines up on the same x as the piano-roll
       toggle icon below it, not just the two buttons' boxes. */
    padding-right: 4px !important;
    border: none;
    background: none;
    display: flex;
    align-items: center;
  }

  /* Injected via {@html}, so outside Svelte's scoping. */
  #settings-btn :global(svg) {
    display: block;
    width: 1.2rem;
    height: 1.2rem;
  }

  #settings-btn:hover {
    background: none;
  }
</style>
