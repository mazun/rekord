//! Harmonica (instrument-agnostic note transcription) ONNX models: a CQT
//! front-end (`cqt156.onnx`) plus one onset/sustain/offset transcription
//! network: small/medium/large (one part, all notes together) or `Mix`, a
//! large network with three output parts (vocal, bass, other) trained for
//! full mixes (see harmonica-repro's `docs/mix-b4-report.md`). Ported from
//! harmonica-repro's `harmonica/infer.py` + `harmonica/ort_backend.py` --
//! see that repo's `docs/phase5-report.md` for the executable spec this
//! follows. Streaming (window construction, block scheduling) lives in
//! `harmonica_stream.rs`; this module is just model loading + single-window
//! inference, mirroring `beat_model.rs`'s split.

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
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

/// A note-transcription output part. The single-part models emit every note
/// as `All`; the `Mix` model separates `Vocal`, `Bass` and `Other` (everything
/// else, drums excluded -- they aren't pitched notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    All,
    Vocal,
    Bass,
    Other,
}

impl Part {
    /// The name the `parts` ONNX metadata field uses for this part.
    pub fn name(self) -> &'static str {
        match self {
            Part::All => "all",
            Part::Vocal => "vocal",
            Part::Bass => "bass",
            Part::Other => "other",
        }
    }
}

/// The part order of the `Mix` model's output axis (its `parts` metadata);
/// checked at load time rather than trusted.
const MIX_PARTS: [Part; 3] = [Part::Vocal, Part::Bass, Part::Other];

/// Which trained transcription network to use. Bigger is more accurate but
/// slower (see docs/phase5-report.md's speed table in harmonica-repro);
/// `HarmonicaModel::load` always falls back to `Small` on CPU regardless of
/// the requested size (see its doc comment). `Mix` is as fast as `Large` but
/// outputs three parts and is trained for full mixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelSize {
    Small,
    Medium,
    Large,
    Mix,
}

impl ModelSize {
    /// The exported ONNX file's bare name (also the value of that file's own
    /// `size` custom-metadata field for the single-part models, checked in
    /// `load_on`).
    pub fn file_name(self) -> &'static str {
        match self {
            ModelSize::Small => "small_uni_300k.onnx",
            ModelSize::Medium => "medium_uni_300k_urmp.onnx",
            ModelSize::Large => "large_uni_300k_urmp.onnx",
            ModelSize::Mix => "mix_large_v2.onnx",
        }
    }

    fn onnx_path(self) -> &'static str {
        match self {
            ModelSize::Small => concat!(env!("CARGO_MANIFEST_DIR"), "/models/small_uni_300k.onnx"),
            ModelSize::Medium => concat!(env!("CARGO_MANIFEST_DIR"), "/models/medium_uni_300k_urmp.onnx"),
            ModelSize::Large => concat!(env!("CARGO_MANIFEST_DIR"), "/models/large_uni_300k_urmp.onnx"),
            ModelSize::Mix => concat!(env!("CARGO_MANIFEST_DIR"), "/models/mix_large_v2.onnx"),
        }
    }

    /// Whether this model has several output parts (`Mix`) rather than one.
    pub fn is_multipart(self) -> bool {
        self == ModelSize::Mix
    }

    /// (th_on, th_sus) of a single-part model, chosen on the f=8 dev set in
    /// harmonica-repro; used unchanged for any future_frames >= 8 (see
    /// phase5-report.md's threshold section -- re-picking per f changes
    /// dev-set OnPOff by <= 0.4 points). `None` for `Mix`, whose per-part
    /// thresholds live in its ONNX metadata (`HarmonicaModel::part_thresholds`).
    pub fn thresholds(self) -> Option<(f32, f32)> {
        match self {
            ModelSize::Small => Some((0.25, 0.45)),
            ModelSize::Medium => Some((0.20, 0.45)),
            ModelSize::Large => Some((0.20, 0.45)),
            ModelSize::Mix => None,
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
            "mix" => Ok(ModelSize::Mix),
            other => Err(anyhow!("unknown harmonica model size {other:?} (expected small/medium/large/mix)")),
        }
    }
}

