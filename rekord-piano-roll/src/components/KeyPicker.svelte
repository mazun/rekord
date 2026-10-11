<script lang="ts">
  import { pipeline, setKeyMode } from "../pipeline.svelte";
  import { t } from "../i18n.svelte";

  // A toggle plus up/down steppers instead of a dropdown: the collapsed
  // window is only a few rows tall, so a 13-item menu opened from here
  // always ran off the bottom of it.
  let auto = $state(true);

  // The key currently displayed -- the detected one in Auto mode, the picked
  // one otherwise (setKeyMode updates the label immediately). Stepping from
  // Auto starts from what's on screen, falling back to C before the first
  // estimate arrives ("--").
  const currentPc = $derived(pipeline.keyPc ?? 0);

  function toggleAuto() {
    auto = !auto;
    // Leaving Auto pins the key that's currently displayed; entering it
    // hands control back to the estimator.
    setKeyMode(auto ? null : currentPc);
  }

  // Stepping while in Auto switches to manual mode -- there's nothing to
  // step through otherwise, so this saves a separate "turn Auto off" click.
  function step(semitones: number) {
    auto = false;
    setKeyMode((currentPc + semitones + 12) % 12);
  }
</script>

<div class="key-row">
  <button
    class="auto-btn"
    class:active={auto}
    type="button"
    aria-pressed={auto}
    title={t("keyPicker.autoDetect")}
    onclick={toggleAuto}
  >
    {t("keyPicker.auto")}
  </button>
  <span id="key-label">{pipeline.keyName}</span>
  <div class="after-key">
    <div class="steppers">
      <button type="button" title={t("keyPicker.semitoneUp")} aria-label={t("keyPicker.semitoneUp")} onclick={() => step(1)}>▲</button>
      <button type="button" title={t("keyPicker.semitoneDown")} aria-label={t("keyPicker.semitoneDown")} onclick={() => step(-1)}>▼</button>
    </div>
    {#if pipeline.bpm !== null}
      <span class="bpm">{pipeline.bpm} BPM</span>
    {/if}
  </div>
</div>

<style>
  /* 1fr / auto / 1fr keeps the label itself centered on the row, the same
     place it was when it was the row's only content, regardless of how wide
     the Auto button or the steppers are. */
  .key-row {
    position: relative;
    display: grid;
    /* Fixed middle column: the label's text length varies with the key
       ("C Major (A Minor)" vs "C♯ Major (A♯ Minor)"), and an auto column
       made the Auto button and steppers shift with it. */
    grid-template-columns: 1fr 13rem 1fr;
    align-items: center;
    column-gap: 0.5rem;
    padding-top: 0.3rem;
  }

  #key-label {
    font-size: 1rem;
    color: var(--text-dim);
    letter-spacing: 0.04em;
    padding: 0.1rem 0.4rem;
    text-align: center;
    white-space: nowrap;
  }

  button {
    font-family: inherit;
    color: var(--text-dim);
    background: none;
    border: none;
    border-radius: 4px;
    cursor: pointer;
  }

  button:hover {
    background: var(--surface);
  }

  .auto-btn {
    justify-self: end;
    font-size: 0.75rem;
    padding: 0.15rem 0.5rem;
  }

  .auto-btn.active {
    color: #e4e7ef;
    background: #262a36;
  }

  /* The steppers and the tempo, side by side to the key label's right. */
  .after-key {
    justify-self: start;
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .bpm {
    color: var(--text-dim);
    font-size: 0.85rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .steppers {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .steppers button {
    font-size: 0.5rem;
    line-height: 1;
    padding: 0.12rem 0.4rem;
  }
</style>
