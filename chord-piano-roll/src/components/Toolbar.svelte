<script lang="ts">
  import { pipeline, startCapture, stopCapture } from "../pipeline.svelte";
  import { settings } from "../settings.svelte";

  let { onOpenSettings }: { onOpenSettings: () => void } = $props();

  // One toggle instead of separate Start/Stop buttons: the icon shows what
  // clicking will do (record while stopped, stop while recording), and
  // "stopping" keeps the stop icon (disabled) until stop_capture returns.
  const recording = $derived(pipeline.status === "running" || pipeline.status === "stopping");
  const label = $derived(recording ? "停止" : "録音開始");

  function toggleCapture() {
    if (pipeline.status === "running") stopCapture();
    else if (pipeline.status === "idle") startCapture(settings.latencyMs);
  }
</script>

<div class="toolbar">
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
      {#if recording}
        <rect x="3" y="3" width="10" height="10" rx="1.5" />
      {:else}
        <circle cx="8" cy="8" r="5.5" />
      {/if}
    </svg>
  </button>
  <button
    id="settings-btn"
    class="icon-btn"
    type="button"
    title="設定"
    aria-label="設定"
    onclick={onOpenSettings}
  >
    ⚙
  </button>
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.6rem 1rem;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  button {
    padding: 0.4rem 1.1rem;
    border-radius: 6px;
    border: 1px solid #3a3f4d;
    background: var(--surface-raised);
    color: var(--text);
    cursor: pointer;
    font-size: 0.95rem;
  }

  button:hover:not(:disabled) {
    background: #313747;
  }

  button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .capture-btn {
    display: flex;
    align-items: center;
    padding: 0.4rem 0.8rem;
    /* Record icon is red (the universal "record" cue); the stop icon
       takes the regular text color. */
    color: #e5484d;
  }

  .capture-btn.recording {
    color: var(--text);
  }

  .capture-btn svg {
    display: block;
    width: 1.1rem;
    height: 1.1rem;
    fill: currentColor;
  }

  #settings-btn {
    margin-left: auto;
    border: none;
    background: none;
    font-size: 1.7rem;
  }

  #settings-btn:hover {
    background: none;
  }
</style>
