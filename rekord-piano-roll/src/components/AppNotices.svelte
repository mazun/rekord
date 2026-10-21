<script lang="ts">
  import { t } from "../i18n.svelte";
  import { ensureModels, models } from "../models.svelte";
  import { pipeline, startCapture } from "../pipeline.svelte";
  import { dismissUpdate, installUpdate, updater } from "../updater.svelte";

  // Fixed to the bottom of the window, out of the layout, so it doesn't disturb the collapsed
  // window's measured size (windowSize.ts).
  const mb = (bytes: number) => (bytes / 1e6).toFixed(0);
  const percent = (done: number, total: number) => (total > 0 ? Math.min(100, (100 * done) / total) : 0);

  async function retryModels() {
    if ((await ensureModels()) && pipeline.status === "idle") await startCapture();
  }
</script>

{#if models.status === "downloading"}
  <div class="notice" role="status">
    <span>{t("notice.modelsDownloading")} {mb(models.doneBytes)} / {mb(models.totalBytes)} MB</span>
    <div class="bar"><div style:width="{percent(models.doneBytes, models.totalBytes)}%"></div></div>
  </div>
{:else if models.status === "error"}
  <div class="notice" role="alert">
    <span>{t("notice.modelsFailed")}</span>
    <span class="detail" title={models.error}>{models.error}</span>
    <button type="button" onclick={retryModels}>{t("notice.retry")}</button>
  </div>
{:else if updater.status === "available"}
  <div class="notice" role="status">
    <span>{t("notice.updateAvailable").replace("{version}", updater.version)}</span>
    <button type="button" class="primary" onclick={installUpdate}>{t("notice.updateInstall")}</button>
    <button type="button" onclick={dismissUpdate}>{t("notice.later")}</button>
  </div>
{:else if updater.status === "installing"}
  <div class="notice" role="status">
    <span>{t("notice.updateDownloading")}</span>
    <div class="bar"><div style:width="{percent(updater.doneBytes, updater.totalBytes)}%"></div></div>
  </div>
{:else if updater.status === "error"}
  <div class="notice" role="alert">
    <span>{t("notice.updateFailed")}</span>
    <button type="button" onclick={dismissUpdate}>{t("notice.close")}</button>
  </div>
{/if}

<style>
  .notice {
    position: fixed;
    left: 50%;
    bottom: 0.8rem;
    transform: translateX(-50%);
    z-index: 10;
    display: flex;
    align-items: center;
    gap: 0.7rem;
    max-width: calc(100vw - 2rem);
    padding: 0.5rem 0.8rem;
    font-size: 0.85rem;
    white-space: nowrap;
    background: var(--surface-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 40%);
  }

  .detail {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text-dim);
  }

  .bar {
    flex: none;
    width: 8rem;
    height: 4px;
    background: var(--border);
    border-radius: 2px;
    overflow: hidden;
  }

  .bar > div {
    height: 100%;
    background: var(--text-dim);
  }

  button {
    flex: none;
    padding: 0.2rem 0.6rem;
    font: inherit;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
  }

  button.primary {
    background: #2f5fa8;
    border-color: #2f5fa8;
  }
</style>
