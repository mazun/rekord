//! Okarina (note transcription for music with vocals and drums; an unofficial
//! reimplementation/derivative of Harmonica) ONNX models: a CQT front-end
//! (`cqt156.onnx`) plus one onset/sustain/offset transcription network in
//! three sizes (S/M/L), each with three output parts (vocal, bass, other; see
//! harmonica-repro's `docs/okarina-sm-report.md` and `docs/mix-s2-report.md`).
//! Ported from harmonica-repro's `harmonica/infer.py` +
//! `harmonica/ort_backend.py` -- see that repo's `docs/phase5-report.md` for
//! the executable spec this follows. Streaming (window construction, block
//! scheduling) lives in `harmonica_stream.rs`; this module is just model
//! loading + single-window inference, mirroring `beat_model.rs`'s split.

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use ndarray::{Array2, ArrayView2};
use ort::ep::{cpu::CPU, directml::DirectML};
use ort::session::{builder::GraphOptimizationLevel, Session, SessionOutputs};
use ort::value::Tensor;

use crate::gpu_class::{self, GpuClass};

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

/// A note-transcription output part. The model separates `Vocal`, `Bass` and
/// `Other` (everything else, drums excluded -- they aren't pitched notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    Vocal,
    Bass,
    Other,
}

impl Part {
    /// The name the `parts` ONNX metadata field uses for this part.
    pub fn name(self) -> &'static str {
        match self {
            Part::Vocal => "vocal",
            Part::Bass => "bass",
            Part::Other => "other",
        }
    }
}

/// The part order of the model's output axis (its `parts` metadata); checked
/// at load time rather than trusted.
const PARTS: [Part; 3] = [Part::Vocal, Part::Bass, Part::Other];

/// Which Okarina network to use. Bigger is more accurate but slower; the
/// 0.256 s block budget (see harmonica-repro's `docs/okarina-sm-report.md`)
/// is met by L only on a DirectML dGPU, by M on a CPU with 8 threads or an
/// iGPU, and by S on a CPU with 4 threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelSize {
    Small,
    Medium,
    Large,
}

impl ModelSize {
    /// The ONNX file's bare name. Sizes are told apart by this name, not by
    /// the file's internal `size` metadata (which keeps the training-side
    /// names).
    pub fn file_name(self) -> &'static str {
        match self {
            ModelSize::Small => "okarina_s.onnx",
            ModelSize::Medium => "okarina_m.onnx",
            ModelSize::Large => "okarina_l.onnx",
        }
    }

    fn onnx_path(self) -> &'static str {
        match self {
            ModelSize::Small => concat!(env!("CARGO_MANIFEST_DIR"), "/models/okarina_s.onnx"),
            ModelSize::Medium => concat!(env!("CARGO_MANIFEST_DIR"), "/models/okarina_m.onnx"),
            ModelSize::Large => concat!(env!("CARGO_MANIFEST_DIR"), "/models/okarina_l.onnx"),
        }
    }

    /// The network's `size` metadata value (the training-side name).
    fn metadata_size(self) -> &'static str {
        match self {
            ModelSize::Small => "small",
            ModelSize::Medium => "medium",
            ModelSize::Large => "large",
        }
    }

    /// "Okarina S" etc., for display.
    pub fn label(self) -> &'static str {
        match self {
            ModelSize::Small => "Okarina S",
            ModelSize::Medium => "Okarina M",
            ModelSize::Large => "Okarina L",
        }
    }
}

impl std::str::FromStr for ModelSize {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "s" | "small" => Ok(ModelSize::Small),
            "m" | "medium" => Ok(ModelSize::Medium),
            "l" | "large" => Ok(ModelSize::Large),
            other => Err(anyhow!("unknown Okarina size {other:?} (expected s/m/l)")),
        }
    }
}

impl std::fmt::Display for ModelSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.metadata_size())
    }
}

/// What the user asked for: a fixed size, or `Auto`, which picks by hardware
/// (see `HarmonicaModel::load`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelChoice {
    Auto,
    Fixed(ModelSize),
}

