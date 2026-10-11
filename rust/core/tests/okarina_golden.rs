//! Golden-vector tests for the Okarina S/M/L models (three parts: vocal, bass,
//! other) against harmonica-repro's `tools/make_golden.py --mix` output
//! (ONNX Runtime 1.24.4 CPU, Python), see that repo's
//! `docs/okarina-sm-report.md` and `docs/mix-s2-report.md`. Per size, two
//! datasets:
//!
//! - `golden`: GuitarSet, notes only in the `other` part (vocal and bass
//!   must stay empty).
//! - `golden_rwc001`: 20 s of RWC-Pop 001 (vocals, bass, drums), notes in
//!   all three parts. RWC can't be redistributed, so like the other golden
//!   tests this reads it from harmonica-repro rather than from this repo.
//!
//! The golden files keep the training side's internal names (`mix_small_*`,
//! `mix_medium_*`, `mix_large_*`); this file maps them to S/M/L. Where they
//! live (relative to the harmonica-repro checkout, default
//! `../../../harmonica-repro`, override with HARMONICA_REPRO_DIR):
//!
//! - S: `runs/okarina_deploy/small/{golden,golden_rwc001}`
//! - M: `runs/okarina_deploy/medium/{golden,golden_rwc001}`
//! - L: `runs/s2_deploy/{golden,golden_rwc001}`
//!
//! Tolerances are 1e-4 on CPU and 1e-3 on DirectML. Tests skip (not fail)
//! for a size whose golden directory or `models/okarina_*.onnx` is absent;
//! DirectML variants also skip when DirectML itself is unavailable.
//!
//! Run with (release -- CPU L is ~0.4s/block):
//! cargo test --manifest-path <repo>/rust/Cargo.toml -p rekord_core --release
//!   okarina_ -- --nocapture --test-threads=1

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use common::*;
use ndarray::{s, Array2};
use rekord_core::harmonica_decode::StreamingDecoder;
use rekord_core::harmonica_model::{
    Device, HarmonicaModel, ModelSize, Part, CHUNK, HOP, N_BINS, N_PITCH, WINDOW_SAMPLES,
};
use rekord_core::harmonica_stream::{BlockStreamer, NoteTranscriber};

const PARTS: [Part; 3] = [Part::Vocal, Part::Bass, Part::Other];

/// One golden dataset of one model size.
struct Golden {
    size: ModelSize,
    /// "guitarset" or "rwc001", for messages.
    dataset: &'static str,
    dir: PathBuf,
    /// The training side's name for this size: file prefix and `meta.json` key.
    key: &'static str,
}

impl Golden {
    fn label(&self) -> String {
        format!("{} {}", self.size.label(), self.dataset)
    }

    fn file(&self, suffix: &str) -> PathBuf {
        self.dir.join(format!("{}_{suffix}", self.key))
    }

    /// The golden's own `thresholds` are the f8 set (what its notes were
    /// decoded with); the f16 set exists only as the ONNX metadata copy in
    /// `meta.json`.
    fn thresholds(&self, part: Part, future_frames: usize) -> (f32, f32) {
        let meta = &read_meta(&self.dir)[self.key];
        let v = if future_frames >= 16 {
            let json = meta["onnx_metadata"]["thresholds_f16"]
                .as_str()
                .expect("golden meta has no thresholds_f16");
            serde_json::from_str::<serde_json::Value>(json).unwrap()[part.name()].clone()
        } else {
            meta["thresholds"][part.name()].clone()
        };
        (v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32)
    }

    fn note_count(&self, part: Part) -> usize {
        read_meta(&self.dir)[self.key]["n_notes"][part.name()]
            .as_u64()
            .unwrap() as usize
    }

    fn notes(&self, part: Part) -> Vec<(f64, f64, f64)> {
        read_notes_json(&self.file(&format!("notes_{}.json", part.name())))
    }

    fn probs(&self, t: usize) -> Vec<PartTriple> {
        split_heads_parts(read_f32_file(&self.file("probs.f32")), t)
    }

    fn window_probs(&self) -> Vec<PartTriple> {
        split_heads_parts(read_f32_file(&self.file("window_probs.f32")), CHUNK)
    }
}

fn repro_dir() -> PathBuf {
    std::env::var("HARMONICA_REPRO_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../harmonica-repro"))
}

