//! Debug builds only (`mod` is behind `cfg(debug_assertions)` in lib.rs): writes the range the user
//! selected on the roll to a folder, to look into what the recognition did there.
//!
//! ```text
//! <Documents>/ReKord debug exports/export_<unix s>/
//!   audio.wav          the recorded audio of the window (16-bit stereo, the capture rate)
//!   raw/001.txt        the notes in rekord_engine's line format, times relative to the window:
//!                      `chord_replay <folder>/raw <out_dir>` re-runs the chord logic on them
//!   chord_layers.json  what each chord layer reported, the lane the roll showed, the keys
//!                      (times are the recording's, not shifted)
//!   meta.json          the window, the selection inside it, how the times relate
//! ```
//!
//! The window is the selection widened by the context the chord layers need (see
//! `EXPORT_PAD_*` in recording.ts), so a replay of the notes reproduces the selection too.

use std::fs;
use std::path::Path;

use rekord_core::audio_ring::SharedRing;
use rekord_core::pipeline::midi_to_name;
use rekord_core::playback::SynthNote;
use rekord_core::spelling::Accidental;
use serde::Deserialize;
use tauri::{AppHandle, Manager};

#[derive(Deserialize)]
pub struct ExportRange {
    window_start_s: f64,
    window_end_s: f64,
    selection_start_s: f64,
    selection_end_s: f64,
    notes: Vec<SynthNote>,
    chord_layers: serde_json::Value,
    lane: serde_json::Value,
    keys: serde_json::Value,
}

// How many frames are converted at a time, so a long window doesn't allocate it all as floats.
const READ_CHUNK_FRAMES: usize = 1 << 16;

pub fn export(app: &AppHandle, ring: &SharedRing, range: ExportRange) -> Result<String, String> {
    // `is_nan()` keeps NaN bounds rejected, as the negated `>` did.
    if range.window_end_s.is_nan()
        || range.window_start_s.is_nan()
        || range.window_end_s <= range.window_start_s
    {
        return Err("empty range".into());
    }
    let unix_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let base = app
        .path()
        .document_dir()
        .map_err(|e| e.to_string())?
        .join("ReKord debug exports");
    let dir = base.join(format!("export_{unix_s}"));
    fs::create_dir_all(dir.join("raw")).map_err(|e| format!("{}: {e}", dir.display()))?;

    let sample_rate = write_audio(
        ring,
        &dir.join("audio.wav"),
        range.window_start_s,
        range.window_end_s,
    )?;
    write_text(&dir.join("raw").join("001.txt"), &notes_text(&range))?;
    let layers = serde_json::json!({ "chord_layers": range.chord_layers, "lane": range.lane, "keys": range.keys });
    write_text(
        &dir.join("chord_layers.json"),
        &serde_json::to_string_pretty(&layers).map_err(|e| e.to_string())?,
    )?;
    let meta = serde_json::json!({
        "profile": "debug",
        "app_version": env!("CARGO_PKG_VERSION"),
        "exported_at_unix_s": unix_s,
        "sample_rate": sample_rate,
        "window_start_s": range.window_start_s,
        "window_end_s": range.window_end_s,
        "selection_start_s": range.selection_start_s,
        "selection_end_s": range.selection_end_s,
        "selection_in_window_s": [range.selection_start_s - range.window_start_s, range.selection_end_s - range.window_start_s],
        "times": "audio.wav and raw/001.txt start at window_start_s (t = recording time - window_start_s); chord_layers.json keeps recording time",
        "replay": "chord_replay <this folder>/raw <out_dir>   (REKORD_CHORD_TUNE=layer=confirmed for the 2 s layer)",
        "note_count": range.notes.len(),
    });
    write_text(
        &dir.join("meta.json"),
        &serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )?;
    Ok(dir.display().to_string())
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `[  0.060s ->   0.190s] note F♯4 (vel 0.57) [other]` -- rekord_engine's note line, which
/// `chord_replay` parses -- with the times relative to the window.
fn notes_text(range: &ExportRange) -> String {
    let mut notes: Vec<&SynthNote> = range.notes.iter().collect();
    notes.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
    let mut out = String::new();
    for n in notes {
        let start = (n.start_s - range.window_start_s).max(0.0);
        let end = (n.end_s - range.window_start_s).min(range.window_end_s - range.window_start_s);
        let part = n.part.name();
        let name = midi_to_name(i32::from(n.pitch), Accidental::Sharp);
        out.push_str(&format!(
            "[{start:7.3}s -> {end:7.3}s] note {name} (vel {:.2}) [{part}]\n",
            n.velocity
        ));
    }
    out
}

/// Writes the window of the recording as a 16-bit stereo WAV; returns the sample rate.
fn write_audio(ring: &SharedRing, path: &Path, start_s: f64, end_s: f64) -> Result<u32, String> {
    let guard = ring.lock().map_err(|e| e.to_string())?;
    let ring = guard.as_ref().ok_or("nothing recorded")?;
    let rate = ring.info().sample_rate;
    let first = (start_s.max(0.0) * f64::from(rate)).round() as u64;
    let last = ((end_s * f64::from(rate)).round() as u64).min(ring.end_frame());
    if last <= first {
        return Err("range is outside the recording".into());
    }
    let mut pcm: Vec<u8> = Vec::with_capacity((last - first) as usize * 4);
    let mut buf = vec![0.0f32; READ_CHUNK_FRAMES * 2];
    let mut frame = first;
    while frame < last {
        let want = ((last - frame) as usize).min(READ_CHUNK_FRAMES);
        let got = ring.read_f32(frame, &mut buf[..want * 2]);
        if got == 0 {
            break;
        }
        for sample in &buf[..got * 2] {
            let v = (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        frame += got as u64;
    }
    fs::write(path, wav_bytes(rate, &pcm)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(rate)
}

fn wav_bytes(rate: u32, pcm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // channels
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
    out.extend_from_slice(&4u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rekord_core::harmonica_model::Part;

    #[test]
    fn wav_header_describes_the_data() {
        let wav = wav_bytes(48000, &[0u8; 8]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 44);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 48000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(wav.len(), 52);
    }

    #[test]
    fn note_lines_are_relative_to_the_window_and_chord_replay_shaped() {
        let range = ExportRange {
            window_start_s: 10.0,
            window_end_s: 20.0,
            selection_start_s: 12.0,
            selection_end_s: 18.0,
            notes: vec![SynthNote {
                start_s: 9.0,
                end_s: 11.5,
                pitch: 66,
                velocity: 0.567,
                part: Part::Other,
            }],
            chord_layers: serde_json::Value::Null,
            lane: serde_json::Value::Null,
            keys: serde_json::Value::Null,
        };
        assert_eq!(
            notes_text(&range),
            "[  0.000s ->   1.500s] note F♯4 (vel 0.57) [other]\n"
        );
    }
}
