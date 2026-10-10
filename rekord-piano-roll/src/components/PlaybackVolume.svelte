<script lang="ts">
  import { pipeline } from "../pipeline.svelte";
  import { saveVolumes, settings, WAVES, type VoicePart, type Wave } from "../settings.svelte";
  import { pushPlaybackParams } from "../transport.svelte";
  import { t } from "../i18n.svelte";

  // A speaker button opening a small panel with the playback mix: the recorded audio, the notes'
  // overall volume, and (for the 3-part model) each part's wave shape and volume, each with a mute
  // button. It lives next to the play button rather than on the settings page: opening settings stops
  // playback, and the point is to adjust the balance while listening.
  const PARTS = ["vocal", "bass", "other"] as const;
  // A single-part model labels every note "all", which has just a wave of its own.
  const threePart = $derived(pipeline.noteModel?.size === "mix");

  let open = $state(false);
  let root: HTMLElement;

  // Shape icons for the wave pickers (16x16, stroked).
  const WAVE_PATHS: Record<Wave, string> = {
    sine: "M1 8c2-7 5-7 7 0s5 7 7 0",
    triangle: "M1 8l3.5-5 7 10L15 8",
    square: "M1 12V4h7v8h7V4",
    saw: "M1 12l6-8v8l8-8",
  };

  /** Saves and sends the mix to the playback in progress. */
  function changed() {
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

{#snippet muteButton(muted: boolean, toggle: () => void)}
  <button
    class="mute-btn"
    class:on={muted}
    type="button"
    title={muted ? t("playbackVolume.unmute") : t("playbackVolume.mute")}
    aria-label={muted ? t("playbackVolume.unmute") : t("playbackVolume.mute")}
    aria-pressed={muted}
    onclick={toggle}
  >
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M2 6h2.5L8 3v10L4.5 10H2z" />
      {#if muted}
        <path class="stroke" d="M10.5 6l4 4M14.5 6l-4 4" />
      {:else}
        <path class="stroke" d="M10 5.5a3.5 3.5 0 0 1 0 5M11.8 3.8a6 6 0 0 1 0 8.4" />
      {/if}
    </svg>
  </button>
{/snippet}

{#snippet wavePicker(part: VoicePart)}
  <div class="waves" role="radiogroup" aria-label={t(`wave.${settings.voices[part].wave}`)}>
    {#each WAVES as wave (wave)}
      <button
        class="wave-btn"
        class:selected={settings.voices[part].wave === wave}
        type="button"
        role="radio"
        aria-checked={settings.voices[part].wave === wave}
        title={t(`wave.${wave}`)}
        aria-label={t(`wave.${wave}`)}
        onclick={() => {
          settings.voices[part].wave = wave;
          changed();
        }}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d={WAVE_PATHS[wave]} /></svg>
      </button>
    {/each}
  </div>
{/snippet}

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
      <div class="row" class:muted={settings.muteOriginal}>
        {@render muteButton(settings.muteOriginal, () => {
          settings.muteOriginal = !settings.muteOriginal;
          changed();
        })}
        <span class="name">{t("playbackVolume.original")}</span>
        <input
          type="range"
          min="0"
          max="1"
          step="0.01"
          value={settings.volumeOriginal}
          oninput={(e) => {
            settings.volumeOriginal = e.currentTarget.valueAsNumber;
            changed();
          }}
        />
      </div>

      <div class="row" class:muted={settings.muteNotes}>
        {@render muteButton(settings.muteNotes, () => {
          settings.muteNotes = !settings.muteNotes;
          changed();
        })}
        <span class="name">{t("playbackVolume.notes")}</span>
        {#if !threePart}
          {@render wavePicker("all")}
        {/if}
        <input
          type="range"
          min="0"
          max="1"
          step="0.01"
          value={settings.volumeNotes}
          oninput={(e) => {
            settings.volumeNotes = e.currentTarget.valueAsNumber;
            changed();
          }}
        />
      </div>

      {#if threePart}
        <div class="parts" class:muted={settings.muteNotes}>
          {#each PARTS as part (part)}
            <div class="row" class:muted={settings.voices[part].muted}>
              {@render muteButton(settings.voices[part].muted, () => {
                settings.voices[part].muted = !settings.voices[part].muted;
                changed();
              })}
              <span class="name"><span class="swatch {part}"></span>{t(`pianoRollView.${part}`)}</span>
              {@render wavePicker(part)}
              <input
                type="range"
                min="0"
                max="1"
                step="0.01"
                value={settings.voices[part].volume}
                oninput={(e) => {
                  settings.voices[part].volume = e.currentTarget.valueAsNumber;
                  changed();
                }}
              />
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  /* Sits first in the piano roll's legend (bottom-left); the panel opens above it. */
  .volume {
    position: relative;
  }

  button {
    border: none;
    background: none;
    color: var(--text-dim);
    cursor: pointer;
  }

  svg {
    display: block;
    fill: currentColor;
    stroke: currentColor;
    stroke-width: 1.2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .speaker-btn {
    display: flex;
    align-items: center;
    padding: 0.3rem 0.6rem;
    border-radius: 6px;
    /* Same backdrop as the legend's part buttons beside it. */
    background: rgba(20, 22, 28, 0.7);
  }

  .speaker-btn:hover,
  .speaker-btn.open {
    color: var(--text);
  }

  .speaker-btn svg {
    width: 1.1rem;
    height: 1.1rem;
  }

  .wave,
  .stroke {
    fill: none;
  }

  .panel {
    position: absolute;
    bottom: calc(100% + 0.3rem);
    left: 0;
    z-index: 2;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    width: 16rem;
    padding: 0.6rem 0.8rem;
    border: 1px solid #3a3f4d;
    border-radius: 8px;
    background: var(--surface-raised);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
  }

  /* [mute] [name ........ waves] over the slider. */
  .row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    column-gap: 0.4rem;
    row-gap: 0.1rem;
    font-size: 0.8rem;
    color: var(--text-dim);
  }

  .row input[type="range"] {
    grid-column: 2 / -1;
  }

  .row.muted .name,
  .row.muted input,
  .row.muted .waves,
  .parts.muted {
    opacity: 0.4;
  }

  .parts {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding-top: 0.5rem;
    border-top: 1px solid #3a3f4d;
  }

  .name {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .swatch {
    width: 0.6rem;
    height: 0.6rem;
    border-radius: 2px;
  }

  /* Mirrors the legend's colors in PianoRollView.svelte. */
  .swatch.vocal {
    background: #6be3a0;
  }

  .swatch.bass {
    background: #ff9f5b;
  }

  .swatch.other {
    background: #aecbff;
  }

  .mute-btn {
    display: flex;
    padding: 0.15rem;
    border-radius: 4px;
  }

  .mute-btn svg {
    width: 1rem;
    height: 1rem;
  }

  .mute-btn:hover,
  .mute-btn.on {
    color: var(--text);
  }

  .mute-btn.on {
    color: #e5484d;
  }

  .waves {
    display: flex;
    gap: 0.1rem;
  }

  .wave-btn {
    display: flex;
    padding: 0.2rem;
    border-radius: 4px;
    opacity: 0.55;
  }

  .wave-btn svg {
    width: 1.05rem;
    height: 1.05rem;
    fill: none;
  }

  .wave-btn:hover {
    opacity: 0.9;
  }

  .wave-btn.selected {
    background: #313747;
    color: #8fb0ff;
    opacity: 1;
  }

  input[type="range"] {
    width: 100%;
    margin: 0;
    accent-color: #5b8cff;
  }
</style>