/// Every golden dataset whose model file and directory exist.
fn goldens() -> Vec<Golden> {
    let repro = repro_dir();
    let mut out = Vec::new();
    for (size, base, key) in [
        (ModelSize::Small, "runs/okarina_deploy/small", "mix_small"),
        (
            ModelSize::Medium,
            "runs/okarina_deploy/medium",
            "mix_medium",
        ),
        (ModelSize::Large, "runs/s2_deploy", "mix_large"),
    ] {
        let model = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("models")
            .join(size.file_name());
        if !model.exists() {
            eprintln!("skipping {}: no model at {}", size.label(), model.display());
            continue;
        }
        for (dataset, sub) in [("guitarset", "golden"), ("rwc001", "golden_rwc001")] {
            let dir = repro.join(base).join(sub);
            if dir.exists() {
                out.push(Golden {
                    size,
                    dataset,
                    dir,
                    key,
                });
            } else {
                eprintln!(
                    "skipping {} {dataset}: no golden vectors at {}",
                    size.label(),
                    dir.display()
                );
            }
        }
    }
    out
}

fn load(size: ModelSize, device: Device) -> Option<HarmonicaModel> {
    match device {
        Device::Cpu => {
            Some(HarmonicaModel::load_on(size, Device::Cpu).expect("cpu load must not fail"))
        }
        Device::DirectML => try_load_dml(size),
    }
}

type PartTriple = (Array2<f32>, Array2<f32>, Array2<f32>);

/// Splits a [3 heads (onset/sustain/offset), 3 parts, rows, N_PITCH]
/// golden buffer into one (onset, sustain, offset) triple per part.
fn split_heads_parts(flat: Vec<f32>, rows: usize) -> Vec<PartTriple> {
    let n = rows * N_PITCH;
    assert_eq!(flat.len(), 3 * PARTS.len() * n);
    let block = |head: usize, part: usize| {
        let start = (head * PARTS.len() + part) * n;
        Array2::from_shape_vec((rows, N_PITCH), flat[start..start + n].to_vec()).unwrap()
    };
    (0..PARTS.len())
        .map(|p| (block(0, p), block(1, p), block(2, p)))
        .collect()
}

// ---------------------------------------------------------------------
// 1. okarina_metadata
// ---------------------------------------------------------------------

