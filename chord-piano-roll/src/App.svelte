<script lang="ts">
  import { onMount } from "svelte";
  import { initApp } from "./legacy";

  // Phase 0 of the Svelte migration: the markup lives here, but all
  // behavior still runs through the original imperative wiring in
  // legacy.ts, unchanged. Later phases move state and handlers into
  // components one at a time.
  onMount(() => {
    initApp();
  });
</script>

<main class="app">
  <div id="main-view">
    <div class="toolbar">
      <button id="start-btn">Start</button>
      <button id="stop-btn" disabled>Stop</button>
      <button
        id="settings-btn"
        class="icon-btn"
        type="button"
        title="設定"
        aria-label="設定"
      >
        ⚙
      </button>
    </div>
    <div class="key-row">
      <div id="key-label" role="button" tabindex="0" aria-haspopup="listbox" aria-expanded="false">--</div>
      <div id="key-menu" class="key-menu hidden" role="listbox"></div>
    </div>
    <div class="chord-row">
      <div class="chord-block">
        <div id="chord-label">--</div>
        <div id="roman-label"></div>
      </div>
      <div class="tension-grid">
        <div class="tension-group">
          <div class="chip" data-tension="b9">b9</div>
          <div class="chip" data-tension="9">9</div>
          <div class="chip" data-tension="#9">#9</div>
        </div>
        <div class="tension-group align-right">
          <div class="chip" data-tension="11">11</div>
          <div class="chip" data-tension="#11">#11</div>
        </div>
        <div class="tension-group">
          <div class="chip" data-tension="b13">b13</div>
          <div class="chip" data-tension="13">13</div>
        </div>
      </div>
      <button
        id="pianoroll-toggle-btn"
        class="icon-btn pianoroll-toggle-btn"
        type="button"
        title="ピアノロールの表示切替"
        aria-label="ピアノロールの表示切替"
        aria-pressed="false"
      ></button>
    </div>
    <div id="pianoroll-container">
      <button id="roles-toggle" class="roles-toggle" type="button" title="クリックでメロディ/ベースの色分けを切り替え">
        <span class="legend-swatch melody"></span>Melody
        <span class="legend-swatch bass"></span>Bass
      </button>
    </div>
  </div>
  <div id="settings-view" class="hidden">
    <div class="settings-page-header">
      <button
        id="settings-back-btn"
        class="icon-btn close-icon"
        type="button"
        title="閉じる"
        aria-label="閉じる"
      ></button>
      <h2>設定</h2>
    </div>
    <div class="settings-row">
      <label for="latency-input"
        >Latency: <span id="latency-value">250</span>ms</label
      >
      <input
        type="range"
        id="latency-input"
        min="100"
        max="1640"
        step="50"
        value="250"
      />
    </div>
  </div>
</main>
