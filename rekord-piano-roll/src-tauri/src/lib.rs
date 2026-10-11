#[cfg(debug_assertions)]
mod debug_export;
mod midi_copy;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use rekord_core::audio_ring::{RecordingInfo, RingTap, SharedRing};
use rekord_core::key_estimator::{KeyEstimate, Mode};
use rekord_core::live_capture::CaptureTap;
use rekord_core::okarina_model::ModelChoice;
use rekord_core::pipeline::{
    KeyMode, LiveControls, NoteModelConfig, CAPTURE_HOP_S, DEFAULT_FUTURE_FRAMES,
    FUTURE_FRAME_CHOICES,
};
use rekord_core::playback::{self, PlaybackParams, PlaybackSession, SynthNote};
use rekord_core::spelling::Accidental;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct CaptureState {
    stop_flag: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
    // Lives across start_capture/stop_capture cycles (unlike stop_flag/
    // handle, which are per-session) so a key choice made before pressing
    // Start -- or left over from a previous session -- isn't lost.
    key_mode: Arc<Mutex<KeyMode>>,
    // Same lifetime as key_mode; changeable while a session runs (set_accidental).
    accidental: Arc<Mutex<Accidental>>,
}

// The playback of the recording (see `rekord_core::playback`), if any. The
// recording itself is the managed `SharedRing`: it outlives capture sessions
// (it is replaced when the next one begins), so it can be played after stop.
#[derive(Default)]
struct PlaybackState {
    session: Option<PlaybackSession>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    mutex.lock().map_err(|e| e.to_string())
}

// Dropping the session stops the audio and joins its thread; that is done after
// the state lock is released.
fn stop_playback(state: &Mutex<PlaybackState>) -> Result<(), String> {
    let session = lock(state)?.session.take();
    drop(session);
    Ok(())
}

// Runs `f` on the playback in progress, if it hasn't ended.
fn with_playback<T>(
    state: &Mutex<PlaybackState>,
    f: impl FnOnce(&PlaybackSession) -> Result<T, String>,
) -> Result<T, String> {
    let guard = lock(state)?;
    match guard.session.as_ref() {
        Some(session) if !session.is_finished() => f(session),
        _ => Err("no playback".into()),
    }
}

