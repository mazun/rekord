<script lang="ts">
  import { pipeline } from "../pipeline.svelte";
  import { fitText } from "../fitText";
</script>

<div class="chord-block">
  <!-- Long names (e.g. "C♯sus4(7)/G♯") shrink to fit the box instead of
       spilling over the tension grid; the box's height is fixed in CSS. -->
  <div id="chord-label"><span use:fitText={pipeline.chord}>{pipeline.chord}</span></div>
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
    /* Fixed height (1.2 x 2.7rem): the inner span's font may shrink for long
       names, and the row must keep its height regardless. */
    height: 3.24rem;
    letter-spacing: 0.02em;
    line-height: 3.24rem;
    white-space: nowrap;
  }

  #roman-label {
    font-size: 1.1rem;
    color: var(--text-dim);
    min-height: 1em;
    padding-top: 0.15rem;
    letter-spacing: 0.03em;
  }
</style>
