//! GPU-capable ONNX Runtime wrapper for the basic-pitch note model. Mirrors
//! chord_poc/onnx_model.py: prefer DirectML, fall back to CPU.

use anyhow::{anyhow, Result};
use ndarray::{Array2, Array3, ArrayView1};
use ort::ep::{cpu::CPU, directml::DirectML};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;

const INPUT_NAME: &str = "serving_default_input_2:0";
const NOTE_OUTPUT: &str = "StatefulPartitionedCall:1";
const ONSET_OUTPUT: &str = "StatefulPartitionedCall:2";

// Embed this crate's own build-time location rather than resolving
// "models/nmp.onnx" relative to the current working directory at
// runtime -- the model must be found the same way regardless of whether
// it's launched from rust/, rust/cli/, a Tauri app's own working
// directory, or anywhere else a binary linking this crate happens to run
// from. build.rs downloads the model into this same directory.
const MODEL_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/models/nmp.onnx");

pub struct OnnxNoteModel {
    session: Session,
}

pub struct WindowOutput {
    pub note: Array2<f32>,
    pub onset: Array2<f32>,
}

impl OnnxNoteModel {
    pub fn load() -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| anyhow!("{e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("{e}"))?
            .with_execution_providers([DirectML::default().build(), CPU::default().build()])
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_file(MODEL_PATH)
            .map_err(|e| anyhow!("{e}"))?;
        Ok(Self { session })
    }

    /// window: AUDIO_N_SAMPLES mono samples. Returns (n_times, n_freqs)
    /// note/onset activation matrices for this one window (untrimmed).
    pub fn predict(&mut self, window: ArrayView1<f32>) -> Result<WindowOutput> {
        let n = window.len();
        let input: Array3<f32> = Array3::from_shape_fn((1, n, 1), |(_, i, _)| window[i]);
        let input_value = Tensor::from_array(input).map_err(|e| anyhow!("{e}"))?;

        let outputs = self
            .session
            .run(ort::inputs![INPUT_NAME => input_value])
            .map_err(|e| anyhow!("{e}"))?;

        let (note_shape, note_data) = outputs[NOTE_OUTPUT]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        let (onset_shape, onset_data) = outputs[ONSET_OUTPUT]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;

        let (t, f) = (note_shape[1] as usize, note_shape[2] as usize);
        let note2d = Array2::from_shape_fn((t, f), |(i, j)| note_data[i * f + j]);

        let (t2, f2) = (onset_shape[1] as usize, onset_shape[2] as usize);
        let onset2d = Array2::from_shape_fn((t2, f2), |(i, j)| onset_data[i * f2 + j]);

        Ok(WindowOutput { note: note2d, onset: onset2d })
    }
}
