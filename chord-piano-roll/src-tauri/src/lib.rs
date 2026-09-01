use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tauri::{AppHandle, Emitter, State};

const HOP_SECONDS: f32 = 0.25;

#[derive(Default)]
struct CaptureState {
    stop_flag: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
}

#[tauri::command]
fn start_capture(app: AppHandle, state: State<Mutex<CaptureState>>) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    if guard.handle.is_some() {
        return Ok(()); // already running
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_thread = Arc::clone(&stop_flag);
    let app_for_thread = app.clone();

    let handle = std::thread::spawn(move || {
        let result = chord_core::pipeline::run_live_session(
            HOP_SECONDS,
            None,
            stop_flag_thread,
            |event| {
                let _ = app_for_thread.emit("pipeline-event", &event);
            },
        );
        if let Err(err) = result {
            let _ = app_for_thread.emit("pipeline-error", err.to_string());
        }
    });

    guard.stop_flag = Some(stop_flag);
    guard.handle = Some(handle);
    Ok(())
}

#[tauri::command]
fn stop_capture(state: State<Mutex<CaptureState>>) -> Result<(), String> {
    let (stop_flag, handle) = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        (guard.stop_flag.take(), guard.handle.take())
    };
    if let Some(flag) = stop_flag {
        flag.store(true, Ordering::Relaxed);
    }
    if let Some(handle) = handle {
        // Blocks briefly (up to about one hop) while the capture thread
        // notices the stop flag and flushes any pending notes/chord.
        let _ = handle.join();
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(CaptureState::default()))
        .invoke_handler(tauri::generate_handler![start_capture, stop_capture])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
