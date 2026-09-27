//! Harmonica (instrument-agnostic note transcription) ONNX models: a CQT
//! front-end (`cqt156.onnx`) plus one of three onset/sustain/offset
//! transcription networks (small/medium/large). Ported from
//! harmonica-repro's `harmonica/infer.py` + `harmonica/ort_backend.py` --
//! see that repo's `docs/phase5-report.md` for the executable spec this
//! follows. Streaming (window construction, block scheduling) lives in
//! `harmonica_stream.rs`; this module is just model loading + single-window
//! inference, mirroring `onnx_model.rs`/`beat_model.rs`'s split.

use anyhow::{anyhow, Result};
use ndarray::{Array2, ArrayView2};
use ort::ep::{cpu::CPU, directml::DirectML};
use ort::session::{builder::GraphOptimizationLevel, Session, SessionOutputs};
use ort::value::Tensor;

/// 16 kHz mono input -- fixed by how Harmonica was trained and baked into
/// the exported ONNX models' shapes below.
pub const SAMPLE_RATE: usize = 16000;
/// Samples between adjacent output frames (32 ms/frame; frame k is centred
/// on sample k*HOP).
pub const HOP: usize = 512;
/// Seconds per frame.
pub const FRAME_S: f32 = HOP as f32 / SAMPLE_RATE as f32;
/// Frames per model window (5 s). Fixed by the exported ONNX input shape and
/// by training (the model normalizes over the whole window).
pub const CHUNK: usize = 156;
/// One-sided CQT kernel context in samples, baked into the exported CQT
/// model (its longest kernel, for the lowest bin).
pub const MARGIN: usize = 15254;
pub const N_BINS: usize = 264;
pub const N_PITCH: usize = 88;
pub const MIN_MIDI: i32 = 21;
/// Samples/frames processed per streaming block: 4096 samples = 0.256 s.
pub const BLOCK_FRAMES: usize = 8;
/// `cqt156.onnx`'s fixed audio input length: (CHUNK-1)*HOP + 2*MARGIN + 1
/// samples of context around the CHUNK output frames.
pub const WINDOW_SAMPLES: usize = (CHUNK - 1) * HOP + 2 * MARGIN + 1;
/// How much of WINDOW_SAMPLES is real (past + "now") audio, i.e. everything
/// except the trailing MARGIN zeros standing in for unavailable future audio.
pub const HISTORY: usize = (CHUNK - 1) * HOP + MARGIN + 1;

const CQT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/models/cqt156.onnx");

/// Which of the three trained transcription networks to use. Bigger is more
/// accurate but slower (see docs/phase5-report.md's speed table in
/// harmonica-repro); `HarmonicaModel::load` always falls back to `Small` on
/// CPU regardless of the requested size (see its doc comment).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelSize {
    Small,
    Medium,
    Large,
}

impl ModelSize {
    /// The exported ONNX file's bare name (also the value of that file's own
    /// `size` custom-metadata field, checked in `load_on`).
    pub fn file_name(self) -> &'static str {
        match self {
            ModelSize::Small => "small_uni_300k.onnx",
            ModelSize::Medium => "medium_uni_300k_urmp.onnx",
            ModelSize::Large => "large_uni_300k_urmp.onnx",
        }
    }

    fn onnx_path(self) -> &'static str {
        match self {
            ModelSize::Small => concat!(env!("CARGO_MANIFEST_DIR"), "/models/small_uni_300k.onnx"),
            ModelSize::Medium => concat!(env!("CARGO_MANIFEST_DIR"), "/models/medium_uni_300k_urmp.onnx"),
            ModelSize::Large => concat!(env!("CARGO_MANIFEST_DIR"), "/models/large_uni_300k_urmp.onnx"),
        }
    }

    /// (th_on, th_sus) chosen on the f=8 dev set in harmonica-repro; used
    /// unchanged for any future_frames >= 8 (see phase5-report.md's
    /// threshold section -- re-picking per f changes dev-set OnPOff by
    /// <= 0.4 points).
    pub fn thresholds(self) -> (f32, f32) {
        match self {
            ModelSize::Small => (0.25, 0.45),
            ModelSize::Medium => (0.20, 0.45),
            ModelSize::Large => (0.20, 0.45),
        }
    }
}

