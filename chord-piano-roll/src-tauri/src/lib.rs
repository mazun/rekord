use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tauri::{AppHandle, Emitter, State};

// Sanity clamp on the frontend-supplied latency setting -- server-side
// insurance so a malformed value (e.g. 0, which would break live_capture's
// resampler chunk sizing) can never reach the pipeline, on top of
// StreamingNoteExtractor::with_stride's own clamp.
const MIN_LATENCY_SECONDS: f32 = 0.05;
const MAX_LATENCY_SECONDS: f32 = 2.0;

#[derive(Default)]
struct CaptureState {
    stop_flag: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
}

#[tauri::command]
fn start_capture(
    app: AppHandle,
    state: State<Mutex<CaptureState>>,
    latency_seconds: f32,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    if guard.handle.is_some() {
        return Ok(()); // already running
    }

    let latency_seconds = latency_seconds.clamp(MIN_LATENCY_SECONDS, MAX_LATENCY_SECONDS);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_thread = Arc::clone(&stop_flag);
    let app_for_thread = app.clone();

    let handle = std::thread::spawn(move || {
        let result = chord_core::pipeline::run_live_session(
            latency_seconds, // hop_seconds (capture delivery granularity)
            latency_seconds, // stride_seconds (model window-advance) -- one
            // slider drives both, so lowering it actually shortens the
            // reported confirm latency instead of being capped by a
            // separately-fixed hop.
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