impl std::fmt::Display for ModelSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ModelSize::Small => "small",
            ModelSize::Medium => "medium",
            ModelSize::Large => "large",
            ModelSize::Mix => "mix",
        })
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
    /// Output parts in the network's axis order (just `[All]` for a
    /// single-part model).
    parts: Vec<Part>,
    /// (th_on, th_sus) per part for 8 and 16 frames of look-ahead, parallel
    /// to `parts`. Single-part models use the same pair for both.
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

/// Reads output `name` as one `[T, features]` array per part. Shape is
/// `[batch, time, features]` for a single-part model and `[batch, parts,
/// time, features]` for a multi-part one; batch is always 1 (see struct doc).
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
                // `Mix` is the large architecture with a wider last layer; its
                // part layout is checked below.
                ("size", if size.is_multipart() { "large".to_string() } else { size.to_string() }),
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
        let (parts, thresholds_f8, thresholds_f16) = if size.is_multipart() {
            let want = MIX_PARTS.iter().map(|p| p.name()).collect::<Vec<_>>().join(",");
            let got = custom("parts").ok_or_else(|| anyhow!("{}: ONNX metadata is missing \"parts\" (wrong/old model file?)", size.file_name()))?;
            if got != want {
                bail!("{}: ONNX metadata \"parts\" = {got:?}, expected {want:?}", size.file_name());
            }
            let parts = MIX_PARTS.to_vec();
            let read = |key: &str| -> Result<Vec<(f32, f32)>> {
                let json = custom(key).ok_or_else(|| anyhow!("{}: ONNX metadata is missing {key:?}", size.file_name()))?;
                parse_thresholds(&json, &parts).with_context(|| format!("{}: metadata {key:?}", size.file_name()))
            };
            let (f8, f16) = (read("thresholds_f8")?, read("thresholds_f16")?);
            (parts, f8, f16)
        } else {
            if custom("parts").is_some() {
                bail!("{}: has a \"parts\" metadata entry but is loaded as a single-part model", size.file_name());
            }
            let th = size.thresholds().expect("single-part sizes have fixed thresholds");
            (vec![Part::All], vec![th], vec![th])
        };
        drop(meta);

        let mut model = Self { cqt, net, size, device, parts, thresholds_f8, thresholds_f16 };

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

    /// Output parts in the order `net` returns them.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// (th_on, th_sus) per part, parallel to `parts()`, for a session with
    /// `future_frames` of look-ahead. The Mix model's metadata carries one
    /// set per look-ahead, and they differ (v2: the vocal th_sus is .35 at
    /// f8, .30 at f16): 16 or more uses the f16 set.
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
        let n = self.parts.len();
        let onset = extract_parts(&outputs, "onset", n)?;
        let sustain = extract_parts(&outputs, "sustain", n)?;
        let offset = extract_parts(&outputs, "offset", n)?;
        Ok(self
            .parts
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
        let th = parse_thresholds(JSON, &MIX_PARTS).unwrap();
        assert_eq!(th, vec![(0.25, 0.3), (0.25, 0.9), (0.1, 0.2)]);
    }

    #[test]
    fn harmonica_thresholds_reject_missing_part_and_bad_json() {
        assert!(parse_thresholds(r#"{"vocal": [0.25, 0.3]}"#, &MIX_PARTS).is_err());
        assert!(parse_thresholds(r#"{"vocal": [0.25]}"#, &[Part::Vocal]).is_err());
        assert!(parse_thresholds("not json", &MIX_PARTS).is_err());
    }

    #[test]
    fn harmonica_model_size_names_round_trip() {
        for size in [ModelSize::Small, ModelSize::Medium, ModelSize::Large, ModelSize::Mix] {
            assert_eq!(size.to_string().parse::<ModelSize>().unwrap(), size);
        }
        assert!(ModelSize::Mix.thresholds().is_none() && ModelSize::Mix.is_multipart());
        assert!(ModelSize::Large.thresholds().is_some() && !ModelSize::Large.is_multipart());
    }
}