impl std::str::FromStr for ModelSize {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "small" => Ok(ModelSize::Small),
            "medium" => Ok(ModelSize::Medium),
            "large" => Ok(ModelSize::Large),
            other => Err(anyhow!("unknown harmonica model size {other:?} (expected small/medium/large)")),
        }
    }
}

impl std::fmt::Display for ModelSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ModelSize::Small => "small",
            ModelSize::Medium => "medium",
            ModelSize::Large => "large",
        })
    }
}

/// Which execution provider a `HarmonicaModel` ended up running on --
/// reported to the UI so it can show why (e.g. DirectML unavailable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    DirectML,
    Cpu,
}

/// A loaded pair of ONNX Runtime sessions (CQT front-end + transcription
/// network) for one model size on one device. Batch is always 1: streaming
/// (live or replayed) processes one window at a time, and large on DirectML
/// is known to corrupt output with bigger batches (see harmonica-repro's
/// phase5-report.md "見つけた問題" -- Re:Kord never hits that path since it
/// never batches, but keep it that way).
pub struct HarmonicaModel {
    cqt: Session,
    net: Session,
    size: ModelSize,
    device: Device,
}

fn check_metadata(session: &Session, expected: &[(&str, String)], label: &str) -> Result<()> {
    let meta = session.metadata().map_err(|e| anyhow!("{e}"))?;
    for (key, want) in expected {
        match meta.custom(key) {
            Some(got) if &got == want => {}
            Some(got) => {
                return Err(anyhow!("{label}: ONNX metadata {key:?} = {got:?}, expected {want:?} (wrong model file?)"));
            }
            None => return Err(anyhow!("{label}: ONNX metadata is missing key {key:?} (wrong/old model file?)")),
        }
    }
    Ok(())
}

fn cpu_intra_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(4, 8)
}

fn build_session(path: &str, device: Device) -> Result<Session> {
    let builder =
        Session::builder().map_err(|e| anyhow!("{e}"))?.with_optimization_level(GraphOptimizationLevel::Level3).map_err(|e| anyhow!("{e}"))?;
    let mut builder = match device {
        // DirectML requires disabling the memory-pattern optimizer and
        // parallel execution mode; `error_on_failure()` on the DirectML
        // dispatch turns a failed EP registration into an Err here instead
        // of a silent fall-through to the CPU entry that follows it.
        Device::DirectML => builder
            .with_memory_pattern(false)
            .map_err(|e| anyhow!("{e}"))?
            .with_parallel_execution(false)
            .map_err(|e| anyhow!("{e}"))?
            .with_execution_providers([DirectML::default().build().error_on_failure(), CPU::default().build()])
            .map_err(|e| anyhow!("{e}"))?,
        Device::Cpu => builder
            .with_intra_threads(cpu_intra_threads())
            .map_err(|e| anyhow!("{e}"))?
            .with_inter_threads(1)
            .map_err(|e| anyhow!("{e}"))?
            .with_execution_providers([CPU::default().build()])
            .map_err(|e| anyhow!("{e}"))?,
    };
    builder.commit_from_file(path).map_err(|e| anyhow!("{e}"))
}

fn extract_2d(outputs: &SessionOutputs<'_>, name: &str) -> Result<Array2<f32>> {
    let (shape, data) = outputs[name].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
    // Shape is [batch, time, features]; batch is always 1 (see struct doc).
    let (t, f) = (shape[shape.len() - 2] as usize, shape[shape.len() - 1] as usize);
    Array2::from_shape_vec((t, f), data.to_vec()).map_err(|e| anyhow!("{e}"))
}

