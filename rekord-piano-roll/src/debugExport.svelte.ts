import { invoke } from "@tauri-apps/api/core";
import { recording } from "./recording";

// Debug builds only: exports the range selected on the roll (Shift+drag) to a folder, to look into
// what the recognition did there -- the recorded audio as a WAV, the notes in the line format of
// `rekord_engine` (what the `chord_replay` example reads), and what the chord layers reported. The
// backend decides whether this build can (`debug_export_enabled`); release builds have no command.

export const debugExport = $state({
  enabled: false,
  /** The range selected on the roll, if any. */
  range: null as { startS: number; endS: number } | null,
  status: "idle" as "idle" | "busy" | "done" | "failed",
  /** The folder of the last export. */
  path: "",
});

export async function initDebugExport(): Promise<void> {
  try {
    debugExport.enabled = await invoke<boolean>("debug_export_enabled");
  } catch {
    debugExport.enabled = false; // no backend (the browser dev page), or a build without the command
  }
}

/** The roll's selection changed: what an earlier export covered is no longer what is selected. */
export function setExportRange(range: { startS: number; endS: number } | null): void {
  debugExport.range = range;
  debugExport.status = "idle";
}

export async function exportSelection(): Promise<void> {
  const range = debugExport.range;
  if (!debugExport.enabled || !range || debugExport.status === "busy") return;
  debugExport.status = "busy";
  try {
    const path = await invoke<string>("debug_export_range", { range: recording.exportRange(range) });
    debugExport.path = path;
    debugExport.status = "done";
    console.info(`exported to ${path}`);
    // The folder is somewhere under Documents: put its path where it can be pasted.
    await navigator.clipboard?.writeText(path).catch(() => {});
  } catch (err) {
    console.error(err);
    debugExport.status = "failed";
  }
}
