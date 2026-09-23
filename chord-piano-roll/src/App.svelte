<script lang="ts">
  import { onMount } from "svelte";
  import { PianoRoll } from "./pianoroll";
  import { pipeline, connectPipeline, startCapture, stopCapture } from "./pipeline.svelte";
  import { settings, saveLatency } from "./settings.svelte";
  import { initKeyMenu, setPianorollVisible } from "./legacy";
  import pianorollToggleIconSvg from "./assets/pianoroll-toggle-icon.svg?raw";

  const TENSION_GROUPS = [
    { tensions: ["b9", "9", "#9"], alignRight: false },
    { tensions: ["11", "#11"], alignRight: true },
    { tensions: ["b13", "13"], alignRight: false },
  ];

  let view = $state<"main" | "settings">("main");
  let rolesEnabled = $state(true);
  let roll: PianoRoll | null = null;

  let appShell: HTMLElement;
  let keyLabel: HTMLElement;
  let keyMenu: HTMLElement;
  let pianorollToggleBtn: HTMLElement;
  let pianorollContainer: HTMLElement;

  // Stopping capture (if running) before navigating away keeps the piano
  // roll's clock/state consistent with what "Start" will set up fresh next
  // time -- the settings page is a separate view, not an overlay on top of
  // a still-running session.
  async function openSettings() {
    if (pipeline.status === "running") {
      await stopCapture();
    }
    view = "settings";
  }

  function togglePianoroll() {
    setPianorollVisible(!settings.pianorollVisible, appShell, pianorollToggleBtn);
  }

  // No separate checkbox -- the legend row itself is the button, and its
  // on/off state reads via dimming alone (see .roles-toggle.off).
  function toggleRoles() {
    rolesEnabled = !rolesEnabled;
    roll?.setRoleColoringEnabled(rolesEnabled);
  }

  onMount(() => {
    const disposeKeyMenu = initKeyMenu(keyLabel, keyMenu);

    // Reproduce a collapsed state from a previous session -- the window
    // itself always starts at tauri.conf.json's expanded default (800x600),
    // so this both shrinks it down to match and captures that starting
    // size as what "show" should restore to later, exactly like a real
    // collapse click would from here.
    if (!settings.pianorollVisible) {
      setPianorollVisible(false, appShell, pianorollToggleBtn);
    }

    let disposed = false;
    let unlisten: (() => void) | null = null;
    (async () => {
      const created = await PianoRoll.create(pianorollContainer);
      if (disposed) {
        created.destroy();
        return;
      }
      roll = created;
      roll.setRoleColoringEnabled(rolesEnabled);
      const off = await connectPipeline({
        onStarted: (totalLatencyS) => roll?.reset(totalLatencyS),
        onNote: (note) => roll?.addNote(note),
      });
      if (disposed) off();
      else unlisten = off;
    })();

    return () => {
      disposed = true;
      disposeKeyMenu();
      unlisten?.();
      roll?.destroy();
      roll = null;
    };
  });
</script>

<main class="app" bind:this={appShell}>
  <div id="main-view" class:hidden={view !== "main"}>
    <div class="toolbar">
      <button
        id="start-btn"
        disabled={pipeline.status !== "idle"}
        onclick={() => startCapture(settings.latencyMs)}>Start</button
      >
      <button id="stop-btn" disabled={pipeline.status !== "running"} onclick={stopCapture}
        >Stop</button
      >
      <button
        id="settings-btn"
        class="icon-btn"
        type="button"
        title="設定"
        aria-label="設定"
        onclick={openSettings}
      >
        ⚙
      </button>
    </div>
    <div class="key-row">
      <div
        id="key-label"
        role="button"
        tabindex="0"
        aria-haspopup="listbox"
        aria-expanded="false"
        bind:this={keyLabel}
      >
        {pipeline.keyName}
      </div>
      <div id="key-menu" class="key-menu hidden" role="listbox" bind:this={keyMenu}></div>
    </div>
    <div class="chord-row">
      <div class="chord-block">
        <div id="chord-label">{pipeline.chord}</div>
        <div id="roman-label">{pipeline.roman}</div>
      </div>
      <div class="tension-grid">
        {#each TENSION_GROUPS as group}
          <div class="tension-group" class:align-right={group.alignRight}>
            {#each group.tensions as tension}
              <div class="chip" class:lit={pipeline.tensions.includes(tension)}>{tension}</div>
            {/each}
          </div>
        {/each}
      </div>
      <!-- Injected as raw markup (via Vite's ?raw import) rather than an
           <img> -- an <img> can't be recolored with currentColor, and this
           icon needs to dim/brighten on hover and turn active-blue when the
           piano roll is hidden, same as every other icon button here. -->
      <button
        id="pianoroll-toggle-btn"
        class="icon-btn pianoroll-toggle-btn"
        class:active={!settings.pianorollVisible}
        type="button"
        title="ピアノロールの表示切替"
        aria-label="ピアノロールの表示切替"
        aria-pressed={!settings.pianorollVisible}
        bind:this={pianorollToggleBtn}
        onclick={togglePianoroll}
      >
        {@html pianorollToggleIconSvg}
      </button>
    </div>
    <div
      id="pianoroll-container"
      class:hidden={!settings.pianorollVisible}
      bind:this={pianorollContainer}
    >
      <button
        id="roles-toggle"
        class="roles-toggle"
        class:off={!rolesEnabled}
        type="button"
        title="クリックでメロディ/ベースの色分けを切り替え"
        onclick={toggleRoles}
      >
        <span class="legend-swatch melody"></span>Melody
        <span class="legend-swatch bass"></span>Bass
      </button>
    </div>
  </div>
  <div id="settings-view" class:hidden={view !== "settings"}>
    <div class="settings-page-header">
      <button
        id="settings-back-btn"
        class="icon-btn close-icon"
        type="button"
        title="閉じる"
        aria-label="閉じる"
        onclick={() => (view = "main")}
      ></button>
      <h2>設定</h2>
    </div>
    <div class="settings-row">
      <label for="latency-input">Latency: <span id="latency-value">{settings.latencyMs}</span>ms</label>
      <input
        type="range"
        id="latency-input"
        min="100"
        max="1640"
        step="50"
        bind:value={settings.latencyMs}
        onchange={saveLatency}
        disabled={pipeline.status !== "idle"}
      />
    </div>
  </div>
</main>
