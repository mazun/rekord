//! Golden-vector tests for the 3-part Mix model (`mix_large_v2.onnx`: vocal,
//! bass, other) against harmonica-repro's `tools/make_golden.py --mix`
//! output (ONNX Runtime 1.24.4 CPU, Python, `runs/s2_deploy`), see that
//! repo's `docs/mix-s2-report.md`. Two datasets:
//!
//! - `golden`: GuitarSet, notes only in the `other` part (vocal and bass
//!   must stay empty).
//! - `golden_rwc001`: 20 s of RWC-Pop 001 (vocals, bass, drums), notes in
//!   all three parts. RWC can't be redistributed, so like the other golden
//!   tests this reads it from harmonica-repro rather than from this repo.
//!
//! Tolerances are 1e-4 on CPU and 1e-3 on DirectML, as for the single-part
//! models (see harmonica_golden.rs). Override the directory holding
//! `golden/` and `golden_rwc001/` with HARMONICA_MIX_GOLDEN_DIR; tests skip
//! (not fail) when it, or `models/mix_large_v2.onnx`, is absent. DirectML
//! variants also skip when DirectML itself is unavailable.
//!
//! Run with (release -- CPU large is ~0.5s/block):
//! cargo test --manifest-path <repo>/rust/Cargo.toml -p rekord_core --release
//!   harmonica_mix -- --nocapture --test-threads=1

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use common::*;
use ndarray::{s, Array2};
use rekord_core::harmonica_decode::StreamingDecoder;
use rekord_core::harmonica_model::{Device, HarmonicaModel, ModelSize, Part, CHUNK, N_BINS, N_PITCH};
use rekord_core::harmonica_stream::NoteTranscriber;

const PARTS: [Part; 3] = [Part::Vocal, Part::Bass, Part::Other];

fn mix_golden_dirs() -> Vec<(&'static str, PathBuf)> {
    let base = std::env::var("HARMONICA_MIX_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../harmonica-repro/runs/s2_deploy"));
    let model = Path::new(env!("CARGO_MANIFEST_DIR")).join("models").join(ModelSize::Mix.file_name());
    if !model.exists() {
        eprintln!("skipping: no Mix model at {}", model.display());
        return Vec::new();
    }
    let dirs: Vec<_> = [("guitarset", "golden"), ("rwc001", "golden_rwc001")]
        .into_iter()
        .map(|(label, sub)| (label, base.join(sub)))
        .filter(|(_, dir)| dir.exists())
        .collect();
    if dirs.is_empty() {
        eprintln!("skipping: no Mix golden vectors under {}", base.display());
    }
    dirs
}

