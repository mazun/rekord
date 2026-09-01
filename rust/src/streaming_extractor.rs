//! True incremental note extraction: run inference only on newly-completed
//! windows, stitching trimmed output onto a running posteriorgram. Mirrors
//! chord_poc/streaming_extractor.py.

use anyhow::Result;
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

fn trim_window_output(raw: &Array2<f32>) -> Array2<f32> {
    let t = raw.dim().0;
    raw.slice(ndarray::s![N_OLAP..t - N_OLAP, ..]).to_owned()
}

pub struct StreamingNoteExtractor<'a> {
    model: &'a mut OnnxNoteModel,
    buffer: Vec<f32>,
    window_start: usize,
    note_frames: Vec<Array2<f32>>,
    onset_frames: Vec<Array2<f32>>,
    total_frames: usize,
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
            let note_trimmed = trim_window_output(&out.note);
            let onset_trimmed = trim_window_output(&out.onset);
            self.total_frames += note_trimmed.dim().0;
            self.note_frames.push(note_trimmed);
            self.onset_frames.push(onset_trimmed);
            self.window_start += HOP_SIZE;
            completed += 1;
        }
        Ok(completed)
    }

    /// Pad any leftover buffered audio (shorter than one full window) with
    /// zeros and process it as a final window, mirroring basic_pitch's own
    /// offline window_audio_file (which pads a file's final short window
    /// rather than dropping it). Intended for a one-shot, mostly-real
    /// clip -- e.g. file-tail reconciliation -- NOT for a live stream: a
    /// live extractor's leftover is usually much shorter relative to the
    /// window, so padding it showed the model mostly-synthetic silence and
    /// suppressed real notes there (verified empirically in the Python
    /// prototype). Here the leftover is bounded by construction (the tail
    /// clip is <= one window long to begin with), so this stays close to
    /// basic_pitch's own validated offline behavior.
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
        model_output_to_notes(&frames, &onsets, 0.5, 0.3, min_note_len_frames)
    }
}
