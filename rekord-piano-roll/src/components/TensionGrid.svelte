<script lang="ts">
  import { pipeline } from "../pipeline.svelte";

  const TENSION_GROUPS = [
    { tensions: ["b13", "13"], alignRight: false },
    { tensions: ["11", "#11"], alignRight: true },
    { tensions: ["b9", "9", "#9"], alignRight: false },
  ];
</script>

<div class="tension-grid">
  {#each TENSION_GROUPS as group}
    <div class="tension-group" class:align-right={group.alignRight}>
      {#each group.tensions as tension}
        <div class="chip" class:lit={pipeline.tensions.includes(tension)}>{tension}</div>
      {/each}
    </div>
  {/each}
</div>

<style>
  .tension-grid {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: 0.4rem;
  }

  .tension-group {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .tension-group.align-right {
    justify-content: flex-end;
  }

  /* One badge per upper-structure tension (see chord_templates::TENSION_NAMES
     in rust/core) -- always present, dim by default, brightened while that
     pitch class is actually sounding outside the stabilized chord. Fixed
     width so b9/9/#9/11/#11/b13/13 all line up regardless of label length. */
  .chip {
    width: 3.2rem;
    padding: 0.32rem 0;
    border-radius: 999px;
    font-size: 0.95rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-align: center;
    background: var(--surface-raised);
    border: 1px solid var(--border);
    color: var(--text-faint);
    transition: background 220ms ease, border-color 220ms ease, color 220ms ease, box-shadow 220ms ease;
  }

  .chip.lit {
    background: color-mix(in srgb, var(--tension) 16%, transparent);
    border-color: var(--tension);
    color: var(--text);
    box-shadow: 0 0 14px color-mix(in srgb, var(--tension) 55%, transparent);
  }
</style>
