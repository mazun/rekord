<script lang="ts">
  import { pipeline, startCapture, stopCapture } from "../pipeline.svelte";
  import { settings } from "../settings.svelte";

  let { onOpenSettings }: { onOpenSettings: () => void } = $props();
</script>

<div class="toolbar">
  <button
    id="start-btn"
    disabled={pipeline.status !== "idle"}
    onclick={() => startCapture(settings.latencyMs)}>Start</button
  >
  <button id="stop-btn" disabled={pipeline.status !== "running"} onclick={stopCapture}>Stop</button>
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