impl HarmonicaModel {
    /// Loads exactly `size` on exactly `device`, no fallback -- callers that
    /// want the "DirectML, else small on CPU" policy should use `load`.
    pub fn load_on(size: ModelSize, device: Device) -> Result<Self> {
        let cqt = build_session(CQT_PATH, device)?;
        check_metadata(
            &cqt,
            &[
                ("sample_rate", SAMPLE_RATE.to_string()),
                ("hop", HOP.to_string()),
                ("n_frames", CHUNK.to_string()),
                ("n_bins", N_BINS.to_string()),
                ("cqt_margin", MARGIN.to_string()),
                ("audio_len", WINDOW_SAMPLES.to_string()),
            ],
            "cqt156.onnx",
        )?;

        let net = build_session(size.onnx_path(), device)?;
        check_metadata(
            &net,
            &[
                ("size", size.to_string()),
                ("sample_rate", SAMPLE_RATE.to_string()),
                ("hop", HOP.to_string()),
                ("chunk", CHUNK.to_string()),
                ("n_bins", N_BINS.to_string()),
                ("n_pitch", N_PITCH.to_string()),
                ("cqt_margin", MARGIN.to_string()),
            ],
            size.file_name(),
        )?;

        let mut model = Self { cqt, net, size, device };

        // The first run of a session costs ~0.7-0.9s (graph optimization,
        // DirectML shader compilation, ...); do it now so it doesn't stall
        // the first real window of a live session. On DirectML a failure
        // here (e.g. a driver that advertises the EP but can't actually run
        // this graph) must surface as Err, not be swallowed -- it isn't.
        let warm_audio = vec![0f32; WINDOW_SAMPLES];
        let warm_cqt = model.cqt(&warm_audio)?;
        model.net(warm_cqt.view())?;

        Ok(model)
    }

    /// Loads `size` on DirectML when `allow_directml`, falling back to
    /// `Small` on CPU (never the originally-requested size) if DirectML
    /// isn't available or fails; goes straight to `Small` on CPU if
    /// `allow_directml` is false. Decided by the user: when DirectML can't
    /// be used, always run the cheapest model on CPU rather than trying to
    /// run the requested size there -- medium/large on CPU are too slow to
    /// keep up with the 256ms block budget (see phase5-report.md's speed
    /// table), so falling back to the same size would silently break
    /// real-time use instead of just degrading accuracy.
    pub fn load(size: ModelSize, allow_directml: bool) -> Result<Self> {
        if allow_directml {
            match Self::load_on(size, Device::DirectML) {
                Ok(model) => return Ok(model),
                Err(e) => eprintln!("HarmonicaModel: DirectML load failed ({e}); falling back to small on CPU"),
            }
        }
        Self::load_on(ModelSize::Small, Device::Cpu)
    }

    pub fn size(&self) -> ModelSize {
        self.size
    }

    pub fn device(&self) -> Device {
        self.device
    }

    /// audio: exactly WINDOW_SAMPLES mono 16kHz samples. Returns [CHUNK,
    /// N_BINS] |CQT| magnitudes (untrimmed -- callers zero out-of-range rows
    /// themselves, see `harmonica_stream.rs`).
    pub fn cqt(&mut self, audio: &[f32]) -> Result<Array2<f32>> {
        assert_eq!(audio.len(), WINDOW_SAMPLES, "harmonica cqt window must be exactly WINDOW_SAMPLES samples");
        let input = Tensor::from_array(([1, WINDOW_SAMPLES], audio.to_vec())).map_err(|e| anyhow!("{e}"))?;
        let outputs = self.cqt.run(ort::inputs!["audio" => input]).map_err(|e| anyhow!("{e}"))?;
        extract_2d(&outputs, "cqt_mag")
    }

    /// cqt_mag: [CHUNK, N_BINS]. Returns [onset, sustain, offset], each
    /// [CHUNK, N_PITCH] sigmoid probabilities (offset is unused by decoding,
    /// per decode.py, but returned for completeness/inspection).
    pub fn net(&mut self, cqt_mag: ArrayView2<f32>) -> Result<[Array2<f32>; 3]> {
        let (t, f) = cqt_mag.dim();
        // Flatten row-major regardless of the view's own memory layout
        // (it may be a slice of a larger array, not necessarily contiguous).
        let data: Vec<f32> = cqt_mag.outer_iter().flat_map(|row| row.into_iter().copied()).collect();
        let input = Tensor::from_array(([1, t, f], data)).map_err(|e| anyhow!("{e}"))?;
        let outputs = self.net.run(ort::inputs!["cqt_mag" => input]).map_err(|e| anyhow!("{e}"))?;
        let onset = extract_2d(&outputs, "onset")?;
        let sustain = extract_2d(&outputs, "sustain")?;
        let offset = extract_2d(&outputs, "offset")?;
        Ok([onset, sustain, offset])
    }
}
