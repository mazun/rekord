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
