<script lang="ts">
  import settingsGearIconSvg from "../assets/settings-gear-icon.svg?raw";
  import { pipeline, startCapture, stopCapture } from "../pipeline.svelte";
  import { togglePlayback, transport } from "../transport.svelte";
  import { t } from "../i18n.svelte";
  import PlaybackVolume from "./PlaybackVolume.svelte";

  let { onOpenSettings }: { onOpenSettings: () => void } = $props();

  // One toggle instead of separate Start/Stop buttons: the icon shows what
  // clicking will do (record while stopped, stop while recording), and
  // "stopping" keeps the stop icon (disabled) until stop_capture returns.
  const recording = $derived(pipeline.status === "running" || pipeline.status === "stopping");
  const label = $derived(recording ? t("toolbar.stop") : t("toolbar.record"));

  function toggleCapture() {
    if (pipeline.status === "running") stopCapture();
    else if (pipeline.status === "idle") startCapture();
  }

  // Replays what was just recorded (see transport.svelte.ts): play / pause toggle. Not while
  // capturing -- the loopback would record the playback -- or when there is nothing recorded.
  const playing = $derived(transport.state === "playing");
  const playLabel = $derived(playing ? t("toolbar.pause") : t("toolbar.play"));
</script>

<div class="left-controls">
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
  id="play-btn"
  class="play-btn"
  type="button"
  title={playLabel}
  aria-label={playLabel}
  disabled={pipeline.status !== "idle" || !pipeline.hasRecording}
  onclick={togglePlayback}
>
  <svg viewBox="0 0 16 16" aria-hidden="true">
    {#if playing}
      <rect x="3.5" y="3" width="3" height="10" rx="0.8" />
      <rect x="9.5" y="3" width="3" height="10" rx="0.8" />
    {:else}
      <path d="M4.5 2.5v11l9-5.5z" />
    {/if}
  </svg>
</button>
<PlaybackVolume />
{#if pipeline.bpm !== null}
  <span class="bpm">{pipeline.bpm} BPM</span>
{/if}
</div>
<button
  id="settings-btn"
  class="icon-btn"
  type="button"
  title={t("toolbar.settings")}
  aria-label={t("toolbar.settings")}
  onclick={onOpenSettings}
>
  {@html settingsGearIconSvg}
</button>

<style>
  /* Each button pins itself to .key-toolbar-row's left/right edge
     (App.svelte, position:relative) and centers vertically on whatever
     height the key label gives that row, the same technique
     PianorollToggle.svelte uses against .chord-row -- not laid out by a
     shared flex container, since the two buttons need independent
     left/right anchors rather than a single shared parent box. */
  button {
    position: absolute;
    /* KeyPicker's own .key-row is also position:relative and comes later
       in the DOM, so without this it paints on top and swallows clicks
       on these buttons even where .key-row has no visible content. */
    z-index: 1;
    top: 50%;
    transform: translateY(-50%);
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

  /* The record, play and volume buttons and the tempo, in one row pinned to the left edge. */
  .left-controls {
    position: absolute;
    z-index: 1;
    left: 0;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
  }

  .left-controls button {
    position: static;
    transform: none;
  }

  .capture-btn {
    display: flex;
    align-items: center;
    padding: 0.4rem 0.8rem;
    border: none;
    background: none;
    /* Record icon is red (the universal "record" cue); the stop icon
       takes the regular text color. */
    color: #e5484d;
  }

  .capture-btn.recording {
    color: var(--text);
  }

  .capture-btn svg,
  .play-btn svg {
    display: block;
    width: 1.1rem;
    height: 1.1rem;
    fill: currentColor;
  }

  .play-btn {
    display: flex;
    align-items: center;
    padding: 0.4rem 0.8rem;
    border: none;
    background: none;
    color: var(--text);
  }

  .play-btn:hover:not(:disabled) {
    background: none;
    color: #fff;
  }

  .bpm {
    margin-left: 0.4rem;
    color: var(--text-dim);
    font-size: 0.85rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    pointer-events: none;
  }

  #settings-btn {
    right: 0;
    /* Matches PianorollToggle.svelte's own right-inset override exactly,
       so the gear's glyph lines up on the same x as the piano-roll
       toggle icon below it, not just the two buttons' boxes. */
    padding-right: 4px !important;
    border: none;
    background: none;
    display: flex;
    align-items: center;
  }

  /* Injected via {@html}, so outside Svelte's scoping. */
  #settings-btn :global(svg) {
    display: block;
    width: 1.2rem;
    height: 1.2rem;
  }

  #settings-btn:hover {
    background: none;
  }
</style>
