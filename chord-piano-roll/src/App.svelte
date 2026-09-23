<script lang="ts">
  import { pipeline, stopCapture } from "./pipeline.svelte";
  import Toolbar from "./components/Toolbar.svelte";
  import KeyPicker from "./components/KeyPicker.svelte";
  import ChordDisplay from "./components/ChordDisplay.svelte";
  import TensionGrid from "./components/TensionGrid.svelte";
  import PianorollToggle from "./components/PianorollToggle.svelte";
  import PianoRollView from "./components/PianoRollView.svelte";
  import SettingsView from "./components/SettingsView.svelte";

  let view = $state<"main" | "settings">("main");

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
</script>

<main class="app">
  <div id="main-view" class:hidden={view !== "main"}>
    <Toolbar onOpenSettings={openSettings} />
    <KeyPicker />
    <div class="chord-row">
      <ChordDisplay />
      <TensionGrid />
      <PianorollToggle />
    </div>
    <PianoRollView />
  </div>
  <SettingsView hidden={view !== "settings"} onClose={() => (view = "main")} />
</main>

<style>
  .chord-row {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2.2rem;
    padding: 0.2rem 1rem 1rem;
  }
</style>
