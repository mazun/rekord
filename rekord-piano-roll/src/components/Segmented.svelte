<script lang="ts" generics="T extends string">
  // Horizontal single-choice control (native radios, so keyboard/a11y come
  // for free).
  let {
    name,
    labelledby,
    options,
    value,
    disabled = false,
    onchange,
  }: {
    name: string;
    labelledby: string;
    options: { value: T; label: string }[];
    value: T;
    disabled?: boolean;
    onchange: (value: T) => void;
  } = $props();
</script>

<div class="segmented" role="radiogroup" aria-labelledby={labelledby}>
  {#each options as o (o.value)}
    <label class:selected={o.value === value} class:disabled>
      <input
        type="radio"
        {name}
        value={o.value}
        checked={o.value === value}
        {disabled}
        onchange={() => onchange(o.value)}
      />
      {o.label}
    </label>
  {/each}
</div>

<style>
  .segmented {
    display: flex;
    width: fit-content;
    border: 1px solid #3a3f4d;
    border-radius: 4px;
    overflow: hidden;
  }

  label {
    position: relative;
    padding: 0.3rem 0.8rem;
    background: var(--surface-raised);
    color: var(--text);
    cursor: pointer;
    white-space: nowrap;
  }

  label + label {
    border-left: 1px solid #3a3f4d;
  }

  label.selected {
    background: #3d4a66;
  }

  label.disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* Visually hidden but still focusable. */
  input {
    position: absolute;
    opacity: 0;
    inset: 0;
    margin: 0;
    pointer-events: none;
  }

  label:has(input:focus-visible) {
    outline: 2px solid var(--text);
    outline-offset: -2px;
  }
</style>
