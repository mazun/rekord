import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

// Checks GitHub Releases for a newer version once per launch (release builds only); the banner
// (UpdateBanner.svelte) offers it, and installing restarts the app.
export type UpdaterStatus = "none" | "available" | "installing" | "error";

export const updater = $state({
  status: "none" as UpdaterStatus,
  version: "",
  doneBytes: 0,
  totalBytes: 0,
});

let pending: Update | null = null;

export async function checkForUpdate(): Promise<void> {
  if (!import.meta.env.PROD) return;
  try {
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
