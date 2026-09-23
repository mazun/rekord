<script lang="ts">
  import { onMount } from "svelte";
  import { pipeline, stopCapture } from "./pipeline.svelte";
  import { settings } from "./settings.svelte";
  import { setPianorollVisible } from "./legacy";
  import Toolbar from "./components/Toolbar.svelte";
  import KeyPicker from "./components/KeyPicker.svelte";
  import ChordDisplay from "./components/ChordDisplay.svelte";
  import TensionGrid from "./components/TensionGrid.svelte";
  import PianoRollView from "./components/PianoRollView.svelte";
  import SettingsView from "./components/SettingsView.svelte";
  import pianorollToggleIconSvg from "./assets/pianoroll-toggle-icon.svg?raw";

  let view = $state<"main" | "settings">("main");

  let appShell: HTMLElement;
  let pianorollToggleBtn: HTMLElement;

  // Stopping capture (if running) before navigating away keeps the piano
  // roll's clock/state consistent with what "Start" will set up fresh next
  // time -- the settings page is a separate view, not an overlay on top of
  // a still-running session.
  async function openSettings() {
    if (pipeline.status === "running") {
      await stopCapture();
    }
    view = "settings";
  }

  function togglePianoroll() {
    setPianorollVisible(!settings.pianorollVisible, appShell, pianorollToggleBtn);
  }

  onMount(() => {
    // Reproduce a collapsed state from a previous session -- the window
    // itself always starts at tauri.conf.json's expanded default (800x600),
    // so this both shrinks it down to match and captures that starting
    // size as what "show" should restore to later, exactly like a real
    // collapse click would from here.
    if (!settings.pianorollVisible) {
      setPianorollVisible(false, appShell, pianorollToggleBtn);
    }
  });
</script>

<main class="app" bind:this={appShell}>
  <div id="main-view" class:hidden={view !== "main"}>
    <Toolbar onOpenSettings={openSettings} />
    <KeyPicker />
    <div class="chord-row">
      <ChordDisplay />
      <TensionGrid />
      <!-- Injected as raw markup (via Vite's ?raw import) rather than an
           <img> -- an <img> can't be recolored with currentColor, and this
           icon needs to dim/brighten on hover and turn active-blue when the
           piano roll is hidden, same as every other icon button here. -->
      <button
        id="pianoroll-toggle-btn"
        class="icon-btn pianoroll-toggle-btn"
        class:active={!settings.pianorollVisible}
        type="button"
        title="ピアノロールの表示切替"
        aria-label="ピアノロールの表示切替"
        aria-pressed={!settings.pianorollVisible}
        bind:this={pianorollToggleBtn}
        onclick={togglePianoroll}
      >
        {@html pianorollToggleIconSvg}
      </button>
    </div>
    <PianoRollView />
  </div>
  <SettingsView hidden={view !== "settings"} onClose={() => (view = "main")} />
</main>
