import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

// Checks GitHub Releases for a newer version once per launch (release builds only); the notice
// (AppNotices.svelte) offers it, and installing restarts the app. The portable (zip) copy can't
// install updates (the updater runs the installer), so its notice opens the release page instead.
export type UpdaterStatus = "none" | "available" | "installing" | "error";

const RELEASE_PAGE = "https://github.com/mazun/rekord/releases/latest";

export const updater = $state({
  status: "none" as UpdaterStatus,
  version: "",
  portable: false,
  doneBytes: 0,
  totalBytes: 0,
});

let pending: Update | null = null;

export async function checkForUpdate(): Promise<void> {
  if (!import.meta.env.PROD) return;
  try {
    updater.portable = await invoke<boolean>("is_portable");
    pending = await check();
  } catch (err) {
    // Offline, or no release yet: try again next launch.
    console.warn("update check failed", err);
    return;
  }
  if (pending) {
    updater.version = pending.version;
    updater.status = "available";
  }
}

export async function installUpdate(): Promise<void> {
  if (!pending) return;
  if (updater.portable) {
    await openUrl(RELEASE_PAGE);
    updater.status = "none";
    return;
  }
  updater.status = "installing";
  updater.doneBytes = 0;
  updater.totalBytes = 0;
  try {
    await pending.downloadAndInstall((event) => {
      if (event.event === "Started") updater.totalBytes = event.data.contentLength ?? 0;
      else if (event.event === "Progress") updater.doneBytes += event.data.chunkLength;
    });
    // On Windows the installer has already closed the app by now; this is for the other OSes.
    await relaunch();
  } catch (err) {
    console.error(err);
    updater.status = "error";
  }
}

export function dismissUpdate(): void {
  updater.status = "none";
}
