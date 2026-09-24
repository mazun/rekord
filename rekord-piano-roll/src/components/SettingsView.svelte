<script lang="ts">
  import { pipeline } from "../pipeline.svelte";
  import { settings, saveLatency } from "../settings.svelte";
  import { i18n, t, setLocale, type Locale } from "../i18n.svelte";

  let { hidden, onClose }: { hidden: boolean; onClose: () => void } = $props();
</script>

<div id="settings-view" class:hidden>
  <div class="settings-page-header">
    <button
      id="settings-back-btn"
      class="icon-btn close-icon"
      type="button"
      title={t("settings.close")}
      aria-label={t("settings.close")}
      onclick={onClose}
    ></button>
    <h2>{t("settings.title")}</h2>
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
  <div class="settings-row">
    <label for="language-select">{t("settings.language")}</label>
    <select
      id="language-select"
      value={i18n.locale}
      onchange={(e) => setLocale(e.currentTarget.value as Locale)}
    >
      <option value="en">English</option>
      <option value="ja">日本語</option>
    </select>
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
       "設定". Calibrated against the ja heading; re-check by eye if it
       reads off next to the en "Settings" heading. */
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

  /* Two stacked .settings-row blocks would otherwise double up their own
     1.5rem top/bottom padding into a 3rem gap between them. */
  .settings-row + .settings-row {
    padding-top: 0;
  }

  .settings-row input[type="range"] {
    width: 100%;
  }

  .settings-row select {
    font: inherit;
    padding: 0.3rem 0.5rem;
    border-radius: 4px;
    border: 1px solid #3a3f4d;
    background: var(--surface-raised);
    color: var(--text);
  }
</style>
