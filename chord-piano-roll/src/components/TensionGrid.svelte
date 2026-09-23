<script lang="ts">
  import { pipeline } from "../pipeline.svelte";

  const TENSION_GROUPS = [
    { tensions: ["b9", "9", "#9"], alignRight: false },
    { tensions: ["11", "#11"], alignRight: true },
    { tensions: ["b13", "13"], alignRight: false },
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
