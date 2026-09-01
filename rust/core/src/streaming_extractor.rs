//! True incremental note extraction: run inference only on newly-completed
//! windows, stitching newly-confirmed frames onto a running posteriorgram.
//! Mirrors chord_poc/streaming_extractor.py, extended with a configurable
//! window-advance stride (see `with_stride`) decoupled from the model's
//! fixed edge-trim width.

use anyhow::{anyhow, Result};
use ndarray::{concatenate, s, Array2, ArrayView1, Axis};

use crate::note_creation::{model_output_to_notes, NoteEvent};
use crate::onnx_model::OnnxNoteModel;

pub const AUDIO_SAMPLE_RATE: usize = 22050;
pub const FFT_HOP: usize = 256;
const AUDIO_WINDOW_LENGTH: usize = 2;
pub const AUDIO_N_SAMPLES: usize = AUDIO_SAMPLE_RATE * AUDIO_WINDOW_LENGTH - FFT_HOP;
pub const ANNOTATIONS_FPS: usize = AUDIO_SAMPLE_RATE / FFT_HOP; // 86, integer division

pub const N_OVERLAPPING_FRAMES: usize = 30;
pub const OVERLAP_LEN: usize = N_OVERLAPPING_FRAMES * FFT_HOP;
// The model's default/legacy window-advance: exactly as far as the trimmed
// reliable interior is wide, so consecutive windows tile with no gap or
// overlap. Still the upper bound for a configurable stride -- anything
// larger would skip real audio.
pub const HOP_SIZE: usize = AUDIO_N_SAMPLES - OVERLAP_LEN;
const N_OLAP: usize = N_OVERLAPPING_FRAMES / 2;

// A stride finer than one FFT hop can't produce a different trimmed output
// (the model's output is itself frame-quantized at this granularity).
pub const MIN_STRIDE_SAMPLES: usize = FFT_HOP;
pub const MAX_STRIDE_SAMPLES: usize = HOP_SIZE;

// A fresh extractor's buffer starts with OVERLAP_LEN/2 zeros already in it
// (see `with_stride`). Feeding it more than this many real samples in one go
// means `push` will complete a window on its own before any caller gets a
// chance to call `finish` -- leaving `finish` to pad a second, mostly-
// synthetic window instead of the small, genuinely-bounded pad its doc
// comment promises. Callers doing one-shot tail reconciliation on a short
// real clip should keep the clip at or under this length.
pub const MAX_FRESH_EXTRACTOR_REAL_SAMPLES: usize = AUDIO_N_SAMPLES - OVERLAP_LEN / 2;

// Cap how many confirmed frames' worth of history note_events() reprocesses
// on every call (bounds its cost -- without a cap, cost grows with total
// elapsed session time, since it fully re-runs onset inference/peak-picking/
// the melodia-trick loop over everything retained). This is a time-based
// cap rather than a window-count one because a configurable stride means
// windows no longer contribute a fixed amount of new data each time. 3.3s
// matches what MAX_RETAINED_WINDOWS=2 retained under the old scheme (the
// only actual correctness requirement is keeping enough lookback for a note
// sustained across a window boundary to have its true end resolved by the
// next window's data -- there's no benefit to retaining more).
const MAX_RETAINED_SECONDS: f32 = 3.3;

struct FrameChunk {
    note: Array2<f32>,
    onset: Array2<f32>,
    // Absolute time (seconds from the start of real audio, i.e. already
    // adjusted for the buffer's initial zero-padding) of this chunk's row 0.
    start_time_s: f32,
}

impl FrameChunk {
    fn len(&self) -> usize {
        self.note.dim().0
    }
}

pub struct StreamingNoteExtractor<'a> {
    model: &'a mut OnnxNoteModel,
    buffer: Vec<f32>,
    window_start: usize,
    stride_samples: usize,
    // Absolute buffer-index position up to which frames have already been
    // appended to `chunks` -- same coordinate space as `window_start` (i.e.
    // includes the initial zero-padding offset; only converted to a
    // real-audio-relative time once, when a chunk's start_time_s is set).
    confirmed_up_to_sample: usize,
    chunks: Vec<FrameChunk>,
    retained_frames: usize,
}

impl<'a> StreamingNoteExtractor<'a> {
    pub fn new(model: &'a mut OnnxNoteModel) -> Self {
        Self::with_stride(model, HOP_SIZE)
    }

    /// Like `new`, but advances the analysis window by `stride_samples`
    /// instead of the legacy `HOP_SIZE` on every completed window --
    /// clamped to `[MIN_STRIDE_SAMPLES, MAX_STRIDE_SAMPLES]` so a caller
    /// can never pass a value that would skip audio (too large) or that's
    /// meaningless given the model's own frame quantization (too small).
    pub fn with_stride(model: &'a mut OnnxNoteModel, stride_samples: usize) -> Self {
        let stride_samples = stride_samples.clamp(MIN_STRIDE_SAMPLES, MAX_STRIDE_SAMPLES);
        Self {
            model,
            buffer: vec![0.0; OVERLAP_LEN / 2],
            window_start: 0,
            stride_samples,
            confirmed_up_to_sample: 0,
            chunks: Vec::new(),
            retained_frames: 0,
        }
    }

