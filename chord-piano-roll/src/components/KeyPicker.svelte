<script lang="ts">
  import { Select } from "bits-ui";
  import { pipeline, keyPairLabel, setKeyMode } from "../pipeline.svelte";

  // Auto + the 12 relative major/minor pairs. The trigger always shows the
  // pipeline's current key label (the detected key in Auto mode), not the
  // selected item's label -- "Auto" itself is never what's displayed.
  const ITEMS = [
    { value: "auto", label: "Auto" },
    ...Array.from({ length: 12 }, (_, pc) => ({ value: String(pc), label: keyPairLabel(pc) })),
  ];

  let mode = $state("auto");
</script>

<div class="key-row">
  <Select.Root
    type="single"
    bind:value={mode}
    onValueChange={(v) => setKeyMode(v === "auto" ? null : Number(v))}
  >
    <Select.Trigger id="key-label">{pipeline.keyName}</Select.Trigger>
    <Select.Content class="key-menu" side="bottom" align="center" sideOffset={3}>
      {#each ITEMS as item (item.value)}
        <Select.Item class="key-menu-item" value={item.value} label={item.label}>
          {item.label}
        </Select.Item>
      {/each}
    </Select.Content>
  </Select.Root>
</div>
