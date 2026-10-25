<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import settingsGearIconSvg from "../assets/settings-gear-icon.svg?raw";
  import { t } from "../i18n.svelte";
  import KeyPicker from "./KeyPicker.svelte";

  let { onToggleSettings, settingsOpen }: { onToggleSettings: () => void; settingsOpen: boolean } =
    $props();

  const win = getCurrentWindow();
  let maximized = $state(false);

  // The window is undecorated, so this bar stands in for the OS one. The maximize button's icon
  // follows the window state, which also changes through a double-click on the bar or a snap.
  onMount(() => {
    const sync = () => win.isMaximized().then((m) => (maximized = m), () => {});
    sync();
    const unlisten = win.onResized(sync);
    return () => {
      unlisten.then((fn) => fn());
    };
  });
</script>

<div class="title-bar" data-tauri-drag-region>
  <div class="side" data-tauri-drag-region>
    <button
      type="button"
      class="settings"
      title={t("toolbar.settings")}
      aria-label={t("toolbar.settings")}
      onclick={onToggleSettings}
    >
      {@html settingsGearIconSvg}
    </button>
  </div>
  <!-- Hidden rather than unmounted: KeyPicker's Auto/manual state would reset. -->
  <div class="key" class:hidden={settingsOpen} data-tauri-drag-region>
    <KeyPicker />
  </div>
  <div class="controls">
    <button
      type="button"
      title={t("window.minimize")}
      aria-label={t("window.minimize")}
      onclick={() => win.minimize()}
    >
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5.5h10" /></svg>
    </button>
    <button
      type="button"
      title={maximized ? t("window.restore") : t("window.maximize")}
      aria-label={maximized ? t("window.restore") : t("window.maximize")}
      onclick={() => win.toggleMaximize()}
    >
      {#if maximized}
        <svg viewBox="0 0 10 10" aria-hidden="true">
          <path d="M2.5 2.5V0.5h7v7h-2M0.5 2.5h7v7h-7z" />
        </svg>
      {:else}
        <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0.5 0.5h9v9h-9z" /></svg>
      {/if}
    </button>
    <button
      type="button"
      class="close"
      title={t("window.close")}
      aria-label={t("window.close")}
      onclick={() => win.close()}
    >
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" /></svg>
    </button>
  </div>
</div>

<style>
  .title-bar {
    display: flex;
    align-items: stretch;
    justify-content: space-between;
    flex: none;
    height: 2rem;
    background: var(--bg);
    color: var(--text);
    user-select: none;
  }

  /* Same width as the controls, so the key readout between them stays centered on the window. */
  .side,
  .controls {
    flex: none;
    width: 8.7rem;
  }

  .side,
  .controls,
  .key {
    display: flex;
  }

  .key {
    flex: 1;
    min-width: 0;
  }

  .key.hidden :global(.key-row) {
    visibility: hidden;
  }

  .controls {
    justify-content: flex-end;
  }

  button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 2.9rem;
    padding: 0;
    border: none;
    border-radius: 0;
    background: none;
    color: var(--text);
    cursor: pointer;
  }

  button:hover {
    background: var(--surface-hover);
  }

  button.close:hover {
    background: var(--danger);
    color: #fff;
  }

  button svg {
    width: 0.65rem;
    height: 0.65rem;
    fill: none;
    stroke: currentColor;
    stroke-width: 1;
  }

  /* Injected via {@html}, so outside Svelte's scoping. */
  button.settings :global(svg) {
    display: block;
    width: 1rem;
    height: 1rem;
  }
</style>
