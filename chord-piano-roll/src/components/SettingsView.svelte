<script lang="ts">
  import { pipeline } from "../pipeline.svelte";
  import { settings, saveLatency } from "../settings.svelte";

  let { hidden, onClose }: { hidden: boolean; onClose: () => void } = $props();
</script>

<div id="settings-view" class:hidden>
  <div class="settings-page-header">
    <button
      id="settings-back-btn"
      class="icon-btn close-icon"
      type="button"
      title="閉じる"
      aria-label="閉じる"
      onclick={onClose}
    ></button>
    <h2>設定</h2>
  </div>
  <div class="settings-row">
    <label for="latency-input">Latency: <span id="latency-value">{settings.latencyMs}</span>ms</label>
    <input
      type="range"
      id="latency-input"
      min="100"
      max="1640"
      step="50"
      bind:value={settings.latencyMs}
      onchange={saveLatency}
      disabled={pipeline.status !== "idle"}
    />
  </div>
</div>
