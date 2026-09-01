//! True incremental note extraction: run inference only on newly-completed
//! windows, stitching trimmed output onto a running posteriorgram. Mirrors
//! chord_poc/streaming_extractor.py.

use anyhow::{anyhow, Result};
use ndarray::{concatenate, Array2, Axis};

use crate::note_creation::{model_output_to_notes, NoteEvent};
use crate::onnx_model::OnnxNoteModel;

pub const AUDIO_SAMPLE_RATE: usize = 22050;
pub const FFT_HOP: usize = 256;
const AUDIO_WINDOW_LENGTH: usize = 2;
pub const AUDIO_N_SAMPLES: usize = AUDIO_SAMPLE_RATE * AUDIO_WINDOW_LENGTH - FFT_HOP;
pub const ANNOTATIONS_FPS: usize = AUDIO_SAMPLE_RATE / FFT_HOP; // 86, integer division

pub const N_OVERLAPPING_FRAMES: usize = 30;
pub const OVERLAP_LEN: usize = N_OVERLAPPING_FRAMES * FFT_HOP;
pub const HOP_SIZE: usize = AUDIO_N_SAMPLES - OVERLAP_LEN;
const N_OLAP: usize = N_OVERLAPPING_FRAMES / 2;

// A fresh extractor's buffer starts with OVERLAP_LEN/2 zeros already in it
// (see `new`). Feeding it more than this many real samples in one go means
// `push` will complete a window on its own before any caller gets a chance
// to call `finish` -- leaving `finish` to pad a second, mostly-synthetic
// window instead of the small, genuinely-bounded pad its doc comment
// promises. Callers doing one-shot tail reconciliation on a short real
// clip should keep the clip at or under this length.
pub const MAX_FRESH_EXTRACTOR_REAL_SAMPLES: usize = AUDIO_N_SAMPLES - OVERLAP_LEN / 2;

// Retain at most this many completed windows of history for note_events()
// to reprocess. Without a cap, note_frames/onset_frames grow for the
// entire session and every note_events() call re-runs the full
// note-formation algorithm (onset inference, peak-picking, the
// melodia-trick loop) over the ever-growing history -- cost per call grows
// with total elapsed time, and in `--live` mode that eventually exceeds
// the ~1.64s window cadence and starts backing up/dropping live audio
// upstream. 64 windows is ~105s of retained context, far more than
// CONFIRM_MARGIN_S (0.5s) needs for correctness.
const MAX_RETAINED_WINDOWS: usize = 64;

fn trim_window_output(raw: &Array2<f32>) -> Result<Array2<f32>> {
    let t = raw.dim().0;
    if t <= 2 * N_OLAP {
        return Err(anyhow!(
            "ONNX model produced {t} time frames per window, expected more than {} \
             (got models/nmp.onnx with an unexpected shape? try deleting rust/models/ \
             and rebuilding so it's re-downloaded)",
            2 * N_OLAP
        ));
    }
    Ok(raw.slice(ndarray::s![N_OLAP..t - N_OLAP, ..]).to_owned())
}

pub struct StreamingNoteExtractor<'a> {
    model: &'a mut OnnxNoteModel,
    buffer: Vec<f32>,
    window_start: usize,
    note_frames: Vec<Array2<f32>>,
    onset_frames: Vec<Array2<f32>>,
    total_frames: usize,
    // Frames dropped from the front of note_frames/onset_frames by the
    // MAX_RETAINED_WINDOWS cap. total_frames keeps counting every frame
    // ever produced (covered_seconds must stay monotonic/absolute), so
    // note_events() adds this back as a time offset for whatever's left.
    dropped_frames: usize,
}

impl<'a> StreamingNoteExtractor<'a> {
    pub fn new(model: &'a mut OnnxNoteModel) -> Self {
        Self {
            model,
            buffer: vec![0.0; OVERLAP_LEN / 2],
            window_start: 0,
            note_frames: Vec::new(),
            onset_frames: Vec::new(),
            total_frames: 0,
            dropped_frames: 0,
        }
    }

    pub fn covered_seconds(&self) -> f32 {
        self.total_frames as f32 / ANNOTATIONS_FPS as f32
    }

    /// Feed newly arrived audio samples. Returns how many windows completed.
    pub fn push(&mut self, new_audio: &[f32]) -> Result<usize> {
        self.buffer.extend_from_slice(new_audio);
        let mut completed = 0;
        while self.window_start + AUDIO_N_SAMPLES <= self.buffer.len() {
            let window = &self.buffer[self.window_start..self.window_start + AUDIO_N_SAMPLES];
            let out = self.model.predict(ndarray::ArrayView1::from(window))?;
            let note_trimmed = trim_window_output(&out.note)?;
            let onset_trimmed = trim_window_output(&out.onset)?;
            self.total_frames += note_trimmed.dim().0;
            self.note_frames.push(note_trimmed);
            self.onset_frames.push(onset_trimmed);
            self.window_start += HOP_SIZE;
            completed += 1;

            while self.note_frames.len() > MAX_RETAINED_WINDOWS {
                let dropped = self.note_frames.remove(0);
                self.onset_frames.remove(0);
                self.dropped_frames += dropped.dim().0;
            }
        }
        Ok(completed)
    }

    /// Pad any leftover buffered audio (shorter than one full window) with
    /// zeros and process it as a final window, mirroring basic_pitch's own
    /// offline window_audio_file (which pads a file's final short window
    /// rather than dropping it). Intended for a one-shot, mostly-real
    /// clip -- e.g. file-tail reconciliation, fed at most
    /// MAX_FRESH_EXTRACTOR_REAL_SAMPLES -- NOT for a live stream: a live
    /// extractor's leftover is usually much shorter relative to the
    /// window, so padding it showed the model mostly-synthetic silence and
    /// suppressed real notes there (verified empirically in the Python
    /// prototype).
    pub fn finish(&mut self) -> Result<usize> {
        let remaining = self.buffer.len() - self.window_start;
        if remaining == 0 {
            return Ok(0);
        }
        self.buffer.resize(self.window_start + AUDIO_N_SAMPLES, 0.0);
        self.push(&[])
    }

    pub fn note_events(&self) -> Vec<NoteEvent> {
        if self.note_frames.is_empty() {
            return Vec::new();
        }
        let note_views: Vec<_> = self.note_frames.iter().map(|a| a.view()).collect();
        let onset_views: Vec<_> = self.onset_frames.iter().map(|a| a.view()).collect();
        let frames = concatenate(Axis(0), &note_views).expect("concat note frames");
        let onsets = concatenate(Axis(0), &onset_views).expect("concat onset frames");

        let min_note_len_frames =
            (127.70 / 1000.0 * (AUDIO_SAMPLE_RATE as f32 / FFT_HOP as f32)).round() as i64;
        let notes = model_output_to_notes(&frames, &onsets, 0.5, 0.3, min_note_len_frames);

        if self.dropped_frames == 0 {
            return notes;
        }
        let offset_s = self.dropped_frames as f32 / ANNOTATIONS_FPS as f32;
        notes.into_iter().map(|(s, e, p, a)| (s + offset_s, e + offset_s, p, a)).collect()
    }
}