#[tauri::command]
fn start_capture(
    app: AppHandle,
    state: State<Mutex<CaptureState>>,
    playback_state: State<Mutex<PlaybackState>>,
    ring: State<SharedRing>,
    model_size: String,
    future_frames: usize,
    accidental: String,
) -> Result<(), String> {
    // A playing output would be captured by the loopback (and the capture
    // clears the recording being played anyway).
    stop_playback(&playback_state)?;

    let mut guard = lock(&state)?;
    if guard.handle.is_some() {
        return Ok(()); // already running
    }

    // Sanity checks on the frontend-supplied model settings -- server-side
    // insurance so a malformed value can never reach the pipeline, on top
    // of whatever validation the frontend's settings loaders already do.
    let choice = model_size
        .parse::<ModelChoice>()
        .unwrap_or(ModelChoice::Auto);
    let future_frames = if FUTURE_FRAME_CHOICES.contains(&future_frames) {
        future_frames
    } else {
        DEFAULT_FUTURE_FRAMES
    };
    let config = NoteModelConfig {
        choice,
        future_frames,
        allow_directml: true,
    };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_thread = Arc::clone(&stop_flag);
    let key_mode_thread = Arc::clone(&guard.key_mode);
    *lock(&guard.accidental)? = accidental.parse().unwrap_or_default();
    let accidental_thread = Arc::clone(&guard.accidental);
    let app_for_thread = app.clone();
    let tap: Box<dyn CaptureTap> = Box::new(RingTap::new(Arc::clone(&ring)));

    let handle = std::thread::spawn(move || {
        let result = rekord_core::pipeline::run_live_session(
            config,
            CAPTURE_HOP_S,
            None,
            LiveControls {
                stop: stop_flag_thread,
                key_mode: key_mode_thread,
                accidental: accidental_thread,
            },
            Some(tap),
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
    let guard = lock(&state)?;
    let mode = match tonic_pc {
        Some(pc) => KeyMode::Manual(KeyEstimate {
            tonic_pc: pc.rem_euclid(12),
            mode: Mode::Major,
        }),
        None => KeyMode::Auto,
    };
    *lock(&guard.key_mode)? = mode;
    Ok(())
}

// "auto" | "sharp" | "flat" -- how chord and key names spell black-key pitch classes.
// Takes effect immediately, mid-session included (the pipeline re-announces the
// displayed chord and key in the new spelling).
#[tauri::command]
fn set_accidental(state: State<Mutex<CaptureState>>, accidental: String) -> Result<(), String> {
    let guard = lock(&state)?;
    *lock(&guard.accidental)? = accidental
        .parse()
        .map_err(|e: anyhow::Error| e.to_string())?;
    Ok(())
}

// Returns what the recording now covers (`None` if nothing was ever recorded),
// which is what playback can use.
#[tauri::command]
fn stop_capture(
    state: State<Mutex<CaptureState>>,
    ring: State<SharedRing>,
) -> Result<Option<RecordingInfo>, String> {
    let (stop_flag, handle) = {
        let mut guard = lock(&state)?;
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
    let info = lock(&ring)?.as_ref().map(|r| r.info());
    Ok(info)
}

// The playback commands are `async` so that opening the output device and
// waiting for a fade don't block the main thread. `gen` is chosen by the
// frontend and tags every position report that follows from the command (see
// `PlaybackEvent`); `notes` are all the notes of the recording, `params`
// says how loud things are and how each part's notes sound.
#[tauri::command]
async fn playback_start(
    app: AppHandle,
    from_s: f64,
    notes: Vec<SynthNote>,
    params: PlaybackParams,
    gen: u32,
) -> Result<(), String> {
    let capture = app.state::<Mutex<CaptureState>>();
    let playback_state = app.state::<Mutex<PlaybackState>>();
    let ring = app.state::<SharedRing>();
    if lock(&capture)?.handle.is_some() {
        return Err("capture is running".into());
    }
    stop_playback(&playback_state)?;
    let emitter = app.clone();
    let session = playback::start(
        Arc::clone(&ring),
        notes,
        from_s,
        params,
        gen,
        move |event| {
            let _ = emitter.emit("playback-event", &event);
        },
    )
    .map_err(|e| format!("{e:#}"))?;
    lock(&playback_state)?.session = Some(session);
    Ok(())
}

// Returns the recording time playback will resume from.
#[tauri::command]
async fn playback_pause(playback_state: State<'_, Mutex<PlaybackState>>) -> Result<f64, String> {
    with_playback(&playback_state, |session| Ok(session.pause()))
}

// Plays from `pos_s`, resuming a paused playback. Fails once the recording has
// been played to its end; the frontend then starts a fresh playback instead.
#[tauri::command]
fn playback_seek(
    playback_state: State<Mutex<PlaybackState>>,
    pos_s: f64,
    gen: u32,
) -> Result<(), String> {
    with_playback(&playback_state, |session| {
        session.seek(pos_s, gen).map_err(|e| e.to_string())
    })
}

#[tauri::command]
fn playback_set_params(
    playback_state: State<Mutex<PlaybackState>>,
    params: PlaybackParams,
) -> Result<(), String> {
    // Nothing to change when nothing is playing.
    let _ = with_playback(&playback_state, |session| {
        session.set_params(&params);
        Ok(())
    });
    Ok(())
}

#[tauri::command]
async fn playback_stop(playback_state: State<'_, Mutex<PlaybackState>>) -> Result<(), String> {
    stop_playback(&playback_state)
}

// Writes the selected notes as a .mid file and drags it out of the window (the frontend calls this
// once a drag from the selection has moved). A sync command: it runs on the main thread, which the
// OS drag needs.
#[tauri::command]
fn drag_notes_as_midi(
    window: tauri::WebviewWindow,
    range: midi_copy::MidiRange,
) -> Result<(), String> {
    midi_copy::drag(&window, range)
}

// Whether this build can export a selected range of the recording for debugging
// (`debug_export_range`); the roll only offers the selection's export button when it can.
#[tauri::command]
fn debug_export_enabled() -> bool {
    cfg!(debug_assertions)
}

// Writes the audio, notes and chord-layer log of the selected range to a folder under Documents and
// returns its path. Debug builds only.
#[cfg(debug_assertions)]
#[tauri::command]
async fn debug_export_range(
    app: AppHandle,
    ring: State<'_, SharedRing>,
    range: debug_export::ExportRange,
) -> Result<String, String> {
    debug_export::export(&app, &ring, range)
}

#[cfg(not(debug_assertions))]
#[tauri::command]
async fn debug_export_range(_range: serde_json::Value) -> Result<String, String> {
    Err("the range export is only in debug builds".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(CaptureState::default()))
        .manage(Mutex::new(PlaybackState::default()))
        .manage::<SharedRing>(Arc::new(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            start_capture,
            stop_capture,
            set_key_mode,
            set_accidental,
            playback_start,
            playback_pause,
            playback_seek,
            playback_set_params,
            playback_stop,
            drag_notes_as_midi,
            debug_export_enabled,
            debug_export_range
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
