//! Beat This! small0 (MIT, CPJKU) and its log-mel frontend as ONNX, CPU only.
//! Exported by harmonica-repro/beattrack/export_onnx.py; copy the two files into
//! core/models/ (not downloaded by build.rs, and core/models is gitignored).

use anyhow::{anyhow, Result};
use ndarray::{Array2, Array3, ArrayView2};
use ort::ep::cpu::CPU;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;

const MEL_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/models/logmel.onnx");
const NET_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/models/beatthis_small0.onnx");

pub const SAMPLE_RATE: usize = 22050;
pub const N_FFT: usize = 1024;
pub const HOP: usize = 441;
pub const N_MELS: usize = 128;

pub struct BeatModel {
    mel: Session,
    net: Session,
}

fn session(path: &str, threads: usize) -> Result<Session> {
    Session::builder()
        .map_err(|e| anyhow!("{e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow!("{e}"))?
        .with_intra_threads(threads)
        .map_err(|e| anyhow!("{e}"))?
        .with_inter_threads(1)
        .map_err(|e| anyhow!("{e}"))?
        .with_execution_providers([CPU::default().build()])
        .map_err(|e| anyhow!("{e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow!("{e}"))
}

impl BeatModel {
    pub fn load(threads: usize) -> Result<Self> {
        Ok(Self {
            mel: session(MEL_PATH, threads)?,
            net: session(NET_PATH, threads)?,
        })
    }

    /// audio: mono 22050 Hz, no padding. Returns (1 + (n - 1024) / 441, 128) log-mel
    /// frames; frame i covers samples [441 i, 441 i + 1024).
    pub fn mel(&mut self, audio: &[f32]) -> Result<Array2<f32>> {
        let input =
            Tensor::from_array(([audio.len()], audio.to_vec())).map_err(|e| anyhow!("{e}"))?;
        let out = self
            .mel
            .run(ort::inputs!["audio" => input])
            .map_err(|e| anyhow!("{e}"))?;
        let (shape, data) = out["spect"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        Array2::from_shape_vec((shape[0] as usize, shape[1] as usize), data.to_vec())
            .map_err(|e| anyhow!("{e}"))
    }

    /// spect: (T, 128). Returns per-frame (P(beat incl. downbeat), P(downbeat)).
    pub fn net(&mut self, spect: ArrayView2<f32>) -> Result<(Vec<f32>, Vec<f32>)> {
        let t = spect.nrows();
        let input: Array3<f32> = spect
            .to_owned()
            .into_shape_with_order((1, t, N_MELS))
            .map_err(|e| anyhow!("{e}"))?;
        let input = Tensor::from_array(input).map_err(|e| anyhow!("{e}"))?;
        let out = self
            .net
            .run(ort::inputs!["spect" => input])
            .map_err(|e| anyhow!("{e}"))?;
        let (_, beat) = out["beat"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        let (_, down) = out["downbeat"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        Ok((beat.to_vec(), down.to_vec()))
    }
}

/// Golden test vectors written by harmonica-repro (`python -m beattrack.online golden`).
/// Override with BEAT_GOLDEN_DIR; tests that need them skip when the directory is missing.
#[cfg(test)]
pub(crate) fn golden_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::var("BEAT_GOLDEN_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../harmonica-repro/runs/beat/golden")
        });
    if dir.exists() {
        Some(dir)
    } else {
        eprintln!("skipping: no golden vectors at {}", dir.display());
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::{Array1, Ix1, Ix2};
    use ndarray_npy::NpzReader;

    #[test]
    fn matches_python_on_golden_stream() {
        let Some(dir) = golden_dir() else { return };
        let mut npz =
            NpzReader::new(std::fs::File::open(dir.join("stream_s00_90s.npz")).unwrap()).unwrap();
        let audio: Array1<f32> = npz
            .by_name::<ndarray::OwnedRepr<f32>, Ix1>("audio")
            .unwrap();
        let spect: Array2<f32> = npz
            .by_name::<ndarray::OwnedRepr<f32>, Ix2>("spect")
            .unwrap();
        let mut m = BeatModel::load(1).unwrap();

        // 512 zeros in front give Beat This!'s center=True framing (the tracker does this)
        let mut padded = vec![0.0f32; N_FFT / 2];
        padded.extend(audio.iter());
        let ours = m.mel(&padded).unwrap();
        assert_eq!(ours.nrows(), spect.nrows());
        let d = (&ours - &spect)
            .mapv(f32::abs)
            .fold(0.0f32, |a, &b| a.max(b));
        assert!(d < 1e-3, "mel max diff {d}");

        // the network on a 10 s window vs a fresh Python-free run is checked in beat_tracker;
        // here just shape and range
        let (b, dn) = m.net(spect.slice(ndarray::s![1000..1500, ..])).unwrap();
        assert_eq!((b.len(), dn.len()), (500, 500));
        assert!(b.iter().chain(dn.iter()).all(|&p| (0.0..=1.0).contains(&p)));
    }
}
