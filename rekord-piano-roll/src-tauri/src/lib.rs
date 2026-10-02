use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use rekord_core::harmonica_model::ModelSize;
use rekord_core::key_estimator::{KeyEstimate, Mode};
use rekord_core::pipeline::{
    KeyMode, NoteModelConfig, CAPTURE_HOP_S, DEFAULT_FUTURE_FRAMES, FUTURE_FRAME_CHOICES,
};
use tauri::{AppHandle, Emitter, State};

#[derive(Default)]
struct CaptureState {
    stop_flag: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
    // Lives across start_capture/stop_capture cycles (unlike stop_flag/
    // handle, which are per-session) so a key choice made before pressing
    // Start -- or left over from a previous session -- isn't lost.
    key_mode: Arc<Mutex<KeyMode>>,
}

#[tauri::command]
fn start_capture(
    app: AppHandle,
    state: State<Mutex<CaptureState>>,
    model_size: String,
    future_frames: usize,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    if guard.handle.is_some() {
        return Ok(()); // already running
    }

    // Sanity checks on the frontend-supplied model settings -- server-side
    // insurance so a malformed value can never reach the pipeline, on top
    // of whatever validation the frontend's settings loaders already do.
    let size = model_size.parse::<ModelSize>().unwrap_or(ModelSize::Mix);
    let future_frames = if FUTURE_FRAME_CHOICES.contains(&future_frames) {
        future_frames
    } else {
        DEFAULT_FUTURE_FRAMES
    };
    let config = NoteModelConfig { size, future_frames, allow_directml: true };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_thread = Arc::clone(&stop_flag);
    let key_mode_thread = Arc::clone(&guard.key_mode);
    let app_for_thread = app.clone();

    let handle = std::thread::spawn(move || {
        let result = rekord_core::pipeline::run_live_session(
            config,
            CAPTURE_HOP_S,
            None,
            stop_flag_thread,
            key_mode_thread,
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

// `tonic_pc` is the target relative-major pitch class (0=C, 1=C#, ...),
// matching KeyEstimate::name()'s "X Major (Y Minor)" display -- `None`
// switches back to automatic estimation. See KeyMode's doc comment for why
// a manual pick doesn't need a separate major/minor choice.
#[tauri::command]
fn set_key_mode(state: State<Mutex<CaptureState>>, tonic_pc: Option<i32>) -> Result<(), String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    let mode = match tonic_pc {
        Some(pc) => KeyMode::Manual(KeyEstimate { tonic_pc: pc.rem_euclid(12), mode: Mode::Major }),
        None => KeyMode::Auto,
    };
    *guard.key_mode.lock().map_err(|e| e.to_string())? = mode;
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
        .invoke_handler(tauri::generate_handler![start_capture, stop_capture, set_key_mode])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
