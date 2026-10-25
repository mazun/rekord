<script lang="ts">
  import { t } from "../i18n.svelte";
  import { pipeline } from "../pipeline.svelte";
  import { togglePlayback, transport } from "../transport.svelte";

  // Replays what was just recorded (see transport.svelte.ts): play / pause toggle. While capturing
  // only with a marker placed (capture is then stopped first -- the loopback would record the
  // playback); otherwise not when there is nothing recorded.
  const playing = $derived(transport.state === "playing");
  const label = $derived(playing ? t("toolbar.pause") : t("toolbar.play"));
</script>

<button
  id="play-btn"
  class="play-btn"
  type="button"
  title={label}
  aria-label={label}
  disabled={pipeline.status === "running"
    ? !transport.marked
    : pipeline.status !== "idle" || !pipeline.hasRecording}
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

<button
  class="play-btn repeat-btn"
  class:on={transport.loop}
  type="button"
  title={t("toolbar.repeat")}
  aria-label={t("toolbar.repeat")}
  aria-pressed={transport.loop}
  onclick={() => (transport.loop = !transport.loop)}
>
  <svg viewBox="0 0 16 16" aria-hidden="true">
    <path
      d="M3 7.5V7a3 3 0 0 1 3-3h5.5V2.2l3 2.8-3 2.8V6H6a1 1 0 0 0-1 1v.5zM13 8.5V9a3 3 0 0 1-3 3H4.5v1.8l-3-2.8 3-2.8V10H10a1 1 0 0 0 1-1v-.5z"
    />
  </svg>
</button>

<style>
  /* Overlaid on the piano roll's top-left corner, just under the chord lane
     (CHORD_LANE_HEIGHT in pianoroll.ts, 34px). */
  .play-btn {
    position: absolute;
    top: calc(34px + 0.5rem);
    left: 0.6rem;
    z-index: 1;
    display: flex;
    align-items: center;
    padding: 0.3rem 0.6rem;
    border: none;
    border-radius: 6px;
    /* Same backdrop as the legend's buttons. */
    background: var(--overlay);
    color: var(--text);
    cursor: pointer;
  }

  .play-btn:hover:not(:disabled) {
    color: var(--text);
  }

  /* Right of the play button (0.6rem + its 2.3rem + a gap). */
  .repeat-btn {
    left: 3.3rem;
    color: var(--text-dim);
  }

  .repeat-btn.on {
    color: var(--play);
  }

  .play-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  svg {
    display: block;
    width: 1.1rem;
    height: 1.1rem;
    fill: currentColor;
  }
</style>
