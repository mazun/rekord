import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// The ONNX models a release build downloads on first launch (see the backend's
// model_download.rs). Capture can't start until they are "ready".
export type ModelsStatus = "checking" | "downloading" | "ready" | "error";

export const models = $state({
  status: "checking" as ModelsStatus,
  doneBytes: 0,
  totalBytes: 0,
  error: "",
});

interface Progress {
  done_bytes: number;
  total_bytes: number;
}

// Resolves to whether the models are ready. Safe to call again after an error (retries the
// files that are still missing).
export async function ensureModels(): Promise<boolean> {
  models.status = "checking";
  models.error = "";
  const unlisten = await listen<Progress>("model-download", (event) => {
    models.status = "downloading";
    models.doneBytes = event.payload.done_bytes;
    models.totalBytes = event.payload.total_bytes;
  });
  try {
    await invoke("ensure_models");
    models.status = "ready";
    return true;
  } catch (err) {
    console.error(err);
    models.error = String(err);
    models.status = "error";
    return false;
  } finally {
    unlisten();
  }
}
