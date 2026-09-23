<script lang="ts">
  import { pipeline, startCapture, stopCapture } from "../pipeline.svelte";
  import { settings } from "../settings.svelte";

  let { onOpenSettings }: { onOpenSettings: () => void } = $props();

  // One toggle instead of separate Start/Stop buttons: the icon shows what
  // clicking will do (record while stopped, stop while recording), and
  // "stopping" keeps the stop icon (disabled) until stop_capture returns.
  const recording = $derived(pipeline.status === "running" || pipeline.status === "stopping");
  const label = $derived(recording ? "停止" : "録音開始");

  function toggleCapture() {
    if (pipeline.status === "running") stopCapture();
    else if (pipeline.status === "idle") startCapture(settings.latencyMs);
  }
</script>

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
  id="settings-btn"
  class="icon-btn"
  type="button"
  title="設定"
  aria-label="設定"
  onclick={onOpenSettings}
>
  <svg viewBox="0 0 512 512" aria-hidden="true">
    <path
      d="M499.453,210.004l-55.851-2.58c-5.102-0.23-9.608-3.395-11.546-8.103l-11.508-27.695
        c-1.937-4.728-0.997-10.145,2.455-13.914l37.668-41.332c4.718-5.188,4.546-13.205-0.421-18.182l-46.434-46.443
        c-4.986-4.967-13.003-5.159-18.2-0.412l-41.312,37.668c-3.778,3.443-9.206,4.402-13.924,2.436l-27.694-11.488
        c-4.718-1.946-7.864-6.454-8.094-11.565l-2.589-55.831C301.675,5.534,295.883,0,288.864,0h-65.708
        c-7.02,0-12.831,5.534-13.156,12.562l-2.571,55.831c-0.23,5.111-3.376,9.618-8.094,11.565L171.64,91.447
        c-4.737,1.966-10.165,1.007-13.924-2.436l-41.331-37.668c-5.198-4.746-13.215-4.564-18.201,0.412L51.769,98.198
        c-4.986,4.977-5.158,12.994-0.422,18.182l37.668,41.332c3.452,3.769,4.373,9.186,2.416,13.914l-11.469,27.695
        c-1.956,4.708-6.444,7.873-11.564,8.103l-55.832,2.58c-7.019,0.316-12.562,6.118-12.562,13.147v65.699
        c0,7.019,5.543,12.83,12.562,13.148l55.832,2.579c5.12,0.229,9.608,3.394,11.564,8.103l11.469,27.694
        c1.957,4.728,1.036,10.146-2.416,13.914l-37.668,41.313c-4.756,5.217-4.564,13.224,0.403,18.201l46.471,46.443
        c4.967,4.977,12.965,5.15,18.182,0.422l41.312-37.677c3.759-3.443,9.207-4.392,13.924-2.435l27.694,11.478
        c4.719,1.956,7.864,6.464,8.094,11.575l2.571,55.831c0.325,7.02,6.136,12.562,13.156,12.562h65.708
        c7.02,0,12.812-5.542,13.138-12.562l2.589-55.831c0.23-5.111,3.376-9.619,8.094-11.575l27.694-11.478
        c4.718-1.957,10.146-1.008,13.924,2.435l41.312,37.677c5.198,4.728,13.215,4.555,18.2-0.422l46.434-46.443
        c4.967-4.977,5.139-12.984,0.421-18.201l-37.668-41.313c-3.452-3.768-4.412-9.186-2.455-13.914l11.508-27.694
        c1.937-4.709,6.444-7.874,11.546-8.103l55.851-2.579c7.019-0.318,12.542-6.129,12.542-13.148v-65.699
        C511.995,216.122,506.472,210.32,499.453,210.004z M256.01,339.618c-46.164,0-83.622-37.438-83.622-83.612
        c0-46.184,37.458-83.622,83.622-83.622s83.602,37.438,83.602,83.622C339.612,302.179,302.174,339.618,256.01,339.618z"
    />
  </svg>
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

  .capture-btn {
    left: 0;
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

  .capture-btn svg {
    display: block;
    width: 1.1rem;
    height: 1.1rem;
    fill: currentColor;
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

  #settings-btn svg {
    display: block;
    width: 1.2rem;
    height: 1.2rem;
    fill: currentColor;
  }

  #settings-btn:hover {
    background: none;
  }
</style>