#[test]
fn okarina_metadata() {
    for g in goldens() {
        let model = HarmonicaModel::load_on(g.size, Device::Cpu).expect("cpu load must not fail");
        assert_eq!(model.size(), g.size);
        assert_eq!(model.parts(), &PARTS);
        for future_frames in [8, 16] {
            for (&part, &(th_on, th_sus)) in PARTS.iter().zip(model.part_thresholds(future_frames))
            {
                let (want_on, want_sus) = g.thresholds(part, future_frames);
                assert!(
                    (th_on - want_on).abs() < 1e-6 && (th_sus - want_sus).abs() < 1e-6,
                    "{} {part:?} f={future_frames}: metadata thresholds ({th_on}, {th_sus}) != golden meta ({want_on}, {want_sus})",
                    g.label()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// 2. okarina_cqt_window_{cpu,dml}
// ---------------------------------------------------------------------

fn run_cqt_window_test(device: Device, tol: f32) {
    // The CQT is shared by all sizes and every golden holds the same window;
    // one dataset per size is enough to also cover each size's own session.
    for g in goldens().into_iter().filter(|g| g.dataset == "guitarset") {
        let Some(mut model) = load(g.size, device) else {
            return;
        };
        let window_audio = read_f32_file(&g.dir.join("window_audio.f32"));
        assert_eq!(window_audio.len(), WINDOW_SAMPLES);
        let golden_mag = Array2::from_shape_vec(
            (CHUNK, N_BINS),
            read_f32_file(&g.dir.join("window_cqt_mag.f32")),
        )
        .unwrap();
        let d = max_abs_diff(&model.cqt(&window_audio).unwrap(), &golden_mag);
        eprintln!(
            "okarina_cqt_window {device:?} {}: max|delta| = {d:.3e}",
            g.label()
        );
        assert!(
            d <= tol,
            "{}: cqt max|delta| {d} exceeds tolerance {tol}",
            g.label()
        );
    }
}

#[test]
fn okarina_cqt_window_cpu() {
    run_cqt_window_test(Device::Cpu, 1e-4);
}

#[test]
fn okarina_cqt_window_dml() {
    run_cqt_window_test(Device::DirectML, 1e-3);
}

// ---------------------------------------------------------------------
// 3. okarina_net_window_{cpu,dml}
// ---------------------------------------------------------------------

fn run_net_window_test(device: Device, tol: f32) {
    for g in goldens() {
        let Some(mut model) = load(g.size, device) else {
            return;
        };
        let cqt_mag = Array2::from_shape_vec(
            (CHUNK, N_BINS),
            read_f32_file(&g.dir.join("window_cqt_mag.f32")),
        )
        .unwrap();
        let out = model.net(cqt_mag.view()).unwrap();
        assert_eq!(out.iter().map(|p| p.part).collect::<Vec<_>>(), PARTS);
        for (probs, (g_on, g_sus, g_off)) in out.iter().zip(&g.window_probs()) {
            let (d_on, d_sus, d_off) = (
                max_abs_diff(&probs.onset, g_on),
                max_abs_diff(&probs.sustain, g_sus),
                max_abs_diff(&probs.offset, g_off),
            );
            eprintln!("okarina_net_window {device:?} {} {:?}: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}", g.label(), probs.part);
            assert!(
                d_on <= tol && d_sus <= tol && d_off <= tol,
                "{} {:?} {device:?}: window delta exceeds {tol}",
                g.label(),
                probs.part
            );
        }
    }
}

#[test]
fn okarina_net_window_cpu() {
    run_net_window_test(Device::Cpu, 1e-4);
}

#[test]
fn okarina_net_window_dml() {
    run_net_window_test(Device::DirectML, 1e-3);
}

// ---------------------------------------------------------------------
// 4. okarina_stream_{cpu,dml}
// ---------------------------------------------------------------------

fn run_stream_test(device: Device, tol: f32) {
    // Irregular chunk sizes, cycled, deliberately not aligned to BLOCK_FRAMES.
    let chunk_pattern = [1usize, 4095, 4097, 12345, 777, 30000];
    for g in goldens() {
        let Some(model) = load(g.size, device) else {
            return;
        };
        let t = read_meta_t(&g.dir);
        let audio = read_f32_file(&g.dir.join("audio_16k.f32"));
        let thresholds = model.part_thresholds(8).to_vec();

        let result = collect_stream(model, &audio, 8, &chunk_pattern);
        assert_eq!(result.frames_produced, t, "expected exactly T={t} frames");
        assert_eq!(
            result.parts.iter().map(|p| p.part).collect::<Vec<_>>(),
            PARTS
        );
        print_block_timings(
            &format!("okarina_stream {device:?} {}", g.label()),
            &result.timings_ms,
        );

        let golden = g.probs(t);
        for ((probs, gold), &(th_on, th_sus)) in result.parts.iter().zip(&golden).zip(&thresholds) {
            let name = format!("okarina_stream {device:?} {} {:?}", g.label(), probs.part);
            check_stream_probs(&name, probs, gold, t, tol);

            let mut decoder = StreamingDecoder::new(th_on, th_sus);
            let mut notes = decoder.push(probs.onset.view(), probs.sustain.view());
            notes.extend(decoder.finish());
            check_notes_match(
                &notes,
                &g.notes(probs.part),
                &GoldenProbs {
                    onset: &gold.0,
                    sustain: &gold.1,
                    th_on,
                    th_sus,
                    tol,
                    t,
                },
                &name,
            );
            if g.note_count(probs.part) == 0 {
                assert!(
                    notes.is_empty(),
                    "{name}: golden has no notes in this part but we decoded {}",
                    notes.len()
                );
            } else {
                assert!(
                    !notes.is_empty(),
                    "{name}: golden has notes in this part but we decoded none"
                );
            }
        }
    }
}

#[test]
fn okarina_stream_cpu() {
    run_stream_test(Device::Cpu, 1e-4);
}

#[test]
fn okarina_stream_dml() {
    run_stream_test(Device::DirectML, 1e-3);
}

// ---------------------------------------------------------------------
// 5. okarina_decode_golden (no ONNX)
// ---------------------------------------------------------------------

#[test]
fn okarina_decode_golden() {
    for g in goldens() {
        let t = read_meta_t(&g.dir);
        for (part, (onset, sustain, _)) in PARTS.iter().zip(&g.probs(t)) {
            let (th_on, th_sus) = g.thresholds(*part, 8);
            let expected: HashSet<(i64, i64, i32)> = g
                .notes(*part)
                .iter()
                .map(|&(a, b, c)| (frame_of(a), frame_of(b), c.round() as i32))
                .collect();
            for blocks in [8usize, 1, t] {
                let mut decoder = StreamingDecoder::new(th_on, th_sus);
                let mut notes = Vec::new();
                let mut r = 0;
                while r < t {
                    let end = (r + blocks).min(t);
                    notes.extend(
                        decoder.push(onset.slice(s![r..end, ..]), sustain.slice(s![r..end, ..])),
                    );
                    r = end;
                }
                notes.extend(decoder.finish());
                let got: HashSet<(i64, i64, i32)> = notes
                    .iter()
                    .map(|n| (n.start_frame as i64, n.end_frame as i64, n.pitch_midi))
                    .collect();
                assert_eq!(
                    got,
                    expected,
                    "{} {part:?} (rows per push: {blocks}): decode mismatch vs golden notes",
                    g.label()
                );
            }
            eprintln!(
                "okarina_decode_golden {} {part:?}: {} notes, exact match in 3 chunkings",
                g.label(),
                expected.len()
            );
        }
    }
}

// ---------------------------------------------------------------------
// 6. okarina_future16_rows (CPU): which rows of the window a block takes
// ---------------------------------------------------------------------

/// The golden window is the one of f=8's block 40 (last frame 335); f=16's
/// block 39 has the same "now", so the same full window, and only the
/// extracted row range differs.
#[test]
fn okarina_future16_rows() {
    let last = 335usize;
    for g in goldens().into_iter().filter(|g| g.dataset == "guitarset") {
        let audio = read_f32_file(&g.dir.join("audio_16k.f32"));
        let prefix = &audio[..last * HOP + 1];
        let window = g.window_probs();

        for (future_frames, block, rows) in [(16usize, 39usize, 132..140), (8, 40, 140..148)] {
            let model = HarmonicaModel::load_on(g.size, Device::Cpu).unwrap();
            let mut streamer = BlockStreamer::new(model, future_frames);
            let mut last_block = None;
            streamer.push(prefix, |b| last_block = Some(b)).unwrap();
            let b = last_block.unwrap_or_else(|| {
                panic!("block {block} (f={future_frames}) should have been produced")
            });
            assert_eq!(b.first_frame, block * 8);
            for (part, (g_on, g_sus, g_off)) in b.parts.iter().zip(&window) {
                let rows = s![rows.clone(), ..];
                let d_on = max_abs_diff(&part.onset, &g_on.slice(rows).to_owned());
                let d_sus = max_abs_diff(&part.sustain, &g_sus.slice(rows).to_owned());
                let d_off = max_abs_diff(&part.offset, &g_off.slice(rows).to_owned());
                eprintln!("okarina_future16_rows {} f={future_frames} {:?}: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}", g.label(), part.part);
                assert!(
                    d_on <= 1e-4 && d_sus <= 1e-4 && d_off <= 1e-4,
                    "{} f={future_frames} {:?}",
                    g.label(),
                    part.part
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// 7. okarina_transcriber_dml: NoteTranscriber end to end
// ---------------------------------------------------------------------

/// Whole `NoteTranscriber` (model + per-part decoders + part tagging) fed in
/// capture-sized chunks (64 ms): every part's confirmed notes must match the
/// golden notes of that part, and `note_events` must carry the part.
#[test]
fn okarina_transcriber_dml() {
    const TOL: f32 = 1e-3;
    for g in goldens() {
        let Some(model) = load(g.size, Device::DirectML) else {
            return;
        };
        let t = read_meta_t(&g.dir);
        let audio = read_f32_file(&g.dir.join("audio_16k.f32"));
        let thresholds = model.part_thresholds(8).to_vec();
        let mut transcriber = NoteTranscriber::new(model, 8);

        let mut confirmed = Vec::new();
        let mut saw_part_in_events = HashSet::new();
        for chunk in audio.chunks(1024) {
            confirmed.extend(transcriber.push(chunk).unwrap());
            saw_part_in_events.extend(transcriber.note_events().iter().map(|e| e.4));
        }
        confirmed.extend(transcriber.finish().unwrap());

        for ((&part, gold), &(th_on, th_sus)) in PARTS.iter().zip(&g.probs(t)).zip(&thresholds) {
            let keys: HashSet<(i64, i64, i32)> = confirmed
                .iter()
                .filter(|n| n.part == part)
                .map(|n| {
                    (
                        frame_of(n.start_s as f64),
                        frame_of(n.end_s as f64),
                        n.pitch_midi,
                    )
                })
                .collect();
            check_note_keys_match(
                &keys,
                &g.notes(part),
                &GoldenProbs {
                    onset: &gold.0,
                    sustain: &gold.1,
                    th_on,
                    th_sus,
                    tol: TOL,
                    t,
                },
                &format!("okarina_transcriber {} {part:?}", g.label()),
            );
            if g.note_count(part) > 0 {
                assert!(
                    saw_part_in_events.contains(&part),
                    "{}: note_events never carried a {part:?} note",
                    g.label()
                );
            }
        }
    }
}