    /// The stride actually in effect (post-clamp) -- use this, not whatever
    /// was originally requested, when deriving a latency figure to report
    /// back to a caller.
    pub fn stride_samples(&self) -> usize {
        self.stride_samples
    }

    /// How much real audio time has been reliably processed so far.
    ///
    /// Derived from `confirmed_up_to_sample` (an exact sample position),
    /// *not* from `total_frames` (a count of discrete FFT_HOP-sized output
    /// rows). Those two disagree whenever `stride_samples` isn't an exact
    /// multiple of FFT_HOP: each window's frame-quantized delta rounds down
    /// via `div_ceil` to avoid re-processing/duplicating a frame, silently
    /// dropping the sub-frame remainder from `total_frames` every time.
    /// That remainder is tiny per window, but at a small stride it
    /// accumulates fast -- confirmed with a real reproduction: at a 100ms
    /// stride, `total_frames`-derived coverage fell behind real elapsed
    /// audio by about 7ms per window, reached the ~3.3s retention cap
    /// (`MAX_RETAINED_SECONDS`) in well under a minute of continuous
    /// audio, and from that point on notes were evicted from the retained
    /// buffer before NoteStitcher's confirm threshold (which reads this
    /// value) ever caught up to them -- i.e. notes silently stopped being
    /// confirmed. `confirmed_up_to_sample` has no such rounding loss (it's
    /// advanced by the caller's exact `stride_samples` every window), so
    /// basing this on it instead is both the fix and more precise in
    /// general, including at the legacy default stride (which has the
    /// same underlying imprecision, just small enough to not have been
    /// noticed at that cadence).
    pub fn covered_seconds(&self) -> f32 {
        self.confirmed_up_to_sample.saturating_sub(OVERLAP_LEN / 2) as f32 / AUDIO_SAMPLE_RATE as f32
    }

    /// Feed newly arrived audio samples. Returns how many windows completed.
    pub fn push(&mut self, new_audio: &[f32]) -> Result<usize> {
        self.buffer.extend_from_slice(new_audio);
        let mut completed = 0;
        while self.window_start + AUDIO_N_SAMPLES <= self.buffer.len() {
            let window = &self.buffer[self.window_start..self.window_start + AUDIO_N_SAMPLES];
            let out = self.model.predict(ArrayView1::from(window))?;
            let t = out.note.dim().0;
            if t <= 2 * N_OLAP {
                return Err(anyhow!(
                    "ONNX model produced {t} time frames per window, expected more than {} \
                     (got models/nmp.onnx with an unexpected shape? try deleting rust/models/ \
                     and rebuilding so it's re-downloaded)",
                    2 * N_OLAP
                ));
            }
            let reliable_local_end = t - N_OLAP;

            // Smallest local frame index in this window whose absolute
            // position is at or past what's already confirmed -- i.e. the
            // start of what's genuinely new here. When strides overlap
            // windows heavily this is often well past N_OLAP.
            let first_new_local = if self.confirmed_up_to_sample <= self.window_start {
                N_OLAP
            } else {
                let need = self.confirmed_up_to_sample - self.window_start;
                N_OLAP.max(need.div_ceil(FFT_HOP))
            }
            .min(reliable_local_end);

            if first_new_local < reliable_local_end {
                let note_delta = out.note.slice(s![first_new_local..reliable_local_end, ..]).to_owned();
                let onset_delta = out.onset.slice(s![first_new_local..reliable_local_end, ..]).to_owned();

                let start_sample = self.window_start + first_new_local * FFT_HOP;
                let start_time_s =
                    start_sample.saturating_sub(OVERLAP_LEN / 2) as f32 / AUDIO_SAMPLE_RATE as f32;

                let n = note_delta.dim().0;
                self.retained_frames += n;
                self.confirmed_up_to_sample = self.window_start + reliable_local_end * FFT_HOP;
                self.chunks.push(FrameChunk { note: note_delta, onset: onset_delta, start_time_s });
            }

            self.window_start += self.stride_samples;
            completed += 1;

            let max_retained_frames = (MAX_RETAINED_SECONDS * ANNOTATIONS_FPS as f32).round() as usize;
            while self.retained_frames > max_retained_frames && self.chunks.len() > 1 {
                let dropped = self.chunks.remove(0);
                self.retained_frames -= dropped.len();
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
        if self.chunks.is_empty() {
            return Vec::new();
        }
        let note_views: Vec<_> = self.chunks.iter().map(|c| c.note.view()).collect();
        let onset_views: Vec<_> = self.chunks.iter().map(|c| c.onset.view()).collect();
        let frames = concatenate(Axis(0), &note_views).expect("concat note frames");
        let onsets = concatenate(Axis(0), &onset_views).expect("concat onset frames");

        let mut times_s = Vec::with_capacity(frames.dim().0);
        for chunk in &self.chunks {
            for row in 0..chunk.len() {
                times_s.push(chunk.start_time_s + row as f32 * FFT_HOP as f32 / AUDIO_SAMPLE_RATE as f32);
            }
        }

        let min_note_len_frames =
            (127.70 / 1000.0 * (AUDIO_SAMPLE_RATE as f32 / FFT_HOP as f32)).round() as i64;
        model_output_to_notes(&frames, &onsets, 0.5, 0.3, min_note_len_frames, &times_s)
    }
}