impl std::str::FromStr for ModelChoice {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        if s.eq_ignore_ascii_case("auto") {
            Ok(ModelChoice::Auto)
        } else {
            s.parse().map(ModelChoice::Fixed).map_err(|_| anyhow!("unknown model {s:?} (expected auto/s/m/l)"))
        }
    }
}

/// One output part's onset/sustain/offset probabilities, each `[rows, N_PITCH]`.
pub struct PartProbs {
    pub part: Part,
    pub onset: Array2<f32>,
    pub sustain: Array2<f32>,
    pub offset: Array2<f32>,
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
    /// The GPU DirectML would use, when this session was loaded through
    /// `load` (`GpuClass::None` for `load_on`, which doesn't look).
    gpu: GpuClass,
    /// (th_on, th_sus) per part (`PARTS` order) for 8 and 16 frames of
    /// look-ahead, from the ONNX metadata.
    thresholds_f8: Vec<(f32, f32)>,
    thresholds_f16: Vec<(f32, f32)>,
}

/// Parses a `thresholds_*` metadata value (`{"vocal": [th_on, th_sus], ...}`)
/// into `parts`' order.
fn parse_thresholds(json: &str, parts: &[Part]) -> Result<Vec<(f32, f32)>> {
    let map: HashMap<String, [f32; 2]> = serde_json::from_str(json).context("parsing thresholds metadata")?;
    parts
        .iter()
        .map(|part| {
            let [on, sus] = *map.get(part.name()).ok_or_else(|| anyhow!("thresholds metadata has no entry for part {:?}", part.name()))?;
            Ok((on, sus))
        })
        .collect()
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

fn cpu_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

fn cpu_intra_threads() -> usize {
    cpu_threads().clamp(4, 8)
}

/// The size to run on the CPU: M needs 8 threads to keep up (about 90 ms per
/// block against the 256 ms budget), S fits in 4 (about 40 ms). Times on
/// weaker CPUs are unmeasured; this is the rule from the training side's
/// report, not a benchmark of this machine.
fn cpu_size() -> ModelSize {
    if cpu_threads() >= 8 {
        ModelSize::Medium
    } else {
        ModelSize::Small
    }
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

/// Reads output `name` as one `[T, features]` array per part. Shape is
/// `[batch, time, features]` for the CQT (one part) and `[batch, parts, time,
/// features]` for the network; batch is always 1 (see struct doc).
fn extract_parts(outputs: &SessionOutputs<'_>, name: &str, n_parts: usize) -> Result<Vec<Array2<f32>>> {
    let (shape, data) = outputs[name].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
    let expected_rank = if n_parts == 1 { 3 } else { 4 };
    if shape.len() != expected_rank || (n_parts > 1 && shape[1] as usize != n_parts) {
        bail!("output {name:?} has shape {shape:?}, expected {n_parts} part(s)");
    }
    let (t, f) = (shape[shape.len() - 2] as usize, shape[shape.len() - 1] as usize);
    (0..n_parts)
        .map(|p| Array2::from_shape_vec((t, f), data[p * t * f..(p + 1) * t * f].to_vec()).map_err(|e| anyhow!("{e}")))
        .collect()
}

impl HarmonicaModel {
    /// Loads exactly `size` on exactly `device`, no fallback -- callers that
    /// want the hardware-dependent policy should use `load`.
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
                ("size", size.metadata_size().to_string()),
                ("sample_rate", SAMPLE_RATE.to_string()),
                ("hop", HOP.to_string()),
                ("chunk", CHUNK.to_string()),
                ("n_bins", N_BINS.to_string()),
                ("n_pitch", N_PITCH.to_string()),
                ("cqt_margin", MARGIN.to_string()),
            ],
            size.file_name(),
        )?;

        let meta = net.metadata().map_err(|e| anyhow!("{e}"))?;
        // ONNX Runtime answers an absent key with an empty string, not None.
        let custom = |key: &str| meta.custom(key).filter(|v| !v.is_empty());
        let want = PARTS.iter().map(|p| p.name()).collect::<Vec<_>>().join(",");
        let got = custom("parts").ok_or_else(|| anyhow!("{}: ONNX metadata is missing \"parts\" (wrong/old model file?)", size.file_name()))?;
        if got != want {
            bail!("{}: ONNX metadata \"parts\" = {got:?}, expected {want:?}", size.file_name());
        }
        let read = |key: &str| -> Result<Vec<(f32, f32)>> {
            let json = custom(key).ok_or_else(|| anyhow!("{}: ONNX metadata is missing {key:?}", size.file_name()))?;
            parse_thresholds(&json, &PARTS).with_context(|| format!("{}: metadata {key:?}", size.file_name()))
        };
        let (thresholds_f8, thresholds_f16) = (read("thresholds_f8")?, read("thresholds_f16")?);
        drop(meta);

        let mut model = Self { cqt, net, size, device, gpu: GpuClass::None, thresholds_f8, thresholds_f16 };

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

    /// Loads the model for `choice`:
    ///
    /// - `Auto` on DirectML picks L for a discrete GPU and M for an
    ///   integrated one (the only sizes that fit the block budget there); with
    ///   no hardware adapter it goes straight to the CPU.
    /// - A fixed choice is loaded as asked on DirectML.
    /// - On the CPU (`allow_directml` false, no adapter, or DirectML failing
    ///   to load) the size follows `cpu_size`, and a fixed choice smaller than
    ///   that is kept. Decided by the user for the old models: when DirectML
    ///   can't be used, never run the requested size on the CPU if it would
    ///   miss the 256 ms block budget -- that would silently break real-time
    ///   use instead of just degrading accuracy.
    pub fn load(choice: ModelChoice, allow_directml: bool) -> Result<Self> {
        let gpu = if allow_directml { gpu_class::probe() } else { GpuClass::None };
        if allow_directml && !(choice == ModelChoice::Auto && gpu == GpuClass::None) {
            let size = match choice {
                ModelChoice::Fixed(size) => size,
                ModelChoice::Auto if matches!(gpu, GpuClass::Discrete(_)) => ModelSize::Large,
                ModelChoice::Auto => ModelSize::Medium,
            };
            match Self::load_on(size, Device::DirectML) {
                Ok(mut model) => {
                    eprintln!("HarmonicaModel: {} on DirectML ({})", size.label(), match &gpu {
                        GpuClass::Discrete(name) => format!("discrete GPU {name}"),
                        GpuClass::Integrated(name) => format!("integrated GPU {name}"),
                        GpuClass::None => "no hardware adapter found".to_string(),
                    });
                    model.gpu = gpu;
                    return Ok(model);
                }
                Err(e) => eprintln!("HarmonicaModel: DirectML load failed ({e}); falling back to the CPU"),
            }
        }
        let size = match choice {
            ModelChoice::Fixed(size) => size.min(cpu_size()),
            ModelChoice::Auto => cpu_size(),
        };
        Self::load_on(size, Device::Cpu)
    }

    pub fn size(&self) -> ModelSize {
        self.size
    }

    pub fn device(&self) -> Device {
        self.device
    }

    /// Whether a block can be expected to take longer than the 0.256 s it
    /// covers: L outside a discrete GPU (273 ms on an iGPU, 278 ms on 8 CPU
    /// threads). Only a fixed choice can end up here; `Auto` never picks it.
    pub fn may_exceed_budget(&self) -> bool {
        self.size == ModelSize::Large && !matches!(self.gpu, GpuClass::Discrete(_))
    }

    /// Output parts in the order `net` returns them.
    pub fn parts(&self) -> &[Part] {
        &PARTS
    }

    /// (th_on, th_sus) per part, parallel to `parts()`, for a session with
    /// `future_frames` of look-ahead. The metadata carries one set per
    /// look-ahead, and they can differ (L: the vocal th_sus is .35 at f8, .30
    /// at f16; S: .30 and .35): 16 or more uses the f16 set.
    pub fn part_thresholds(&self, future_frames: usize) -> &[(f32, f32)] {
        if future_frames >= 16 {
            &self.thresholds_f16
        } else {
            &self.thresholds_f8
        }
    }

    /// audio: exactly WINDOW_SAMPLES mono 16kHz samples. Returns [CHUNK,
    /// N_BINS] |CQT| magnitudes (untrimmed -- callers zero out-of-range rows
    /// themselves, see `harmonica_stream.rs`).
    pub fn cqt(&mut self, audio: &[f32]) -> Result<Array2<f32>> {
        assert_eq!(audio.len(), WINDOW_SAMPLES, "harmonica cqt window must be exactly WINDOW_SAMPLES samples");
        let input = Tensor::from_array(([1, WINDOW_SAMPLES], audio.to_vec())).map_err(|e| anyhow!("{e}"))?;
        let outputs = self.cqt.run(ort::inputs!["audio" => input]).map_err(|e| anyhow!("{e}"))?;
        Ok(extract_parts(&outputs, "cqt_mag", 1)?.remove(0))
    }

    /// cqt_mag: [CHUNK, N_BINS]. Returns one `PartProbs` per `parts()` entry,
    /// each holding [CHUNK, N_PITCH] sigmoid probabilities (offset is unused
    /// by decoding, per decode.py, but returned for completeness/inspection).
    pub fn net(&mut self, cqt_mag: ArrayView2<f32>) -> Result<Vec<PartProbs>> {
        let (t, f) = cqt_mag.dim();
        // Flatten row-major regardless of the view's own memory layout
        // (it may be a slice of a larger array, not necessarily contiguous).
        let data: Vec<f32> = cqt_mag.outer_iter().flat_map(|row| row.into_iter().copied()).collect();
        let input = Tensor::from_array(([1, t, f], data)).map_err(|e| anyhow!("{e}"))?;
        let outputs = self.net.run(ort::inputs!["cqt_mag" => input]).map_err(|e| anyhow!("{e}"))?;
        let n = PARTS.len();
        let onset = extract_parts(&outputs, "onset", n)?;
        let sustain = extract_parts(&outputs, "sustain", n)?;
        let offset = extract_parts(&outputs, "offset", n)?;
        Ok(PARTS
            .iter()
            .zip(onset.into_iter().zip(sustain).zip(offset))
            .map(|(&part, ((onset, sustain), offset))| PartProbs { part, onset, sustain, offset })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"vocal": [0.25, 0.3], "bass": [0.25, 0.9], "other": [0.1, 0.2]}"#;

    #[test]
    fn harmonica_thresholds_follow_the_given_part_order() {
        let th = parse_thresholds(JSON, &[Part::Other, Part::Vocal]).unwrap();
        assert_eq!(th, vec![(0.1, 0.2), (0.25, 0.3)]);
        let th = parse_thresholds(JSON, &PARTS).unwrap();
        assert_eq!(th, vec![(0.25, 0.3), (0.25, 0.9), (0.1, 0.2)]);
    }

    #[test]
    fn harmonica_thresholds_reject_missing_part_and_bad_json() {
        assert!(parse_thresholds(r#"{"vocal": [0.25, 0.3]}"#, &PARTS).is_err());
        assert!(parse_thresholds(r#"{"vocal": [0.25]}"#, &[Part::Vocal]).is_err());
        assert!(parse_thresholds("not json", &PARTS).is_err());
    }

    #[test]
    fn harmonica_model_size_names_round_trip() {
        for size in [ModelSize::Small, ModelSize::Medium, ModelSize::Large] {
            assert_eq!(size.to_string().parse::<ModelSize>().unwrap(), size);
        }
        assert_eq!("S".parse::<ModelSize>().unwrap(), ModelSize::Small);
        assert!("mix".parse::<ModelSize>().is_err());
        assert!(ModelSize::Small < ModelSize::Medium && ModelSize::Medium < ModelSize::Large);
    }

    #[test]
    fn harmonica_model_choice_parses_auto_and_sizes() {
        assert_eq!("auto".parse::<ModelChoice>().unwrap(), ModelChoice::Auto);
        assert_eq!("AUTO".parse::<ModelChoice>().unwrap(), ModelChoice::Auto);
        assert_eq!("m".parse::<ModelChoice>().unwrap(), ModelChoice::Fixed(ModelSize::Medium));
        assert!("mix".parse::<ModelChoice>().is_err());
    }
}
