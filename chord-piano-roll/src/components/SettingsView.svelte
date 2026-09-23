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

<style>
  .settings-page-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.6rem 1rem;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: 1.1rem;
    font-weight: 600;
  }

  /* A CSS-drawn X rather than the "x" glyph -- font glyph metrics for
     symbols like this routinely sit off-center within their own em box
     (varies by font/renderer), which no amount of flex/line-height
     centering on the surrounding button can correct. Two rotated bars
     centered via absolute positioning are exact regardless of font. */
  .close-icon {
    position: relative;
    /* +2px nudge -- the bars are drawn dead-center in this box (verified via
       computed getBoundingClientRect against the <h2>, which matched to
       <1px), but Segoe UI's Latin line-height metrics center the box itself
       slightly higher than where the kanji glyphs (drawn from a CJK
       fallback font with different ascent/descent proportions) visually
       sit, so a geometrically-centered icon still reads as high next to
       "設定". */
    top: 2px;
    width: 1.6rem;
    height: 1.6rem;
    padding: 0 !important;
    border: none;
    background: none;
  }

  .close-icon:hover {
    background: none;
  }

  .close-icon::before,
  .close-icon::after {
    content: "";
    position: absolute;
    top: 50%;
    left: 50%;
    width: 60%;
    height: 2px;
    background: var(--text);
  }

  .close-icon::before {
    transform: translate(-50%, -50%) rotate(45deg);
  }

  .close-icon::after {
    transform: translate(-50%, -50%) rotate(-45deg);
  }

  .settings-row {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-width: 24rem;
    padding: 1.5rem 1.2rem;
    color: var(--text);
    font-size: 0.95rem;
  }

  .settings-row input[type="range"] {
    width: 100%;
  }
</style>
