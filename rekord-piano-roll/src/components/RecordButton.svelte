<script lang="ts">
  import { t } from "../i18n.svelte";
  import { pipeline, startCapture, stopCapture } from "../pipeline.svelte";

  // One toggle instead of separate Start/Stop buttons: the icon shows what
  // clicking will do (record while stopped, stop while recording), and
  // "stopping" keeps the stop icon (disabled) until stop_capture returns.
  const recording = $derived(pipeline.status === "running" || pipeline.status === "stopping");
  const label = $derived(recording ? t("toolbar.stop") : t("toolbar.record"));

  function toggleCapture() {
    if (pipeline.status === "running") stopCapture();
    else if (pipeline.status === "idle") startCapture();
  }
</script>

<button
  id="capture-btn"
  class="capture-btn"
  class:recording
  type="button"
  title={label}
  aria-label={label}
  disabled={pipeline.status === "loading" || pipeline.status === "stopping"}
  onclick={toggleCapture}
>
  <svg viewBox="0 0 16 16" aria-hidden="true">
    <rect class="shape" x="2" y="2" width="12" height="12" rx="6" />
  </svg>
</button>

<style>
  /* Sits in the chord row's flow, left of the chord label and about as big
     as it: the app's main control. Red in both states (record / stop). */
  .capture-btn {
    flex: none;
    display: flex;
    align-items: center;
    /* 3.12rem icon + padding = 3.888rem (1.2x the chord label's 3.24rem height,
       ChordDisplay.svelte). */
    padding: 0.384rem;
    border: none;
    border-radius: 50%;
    /* A faint disc in both states, so it reads as a button. */
    background: rgba(255, 255, 255, 0.08);
    color: #e5484d;
    cursor: pointer;
  }

  .capture-btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.14);
  }

  .capture-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  svg {
    display: block;
    width: 3.12rem;
    height: 3.12rem;
    fill: currentColor;
  }

  /* The record circle morphs into the slightly rounded stop square. */
  .shape {
    transition:
      x 0.2s ease,
      y 0.2s ease,
      width 0.2s ease,
      height 0.2s ease,
      rx 0.2s ease;
  }

  .recording .shape {
    x: 3px;
    y: 3px;
    width: 10px;
    height: 10px;
    rx: 2px;
  }
</style>
