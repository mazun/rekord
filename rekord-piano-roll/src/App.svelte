<script lang="ts">
  import ChordDisplay from "./components/ChordDisplay.svelte";
  import DrumToggle from "./components/DrumToggle.svelte";
  import KeyPicker from "./components/KeyPicker.svelte";
  import PianoRollView from "./components/PianoRollView.svelte";
  import PianorollToggle from "./components/PianorollToggle.svelte";
  import RecordButton from "./components/RecordButton.svelte";
  import SettingsView from "./components/SettingsView.svelte";
  import TensionGrid from "./components/TensionGrid.svelte";
  import Toolbar from "./components/Toolbar.svelte";
  import { pipeline, startCapture, stopCapture } from "./pipeline.svelte";
  import { stopPlayback } from "./transport.svelte";
  import { enterSettingsWindowSize, leaveSettingsWindowSize } from "./windowSize";

  let view = $state<"main" | "settings">("main");

  // Stopping capture (if running) before navigating away keeps the piano
  // roll's clock/state consistent with what "Start" will set up fresh next
  // time -- the settings page is a separate view, not an overlay on top of
  // a still-running session. Recording is the app's resting state (it
  // starts on launch), so coming back resumes it -- picking up any note-
  // model setting change made in between -- if it was running when
  // settings opened.
  let resumeOnClose = false;

  async function openSettings() {
    // The settings page is a separate view and the roll behind it can't be heard being adjusted.
    await stopPlayback();
    resumeOnClose = pipeline.status === "running";
    if (resumeOnClose) {
      await stopCapture();
    }
    await enterSettingsWindowSize();
    view = "settings";
  }

  async function closeSettings() {
    // Switch views before shrinking the window back down -- restoring a
    // collapsed size while the settings page is still showing would
    // briefly cram it into that sliver instead of the main view.
    view = "main";
    await leaveSettingsWindowSize();
    if (resumeOnClose && pipeline.status === "idle") {
      startCapture();
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
      <div class="chord-group">
        <RecordButton />
        <ChordDisplay />
      </div>
      <TensionGrid />
      <PianorollToggle />
      <DrumToggle />
    </div>
    <PianoRollView />
  </div>
  <SettingsView hidden={view !== "settings"} onClose={closeSettings} />
</main>

<style>
  /* No more standalone top bar -- the settings button is pinned
     (Toolbar.svelte, position:absolute) to this row's right edge,
     vertically centered on whatever height the key label gives the row,
     the same way PianorollToggle.svelte pins itself to .chord-row. */
  .key-toolbar-row {
    position: relative;
  }

  /* The piano-roll toggle is pinned to the row's right edge, out of flow; the
     side padding (mirrored on the left to keep the group centered) reserves
     room for it so the collapsed window doesn't draw it over the tension grid. */
  /* Top-aligned so the record button lines up with the chord label itself
     (the label is a fixed 3.24rem tall, ChordDisplay.svelte; the button is
     taller and hangs below it), not with the label plus the roman numeral
     under it. */
  .chord-group {
    display: flex;
    align-items: flex-start;
    gap: 1.4rem;
  }

  .chord-row {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2.2rem;
    padding: 1rem 4.8rem; /* room for the two toggles */
  }
</style>
