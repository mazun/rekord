// Release builds don't ship the ONNX models (~51 MB) in the installer: they download them into
// the app's local data directory (a portable copy's `data` folder, see `portable`) on first launch, from the pinned commits and checked against
// the pinned hashes in `rekord_core::model_files`, so an app update doesn't download unchanged
// weights again. Debug builds read the checkout's core/models/ (where build.rs put them).

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

use rekord_core::model_files::{self, ModelFile};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

// How often (in bytes) a download reports its progress.
const PROGRESS_STEP: u64 = 256 * 1024;

#[derive(Clone, Serialize)]
struct Progress {
    done_bytes: u64,
    total_bytes: u64,
}

// Makes sure every model file is in place, downloading the missing (or damaged) ones, and points
// `model_files` at them. While it downloads, it emits `model-download` progress events.
pub fn ensure(app: &AppHandle) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    // A reloaded webview asks again while the first call may still be downloading.
    static BUSY: Mutex<()> = Mutex::new(());
    let _busy = BUSY.lock().map_err(|e| e.to_string())?;

    let dir = match crate::portable::data_dir() {
        Some(dir) => dir.clone(),
        None => app.path().app_local_data_dir().map_err(|e| e.to_string())?,
    }
    .join("models");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let missing: Vec<&ModelFile> = model_files::all()
        .filter(|f| !is_in_place(&dir.join(f.name), f))
        .collect();
    let total_bytes = missing.iter().map(|f| f.size).sum();
    let mut done_bytes = 0;
    let report = |done_bytes| {
        let _ = app.emit(
            "model-download",
            Progress {
                done_bytes,
                total_bytes,
            },
        );
    };
    if !missing.is_empty() {
        report(0);
    }
    for file in missing {
        download(&dir, file, |n| report(done_bytes + n))?;
        done_bytes += file.size;
    }

    model_files::set_dir(dir);
    Ok(())
}

fn is_in_place(path: &Path, file: &ModelFile) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    meta.len() == file.size
        && std::fs::read(path).is_ok_and(|bytes| sha256_hex(&Sha256::digest(bytes)) == file.sha256)
}

// Writes `file` into `dir` through a `.part` file, renamed once its size and hash are checked.
fn download(dir: &Path, file: &ModelFile, mut progress: impl FnMut(u64)) -> Result<(), String> {
    let url = file.url();
    let fail = |e: &dyn std::fmt::Display| format!("downloading {url}: {e}");
    let path = dir.join(file.name);
    let tmp_path = dir.join(format!("{}.part", file.name));

    let result = (|| {
        let response = ureq::get(&url).call().map_err(|e| fail(&e))?;
        let mut reader = response.into_body().into_reader();
        let mut out = File::create(&tmp_path).map_err(|e| fail(&e))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0; 64 * 1024];
        let (mut done, mut reported) = (0u64, 0u64);
        loop {
            let n = reader.read(&mut buf).map_err(|e| fail(&e))?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > file.size {
                return Err(fail(&"larger than the pinned size"));
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| fail(&e))?;
            if done - reported >= PROGRESS_STEP {
                progress(done);
                reported = done;
            }
        }
        out.flush().map_err(|e| fail(&e))?;
        drop(out);
        if done != file.size {
            return Err(fail(&"smaller than the pinned size"));
        }
        if sha256_hex(&hasher.finalize()) != file.sha256 {
            return Err(fail(&"does not match the pinned SHA-256"));
        }
        progress(done);
        std::fs::rename(&tmp_path, &path).map_err(|e| fail(&e))
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

fn sha256_hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
