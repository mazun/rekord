<script lang="ts">
  import { pipeline } from "../pipeline.svelte";
</script>

<div class="chord-block">
  <div id="chord-label">{pipeline.chord}</div>
  <div id="roman-label">{pipeline.roman}</div>
</div>

<style>
  .chord-block {
    /* Fixed so the tension grid's on-screen position doesn't shift every
       time the chord label's text length changes (e.g. "C" vs "Asus4(7)/G")
       -- text-align:right keeps the label flush against the tension grid
       regardless of how much of this box it actually fills. */
    width: 13rem;
    flex: none;
    text-align: right;
  }

  #chord-label {
    font-size: 2.7rem;
    font-weight: 600;
    min-height: 1.2em;
    letter-spacing: 0.02em;
    line-height: 1.1;
  }

  #roman-label {
    font-size: 1.1rem;
    color: var(--text-dim);
    min-height: 1em;
    padding-top: 0.15rem;
    letter-spacing: 0.03em;
  }
</style>
