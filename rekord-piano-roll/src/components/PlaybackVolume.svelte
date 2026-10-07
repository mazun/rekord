<script lang="ts">
  import { saveVolumes, settings } from "../settings.svelte";
  import { pushPlaybackParams } from "../transport.svelte";
  import { t } from "../i18n.svelte";

  // A speaker button opening a small panel with the two playback volumes. It lives next to the
  // play button rather than on the settings page: opening settings stops playback, and the point
  // is to adjust the balance while listening.
  let open = $state(false);
  let root: HTMLElement;

  function setVolume(which: "volumeOriginal" | "volumeSine", event: Event & { currentTarget: HTMLInputElement }) {
    settings[which] = Number(event.currentTarget.value);
    saveVolumes();
    pushPlaybackParams();
  }

  function closeOnOutside(event: PointerEvent) {
    if (open && !root.contains(event.target as Node)) open = false;
  }

  function closeOnEscape(event: KeyboardEvent) {
    if (event.key === "Escape") open = false;
  }
</script>

<svelte:window onpointerdown={closeOnOutside} onkeydown={closeOnEscape} />

<div class="volume" bind:this={root}>
  <button
    class="speaker-btn"
    class:open
    type="button"
    title={t("playbackVolume.title")}
    aria-label={t("playbackVolume.title")}
    aria-expanded={open}
    onclick={() => (open = !open)}
  >
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M2 6h2.5L8 3v10L4.5 10H2z" />
      <path class="wave" d="M10 5.5a3.5 3.5 0 0 1 0 5M11.8 3.8a6 6 0 0 1 0 8.4" />
    </svg>
  </button>
  {#if open}
    <div class="panel" role="group" aria-label={t("playbackVolume.title")}>
      <label>
        <span>{t("playbackVolume.original")}</span>
        <input type="range" min="0" max="1" step="0.01" value={settings.volumeOriginal} oninput={(e) => setVolume("volumeOriginal", e)} />
      </label>
      <label>
        <span>{t("playbackVolume.sine")}</span>
        <input type="range" min="0" max="1" step="0.01" value={settings.volumeSine} oninput={(e) => setVolume("volumeSine", e)} />
      </label>
    </div>
  {/if}
</div>

<style>
  /* Sits in Toolbar's row of left-hand controls; the panel opens below it. */
  .volume {
    position: relative;
  }

  .speaker-btn {
    display: flex;
    align-items: center;
    padding: 0.4rem 0.8rem;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text-dim);
    cursor: pointer;
  }

  .speaker-btn:hover,
  .speaker-btn.open {
    color: var(--text);
  }

  .speaker-btn svg {
    display: block;
    width: 1.1rem;
    height: 1.1rem;
    fill: currentColor;
    stroke: currentColor;
    stroke-width: 1.2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .speaker-btn .wave {
    fill: none;
  }

  .panel {
    position: absolute;
    top: calc(100% + 0.3rem);
    left: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    width: 12rem;
    padding: 0.6rem 0.8rem;
    border: 1px solid #3a3f4d;
    border-radius: 8px;
    background: var(--surface-raised);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    font-size: 0.8rem;
    color: var(--text-dim);
  }

  input[type="range"] {
    width: 100%;
    margin: 0;
    accent-color: #5b8cff;
  }
</style>
