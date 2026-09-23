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

<style>
  .key-row {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    padding-top: 0.3rem;
  }

  /* Everything below is rendered by Bits UI components, not this one's own
     markup, so Svelte's scoping can't reach it -- hence :global(), kept
     under .key-row so it still only applies here (Select.Content isn't
     portaled, so the menu stays inside .key-row in the DOM). */

  /* A Select.Trigger, so a real <button> -- reset its native chrome so it
     still reads as the plain text label it used to be. */
  .key-row :global(#key-label) {
    background: none;
    border: none;
    font-family: inherit;
    text-align: center;
    font-size: 1rem;
    color: var(--text-dim);
    letter-spacing: 0.04em;
    cursor: pointer;
    border-radius: 4px;
    padding: 0.1rem 0.4rem;
  }

  .key-row :global(#key-label:hover) {
    background: var(--surface);
  }

  /* Positioned by Bits UI's floating wrapper (side/align/sideOffset on
     Select.Content above), not by this rule. */
  .key-row :global(.key-menu) {
    z-index: 10;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.25rem 0;
    max-height: 260px;
    overflow-y: auto;
    min-width: 180px;
  }

  .key-row :global(.key-menu:focus) {
    outline: none;
  }

  .key-row :global(.key-menu-item) {
    font-size: 0.8rem;
    color: var(--text-dim);
    background: none;
    border: none;
    text-align: left;
    padding: 0.3rem 0.7rem;
    cursor: pointer;
  }

  .key-row :global(.key-menu-item[data-highlighted]) {
    background: #262a36;
  }

  .key-row :global(.key-menu-item[data-selected]) {
    color: #e4e7ef;
  }
</style>
