<script lang="ts">
  import { onMount } from "svelte";
  import { settings } from "../settings.svelte";
  import { initWindowSize, setPianorollVisible } from "../windowSize";
  import pianorollToggleIconSvg from "../assets/pianoroll-toggle-icon.svg?raw";
  import { t } from "../i18n.svelte";

  let button: HTMLButtonElement;

  // The collapsed-size measurement runs over the whole app shell, not just
  // this button -- found from here rather than threaded down as a prop,
  // since there's only ever the one.
  function toggle(visible: boolean, rememberSize = true) {
    const appShell = button.closest<HTMLElement>(".app");
    if (appShell) setPianorollVisible(visible, appShell, button, rememberSize);
  }

  onMount(() => {
    // Reproduce a collapsed state from a previous session -- the window
    // starts at tauri.conf.json's expanded default (800x600) on launch, so
    // this shrinks it down to match. It deliberately doesn't record the
    // current size as what "show" restores to (see windowSize.ts) -- after
    // a webview reload the window is already collapsed. The saved expanded
    // size is applied first so a visible roll reopens at its last size.
    initWindowSize().then(() => {
      if (!settings.pianorollVisible) toggle(false, false);
    });
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
  title={t("pianorollToggle.toggle")}
  aria-label={t("pianorollToggle.toggle")}
  aria-pressed={!settings.pianorollVisible}
  bind:this={button}
  onclick={() => toggle(!settings.pianorollVisible)}
>
  {@html pianorollToggleIconSvg}
</button>

<style>
  /* Pinned to the chord row's own right edge (like the settings gear pins
     to the toolbar's) rather than flowing next to the tension grid --
     flowing with the centered chord-block/tension-grid group would have it
     drift with the window width instead of staying put at a fixed corner.
     `bottom` matches .chord-row's own padding-bottom (App.svelte) exactly:
     the tension grid is the row's tallest child, so its bottom edge sits
     flush with the row's content-box bottom (border-box bottom minus that
     same padding) -- using the same value here lines the two up without
     measuring anything. */
  .pianoroll-toggle-btn {
    position: absolute;
    /* Offset by less than the tension grid's own 1rem inset from the row's
       edges -- .icon-btn's padding (0.3rem bottom, 0.55rem right, both
       !important) sits between this box and the svg inside it, so the
       visible icon needs that padding subtracted back out to land flush
       with the grid rather than the icon's own border box. */
    right: 0;
    bottom: 0.7rem;
    /* Narrower than .icon-btn's own 0.55rem right padding -- centers the
       icon on the piano roll's keyboard strip below (KEYBOARD_WIDTH, 34px,
       in pianoroll.ts) rather than on this button's own box, which sits
       further left than the keyboard's true center. */
    padding-right: 4px !important;
    border: none;
    background: none;
    color: var(--text);
    /* Piano roll shown -- dimmed (same opacity as .roles-toggle.off
       elsewhere) so the icon reads as "more to reveal here" rather than
       drawing attention while the roll itself is already visible; full
       brightness (below, .active) then reads as "this is the only way back
       to the roll" once it's hidden. */
    opacity: 0.35;
  }

  .pianoroll-toggle-btn:hover {
    background: none;
  }

  /* Injected via {@html}, so outside Svelte's scoping. */
  .pianoroll-toggle-btn :global(svg) {
    display: block;
    width: 26px;
    height: 26px;
  }

  .pianoroll-toggle-btn.active {
    opacity: 1;
  }
</style>
