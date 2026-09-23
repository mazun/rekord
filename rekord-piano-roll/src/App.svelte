<script lang="ts">
  import { pipeline, startCapture, stopCapture } from "./pipeline.svelte";
  import { settings } from "./settings.svelte";
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
  // a still-running session. Recording is the app's resting state (it
  // starts on launch), so coming back resumes it -- picking up any latency
  // change made in between -- if it was running when settings opened.
  let resumeOnClose = false;

  async function openSettings() {
    resumeOnClose = pipeline.status === "running";
    if (resumeOnClose) {
      await stopCapture();
    }
    view = "settings";
  }

  function closeSettings() {
    view = "main";
    if (resumeOnClose && pipeline.status === "idle") {
      startCapture(settings.latencyMs);
    }
  }
</script>

<main class="app">
  <div id="main-view" class:hidden={view !== "main"}>
    <div class="key-toolbar-row">
      <Toolbar onOpenSettings={openSettings} />
      <KeyPicker />
    </div>
    <div class="chord-row">
      <ChordDisplay />
      <TensionGrid />
      <PianorollToggle />
    </div>
    <PianoRollView />
  </div>
  <SettingsView hidden={view !== "settings"} onClose={closeSettings} />
</main>

<style>
  /* No more standalone top bar -- the record/settings buttons are pinned
     (Toolbar.svelte, position:absolute) to this row's left/right edges,
     vertically centered on whatever height the key label gives the row,
     the same way PianorollToggle.svelte pins itself to .chord-row. */
  .key-toolbar-row {
    position: relative;
  }

  .chord-row {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2.2rem;
    padding: 1rem;
  }
</style>
