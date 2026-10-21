//! ADTOF Frame_RNN (Zehren et al., CC BY-NC-SA 4.0) as ONNX, CPU only: drum onset
//! activations from a full mix. The ONNX conversion is a community one
//! (huggingface.co/thebluescreen/adtof-drums-onnx); it takes mono 44.1 kHz audio and
//! includes madmom's log-filtered spectrogram. DirectML is much slower than the CPU
//! for this graph (30 s of audio: 6 s vs 0.12 s), so only the CPU provider is used.

use anyhow::{anyhow, Result};
use ort::session::Session;
use ort::value::Tensor;

use crate::beat_model;

const MODEL_FILE: &str = "adtof_frame_rnn.onnx";

pub const SAMPLE_RATE: usize = 44100;
/// Samples per activation frame (100 frames per second).
pub const HOP: usize = 441;
pub const FRAMES_PER_SECOND: f32 = 100.0;
pub const N_CLASSES: usize = 5;

pub struct DrumModel {
    session: Session,
}

impl DrumModel {
    pub fn load(threads: usize) -> Result<Self> {
        Ok(Self {
            session: beat_model::session(MODEL_FILE, threads)?,
        })
    }

    /// audio: mono 44.1 kHz. Returns `ceil(len / 441)` frames of sigmoid activations; frame `i`
    /// is centred on sample `441 i`.
    pub fn activations(&mut self, audio: &[f32]) -> Result<Vec<[f32; N_CLASSES]>> {
        let input =
            Tensor::from_array(([1, audio.len()], audio.to_vec())).map_err(|e| anyhow!("{e}"))?;
        let out = self
            .session
            .run(ort::inputs!["waveform" => input])
            .map_err(|e| anyhow!("{e}"))?;
        let (shape, data) = out["activations"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        if shape.len() != 3 || shape[2] as usize != N_CLASSES {
            return Err(anyhow!("unexpected drum activation shape {shape:?}"));
        }
        Ok(data.as_chunks::<N_CLASSES>().0.to_vec())
    }
}
