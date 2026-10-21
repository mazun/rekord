<script lang="ts">
  import { getVersion } from "@tauri-apps/api/app";
  import { i18n, type Locale, setLocale, t } from "../i18n.svelte";
  import { pipeline, setAccidental } from "../pipeline.svelte";
  import { type Accidental, type ModelSize, saveAccidental, saveFutureFrames, saveModelSize, settings } from "../settings.svelte";
  import Segmented from "./Segmented.svelte";

  let { hidden, onClose }: { hidden: boolean; onClose: () => void } = $props();

  let version = $state("");
  getVersion().then(
    (v) => (version = v ?? ""),
    () => {},
  );

  // Description of the currently selected option, shown under each select.
  const modelDesc = $derived(
    {
      auto: t("settings.noteModelAutoDesc"),
      large: t("settings.noteModelLargeDesc"),
      medium: t("settings.noteModelMediumDesc"),
      small: t("settings.noteModelSmallDesc"),
    }[settings.modelSize],
  );
  const lookAheadDesc = $derived(
    settings.futureFrames === 8 ? t("settings.lookAhead8Desc") : t("settings.lookAhead16Desc"),
  );
  const accidentalDesc = $derived(
    {
      auto: t("settings.accidentalAutoDesc"),
      sharp: t("settings.accidentalSharpDesc"),
      flat: t("settings.accidentalFlatDesc"),
    }[settings.accidental],
  );
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
    <span class="settings-label" id="model-size-label">{t("settings.noteModel")}</span>
    <Segmented
      name="model-size"
      labelledby="model-size-label"
      options={[
        { value: "auto", label: t("settings.noteModelAuto") },
        { value: "large", label: t("settings.noteModelLarge") },
        { value: "medium", label: t("settings.noteModelMedium") },
        { value: "small", label: t("settings.noteModelSmall") },
      ] as { value: ModelSize; label: string }[]}
      value={settings.modelSize}
      disabled={pipeline.status !== "idle"}
      onchange={(v) => {
        settings.modelSize = v;
        saveModelSize();
      }}
    />
    <p class="settings-note">{modelDesc}</p>
    {#if pipeline.noteModel}
      <p class="settings-note">
        {t("settings.noteModelLastRun")}: Okarina {pipeline.noteModel.size[0].toUpperCase()} ({pipeline.noteModel.device === "cpu" ? "CPU" : "DirectML"})
      </p>
      {#if pipeline.noteModel.device === "cpu"}
        <p class="settings-note">{t("settings.cpuFallbackNote")}</p>
      {/if}
      {#if pipeline.noteModel.over_budget}
        <p class="settings-note">{t("settings.overBudgetNote")}</p>
      {/if}
    {/if}
  </div>
  <div class="settings-row">
    <span class="settings-label" id="future-frames-label">{t("settings.lookAhead")}</span>
    <Segmented
      name="future-frames"
      labelledby="future-frames-label"
      options={[
        { value: "8", label: t("settings.lookAhead8") },
        { value: "16", label: t("settings.lookAhead16") },
      ]}
      value={String(settings.futureFrames)}
      disabled={pipeline.status !== "idle"}
      onchange={(v) => {
        settings.futureFrames = Number(v);
        saveFutureFrames();
      }}
    />
    <p class="settings-note">{lookAheadDesc}</p>
  </div>
  <div class="settings-row">
    <span class="settings-label" id="accidental-label">{t("settings.accidental")}</span>
    <Segmented
      name="accidental"
      labelledby="accidental-label"
      options={[
        { value: "auto", label: t("settings.accidentalAuto") },
        { value: "sharp", label: t("settings.accidentalSharp") },
        { value: "flat", label: t("settings.accidentalFlat") },
      ] as { value: Accidental; label: string }[]}
      value={settings.accidental}
      onchange={(v) => {
        settings.accidental = v;
        saveAccidental();
        setAccidental();
      }}
    />
    <p class="settings-note">{accidentalDesc}</p>
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
  {#if version}
    <p class="settings-row settings-note">{t("settings.version")} {version}</p>
  {/if}
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
    padding: 0;
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
    max-width: 32rem;
    padding: 1.5rem 1.2rem;
    color: var(--text);
    font-size: 0.95rem;
  }

  /* Two stacked .settings-row blocks would otherwise double up their own
     1.5rem top/bottom padding into a 3rem gap between them. */
  .settings-row + .settings-row {
    padding-top: 0;
  }

  .settings-row select {
    font: inherit;
    padding: 0.3rem 0.5rem;
    border-radius: 4px;
    border: 1px solid #3a3f4d;
    background: var(--surface-raised);
    color: var(--text);
  }

  .settings-note {
    margin: 0;
    color: var(--text-dim);
    font-size: 0.85rem;
  }
</style>