fn load_mix(device: Device) -> Option<HarmonicaModel> {
    match device {
        Device::Cpu => Some(HarmonicaModel::load_on(ModelSize::Mix, Device::Cpu).expect("cpu load must not fail")),
        Device::DirectML => try_load_dml(ModelSize::Mix),
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
    (0..PARTS.len()).map(|p| (block(0, p), block(1, p), block(2, p))).collect()
}

/// The golden's own `thresholds` are the f8 set (what its notes were decoded
/// with); the f16 set exists only as the ONNX metadata copy in `meta.json`.
fn golden_thresholds(dir: &Path, part: Part, future_frames: usize) -> (f32, f32) {
    let meta = read_meta(dir);
    let v = if future_frames >= 16 {
        let json = meta["mix_large"]["onnx_metadata"]["thresholds_f16"].as_str().expect("golden meta has no thresholds_f16");
        serde_json::from_str::<serde_json::Value>(json).unwrap()[part.name()].clone()
    } else {
        meta["mix_large"]["thresholds"][part.name()].clone()
    };
    (v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32)
}

fn golden_note_count(dir: &Path, part: Part) -> usize {
    read_meta(dir)["mix_large"]["n_notes"][part.name()].as_u64().unwrap() as usize
}

fn golden_notes(dir: &Path, part: Part) -> Vec<(f64, f64, f64)> {
    read_notes_json(&dir.join(format!("mix_large_notes_{}.json", part.name())))
}

// ---------------------------------------------------------------------
// 1. harmonica_mix_metadata
// ---------------------------------------------------------------------

#[test]
fn harmonica_mix_metadata() {
    let dirs = mix_golden_dirs();
    let Some((_, dir)) = dirs.first() else { return };
    let model = HarmonicaModel::load_on(ModelSize::Mix, Device::Cpu).expect("cpu load must not fail");
    assert_eq!(model.parts(), &PARTS);
    for future_frames in [8, 16] {
        for (&part, &(th_on, th_sus)) in PARTS.iter().zip(model.part_thresholds(future_frames)) {
            let (want_on, want_sus) = golden_thresholds(dir, part, future_frames);
            assert!(
                (th_on - want_on).abs() < 1e-6 && (th_sus - want_sus).abs() < 1e-6,
                "{part:?} f={future_frames}: metadata thresholds ({th_on}, {th_sus}) != golden meta ({want_on}, {want_sus})"
            );
        }
    }
    // A single-part model still reports exactly one part.
    let single = HarmonicaModel::load_on(ModelSize::Small, Device::Cpu).unwrap();
    assert_eq!(single.parts(), &[Part::All]);
    assert_eq!(single.part_thresholds(8), &[ModelSize::Small.thresholds().unwrap()]);
}

// ---------------------------------------------------------------------
// 2. harmonica_mix_net_window_{cpu,dml}
// ---------------------------------------------------------------------

fn run_net_window_test(device: Device, tol: f32) {
    for (label, dir) in mix_golden_dirs() {
        let Some(mut model) = load_mix(device) else { return };
        let cqt_mag = Array2::from_shape_vec((CHUNK, N_BINS), read_f32_file(&dir.join("window_cqt_mag.f32"))).unwrap();
        let out = model.net(cqt_mag.view()).unwrap();
        assert_eq!(out.iter().map(|p| p.part).collect::<Vec<_>>(), PARTS);
        let golden = split_heads_parts(read_f32_file(&dir.join("mix_large_window_probs.f32")), CHUNK);
        for (probs, (g_on, g_sus, g_off)) in out.iter().zip(&golden) {
            let (d_on, d_sus, d_off) = (max_abs_diff(&probs.onset, g_on), max_abs_diff(&probs.sustain, g_sus), max_abs_diff(&probs.offset, g_off));
            eprintln!("harmonica_mix_net_window {device:?} {label} {:?}: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}", probs.part);
            assert!(d_on <= tol && d_sus <= tol && d_off <= tol, "{label} {:?} {device:?}: window delta exceeds {tol}", probs.part);
        }
    }
}

#[test]
fn harmonica_mix_net_window_cpu() {
    run_net_window_test(Device::Cpu, 1e-4);
}

#[test]
fn harmonica_mix_net_window_dml() {
    run_net_window_test(Device::DirectML, 1e-3);
}

// ---------------------------------------------------------------------
// 3. harmonica_mix_stream_{cpu,dml}
// ---------------------------------------------------------------------

fn run_stream_test(device: Device, tol: f32) {
    // Irregular chunk sizes, cycled, deliberately not aligned to BLOCK_FRAMES.
    let chunk_pattern = [1usize, 4095, 4097, 12345, 777, 30000];
    for (label, dir) in mix_golden_dirs() {
        let Some(model) = load_mix(device) else { return };
        let t = read_meta_t(&dir);
        let audio = read_f32_file(&dir.join("audio_16k.f32"));
        let thresholds = model.part_thresholds(8).to_vec();

        let result = collect_stream(model, &audio, 8, &chunk_pattern);
        assert_eq!(result.frames_produced, t, "expected exactly T={t} frames");
        assert_eq!(result.parts.iter().map(|p| p.part).collect::<Vec<_>>(), PARTS);
        print_block_timings(&format!("harmonica_mix_stream {device:?} {label}"), &result.timings_ms);

        let golden = split_heads_parts(read_f32_file(&dir.join("mix_large_probs.f32")), t);
        for ((probs, g), &(th_on, th_sus)) in result.parts.iter().zip(&golden).zip(&thresholds) {
            let name = format!("harmonica_mix_stream {device:?} {label} {:?}", probs.part);
            check_stream_probs(&name, probs, g, t, tol);

            let mut decoder = StreamingDecoder::new(th_on, th_sus);
            let mut notes = decoder.push(probs.onset.view(), probs.sustain.view());
            notes.extend(decoder.finish());
            check_notes_match(&notes, &golden_notes(&dir, probs.part), &g.0, &g.1, th_on, th_sus, tol, t, &name);
            if golden_note_count(&dir, probs.part) == 0 {
                assert!(notes.is_empty(), "{name}: golden has no notes in this part but we decoded {}", notes.len());
            } else {
                assert!(!notes.is_empty(), "{name}: golden has notes in this part but we decoded none");
            }
        }
    }
}

// 2e-4 rather than the single-part models' 1e-4: with the Mix model the CPU
// steady-state sustain of the `other` part differs from the golden by up to
// 1.2e-4 (GuitarSet), the same ONNX Runtime 1.28 (pyke build) vs 1.24.4
// (golden) effect described in common::check_stream_probs. Decoded notes are
// still compared exactly (modulo near-ties) right below it.
#[test]
fn harmonica_mix_stream_cpu() {
    run_stream_test(Device::Cpu, 2e-4);
}

#[test]
fn harmonica_mix_stream_dml() {
    run_stream_test(Device::DirectML, 1e-3);
}

// ---------------------------------------------------------------------
// 4. harmonica_mix_decode_golden (no ONNX)
// ---------------------------------------------------------------------

#[test]
fn harmonica_mix_decode_golden() {
    for (label, dir) in mix_golden_dirs() {
        let t = read_meta_t(&dir);
        let golden = split_heads_parts(read_f32_file(&dir.join("mix_large_probs.f32")), t);
        for (part, (onset, sustain, _)) in PARTS.iter().zip(&golden) {
            let (th_on, th_sus) = golden_thresholds(&dir, *part, 8);
            let expected: HashSet<(i64, i64, i32)> =
                golden_notes(&dir, *part).iter().map(|&(a, b, c)| (frame_of(a), frame_of(b), c.round() as i32)).collect();
            for blocks in [8usize, 1, t] {
                let mut decoder = StreamingDecoder::new(th_on, th_sus);
                let mut notes = Vec::new();
                let mut r = 0;
                while r < t {
                    let end = (r + blocks).min(t);
                    notes.extend(decoder.push(onset.slice(s![r..end, ..]), sustain.slice(s![r..end, ..])));
                    r = end;
                }
                notes.extend(decoder.finish());
                let got: HashSet<(i64, i64, i32)> = notes.iter().map(|n| (n.start_frame as i64, n.end_frame as i64, n.pitch_midi)).collect();
                assert_eq!(got, expected, "{label} {part:?} (rows per push: {blocks}): decode mismatch vs golden notes");
            }
            eprintln!("harmonica_mix_decode_golden {label} {part:?}: {} notes, exact match in 3 chunkings", expected.len());
        }
    }
}

// ---------------------------------------------------------------------
// 5. harmonica_mix_transcriber_dml: NoteTranscriber end to end
// ---------------------------------------------------------------------

/// Whole `NoteTranscriber` (model + per-part decoders + part tagging) fed in
/// capture-sized chunks (64 ms): every part's confirmed notes must match the
/// golden notes of that part, and `note_events` must carry the part.
#[test]
fn harmonica_mix_transcriber_dml() {
    const TOL: f32 = 1e-3;
    for (label, dir) in mix_golden_dirs() {
        let Some(model) = load_mix(Device::DirectML) else { return };
        let t = read_meta_t(&dir);
        let audio = read_f32_file(&dir.join("audio_16k.f32"));
        let thresholds = model.part_thresholds(8).to_vec();
        let mut transcriber = NoteTranscriber::new(model, 8);

        let mut confirmed = Vec::new();
        let mut saw_part_in_events = HashSet::new();
        for chunk in audio.chunks(1024) {
            confirmed.extend(transcriber.push(chunk).unwrap());
            saw_part_in_events.extend(transcriber.note_events().iter().map(|e| e.4));
        }
        confirmed.extend(transcriber.finish().unwrap());

        let golden = split_heads_parts(read_f32_file(&dir.join("mix_large_probs.f32")), t);
        for ((&part, g), &(th_on, th_sus)) in PARTS.iter().zip(&golden).zip(&thresholds) {
            let keys: HashSet<(i64, i64, i32)> = confirmed
                .iter()
                .filter(|n| n.part == part)
                .map(|n| (frame_of(n.start_s as f64), frame_of(n.end_s as f64), n.pitch_midi))
                .collect();
            check_note_keys_match(&keys, &golden_notes(&dir, part), &g.0, &g.1, th_on, th_sus, TOL, t, &format!("harmonica_mix_transcriber {label} {part:?}"));
            if golden_note_count(&dir, part) > 0 {
                assert!(saw_part_in_events.contains(&part), "{label}: note_events never carried a {part:?} note");
            }
        }
        assert!(confirmed.iter().all(|n| n.part != Part::All), "Mix notes must never be tagged Part::All");
    }
}
