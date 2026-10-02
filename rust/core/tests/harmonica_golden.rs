//! Golden-vector tests against harmonica-repro's `tools/make_golden.py`
//! output (ONNX Runtime 1.24.4 CPU, Python). Tolerances are 1e-4 on CPU and
//! 1e-3 on DirectML, as in that repo's `docs/phase5-report.md`, except the
//! first frames of a stream (see `harmonica_stream_*`).
//!
//! Override the golden directory with HARMONICA_GOLDEN_DIR; tests skip (not
//! fail) when it's absent, same pattern as BEAT_GOLDEN_DIR in beat_model.rs.
//! DirectML variants additionally skip (not fail) when DirectML itself is
//! unavailable on this machine.
//!
//! Run with (release -- CPU large is ~0.6s/block):
//! cargo test --manifest-path <repo>/rust/Cargo.toml -p rekord_core --release
//!   harmonica_ -- --nocapture --test-threads=1

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use common::*;
use ndarray::{s, Array2};
use rekord_core::harmonica_decode::StreamingDecoder;
use rekord_core::harmonica_model::{Device, HarmonicaModel, ModelSize, Part, PartProbs, CHUNK, HOP, N_PITCH, WINDOW_SAMPLES};
use rekord_core::harmonica_stream::BlockStreamer;

fn golden_dir() -> Option<PathBuf> {
    let dir = std::env::var("HARMONICA_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../harmonica-repro/runs/p5/golden"));
    if dir.exists() {
        Some(dir)
    } else {
        eprintln!("skipping: no golden vectors at {}", dir.display());
        None
    }
}

/// Splits a [3*T, N_PITCH]-flattened golden buffer (onset/sustain/offset
/// stacked along axis 0, T rows each) into three [T, N_PITCH] arrays.
fn split_three(flat: Vec<f32>, t: usize) -> (Array2<f32>, Array2<f32>, Array2<f32>) {
    let n = t * N_PITCH;
    assert_eq!(flat.len(), 3 * n);
    let onset = Array2::from_shape_vec((t, N_PITCH), flat[0..n].to_vec()).unwrap();
    let sustain = Array2::from_shape_vec((t, N_PITCH), flat[n..2 * n].to_vec()).unwrap();
    let offset = Array2::from_shape_vec((t, N_PITCH), flat[2 * n..3 * n].to_vec()).unwrap();
    (onset, sustain, offset)
}

// ---------------------------------------------------------------------
// 1. harmonica_cqt_window_{cpu,dml}
// ---------------------------------------------------------------------

fn run_cqt_window_test(device: Device, tol: f32) {
    let Some(dir) = golden_dir() else { return };
    let window_audio = read_f32_file(&dir.join("window_audio.f32"));
    assert_eq!(window_audio.len(), WINDOW_SAMPLES);
    let golden_mag = Array2::from_shape_vec((CHUNK, rekord_core::harmonica_model::N_BINS), read_f32_file(&dir.join("window_cqt_mag.f32"))).unwrap();

    let mut model = match device {
        Device::Cpu => HarmonicaModel::load_on(ModelSize::Small, Device::Cpu).expect("cpu load must not fail"),
        Device::DirectML => match try_load_dml(ModelSize::Small) {
            Some(m) => m,
            None => return,
        },
    };

    let mag = model.cqt(&window_audio).unwrap();
    let d = max_abs_diff(&mag, &golden_mag);
    eprintln!("harmonica_cqt_window {device:?}: max|delta| = {d:.3e}");
    assert!(d <= tol, "cqt max|delta| {d} exceeds tolerance {tol}");
}

#[test]
fn harmonica_cqt_window_cpu() {
    run_cqt_window_test(Device::Cpu, 1e-4);
}

#[test]
fn harmonica_cqt_window_dml() {
    run_cqt_window_test(Device::DirectML, 1e-3);
}

// ---------------------------------------------------------------------
// 2. harmonica_net_window_{cpu,dml} (all 3 sizes)
// ---------------------------------------------------------------------

fn run_net_window_test(device: Device, tol: f32) {
    let Some(dir) = golden_dir() else { return };
    let cqt_mag = Array2::from_shape_vec((CHUNK, rekord_core::harmonica_model::N_BINS), read_f32_file(&dir.join("window_cqt_mag.f32"))).unwrap();

    // Block 40 (f=8): last = 335, w0 = last - (CHUNK-1) = 180 >= 0, so this
    // golden window needs no negative-frame row masking -- confirm that.
    let last: i64 = 335;
    let w0 = last - (CHUNK as i64 - 1);
    assert_eq!(w0, 180);
    assert!(w0 >= 0);

    for size in [ModelSize::Small, ModelSize::Medium, ModelSize::Large] {
        let mut model = match device {
            Device::Cpu => HarmonicaModel::load_on(size, Device::Cpu).unwrap_or_else(|e| panic!("cpu load must not fail for {size}: {e}")),
            Device::DirectML => match try_load_dml(size) {
                Some(m) => m,
                None => return,
            },
        };

        let mut parts = model.net(cqt_mag.view()).unwrap();
        assert_eq!(parts.len(), 1);
        let PartProbs { onset, sustain, offset, .. } = parts.remove(0);
        let (g_onset, g_sustain, g_offset) = split_three(read_f32_file(&dir.join(format!("{size}_window_probs.f32"))), CHUNK);

        let d_on = max_abs_diff(&onset, &g_onset);
        let d_sus = max_abs_diff(&sustain, &g_sustain);
        let d_off = max_abs_diff(&offset, &g_offset);
        eprintln!("harmonica_net_window {device:?} {size}: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}");
        assert!(d_on <= tol, "{size} {device:?} onset max|delta| {d_on} > {tol}");
        assert!(d_sus <= tol, "{size} {device:?} sustain max|delta| {d_sus} > {tol}");
        assert!(d_off <= tol, "{size} {device:?} offset max|delta| {d_off} > {tol}");
    }
}

#[test]
fn harmonica_net_window_cpu() {
    run_net_window_test(Device::Cpu, 1e-4);
}

#[test]
fn harmonica_net_window_dml() {
    run_net_window_test(Device::DirectML, 1e-3);
}

// ---------------------------------------------------------------------
// 3. harmonica_stream_{cpu,dml} (all 3 sizes, f=8)
// ---------------------------------------------------------------------

fn run_stream_test(device: Device, tol: f32) {
    let Some(dir) = golden_dir() else { return };
    let t = read_meta_t(&dir);
    let audio = read_f32_file(&dir.join("audio_16k.f32"));
    let future_frames = 8usize;
    // Irregular chunk sizes, cycled, deliberately not aligned to BLOCK_FRAMES.
    let chunk_pattern = [1usize, 4095, 4097, 12345, 777, 30000];

    let mut small_cpu_result: Option<StreamResult> = None;

    for size in [ModelSize::Small, ModelSize::Medium, ModelSize::Large] {
        let model = match device {
            Device::Cpu => HarmonicaModel::load_on(size, Device::Cpu).unwrap_or_else(|e| panic!("cpu load must not fail for {size}: {e}")),
            Device::DirectML => match try_load_dml(size) {
                Some(m) => m,
                None => continue,
            },
        };

        let result = collect_stream(model, &audio, future_frames, &chunk_pattern);
        assert_eq!(result.frames_produced, t, "expected exactly T={t} frames");
        assert_eq!(result.parts.len(), 1, "a single-part model has exactly one part");
        let probs = &result.parts[0];
        assert_eq!(probs.part, Part::All);

        let golden = split_three(read_f32_file(&dir.join(format!("{size}_probs.f32"))), t);
        check_stream_probs(&format!("harmonica_stream {device:?} {size}"), probs, &golden, t, tol);
        print_block_timings(&format!("harmonica_stream {device:?} {size}"), &result.timings_ms);

        let (th_on, th_sus) = size.thresholds().unwrap();
        let mut decoder = StreamingDecoder::new(th_on, th_sus);
        let mut notes = decoder.push(probs.onset.view(), probs.sustain.view());
        notes.extend(decoder.finish());
        let golden_notes = read_notes_json(&dir.join(format!("{size}_notes.json")));
        check_notes_match(&notes, &golden_notes, &golden.0, &golden.1, th_on, th_sus, tol, t, &format!("harmonica_stream {device:?} {size}"));

        if device == Device::Cpu && size == ModelSize::Small {
            small_cpu_result = Some(result);
        }
    }

    if let Some(chunked) = small_cpu_result {
        let model = HarmonicaModel::load_on(ModelSize::Small, Device::Cpu).unwrap();
        let whole = collect_stream(model, &audio, future_frames, &[audio.len()]);
        let (w, c) = (&whole.parts[0], &chunked.parts[0]);
        let d_on = max_abs_diff(&w.onset, &c.onset);
        let d_sus = max_abs_diff(&w.sustain, &c.sustain);
        let d_off = max_abs_diff(&w.offset, &c.offset);
        eprintln!("harmonica_stream whole-vs-chunked (small, cpu): max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}");
        assert!(d_on < 1e-6 && d_sus < 1e-6 && d_off < 1e-6, "whole-audio push must match irregular chunking to within 1e-6");
    }
}

#[test]
fn harmonica_stream_cpu() {
    run_stream_test(Device::Cpu, 1e-4);
}

#[test]
fn harmonica_stream_dml() {
    run_stream_test(Device::DirectML, 1e-3);
}

// ---------------------------------------------------------------------
// 4. harmonica_decode_golden (no ONNX)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum ChunkMode {
    Blocks8,
    OneAtATime,
    AllAtOnce,
}

#[test]
fn harmonica_decode_golden() {
    let Some(dir) = golden_dir() else { return };
    let t = read_meta_t(&dir);

    for size in [ModelSize::Small, ModelSize::Medium, ModelSize::Large] {
        let (th_on, th_sus) = size.thresholds().unwrap();
        let (onset, sustain, _offset) = split_three(read_f32_file(&dir.join(format!("{size}_probs.f32"))), t);
        let golden_notes = read_notes_json(&dir.join(format!("{size}_notes.json")));
        let expected: HashSet<(i64, i64, i32)> =
            golden_notes.iter().map(|&(a, b, c)| (frame_of(a), frame_of(b), c.round() as i32)).collect();

        for mode in [ChunkMode::Blocks8, ChunkMode::OneAtATime, ChunkMode::AllAtOnce] {
            let mut decoder = StreamingDecoder::new(th_on, th_sus);
            let mut notes = Vec::new();
            match mode {
                ChunkMode::Blocks8 => {
                    let mut r = 0;
                    while r < t {
                        let end = (r + 8).min(t);
                        notes.extend(decoder.push(onset.slice(s![r..end, ..]), sustain.slice(s![r..end, ..])));
                        r = end;
                    }
                }
                ChunkMode::OneAtATime => {
                    for r in 0..t {
                        notes.extend(decoder.push(onset.slice(s![r..r + 1, ..]), sustain.slice(s![r..r + 1, ..])));
                    }
                }
                ChunkMode::AllAtOnce => {
                    notes.extend(decoder.push(onset.view(), sustain.view()));
                }
            }
            notes.extend(decoder.finish());
            let got: HashSet<(i64, i64, i32)> = notes.iter().map(|n| (n.start_frame as i64, n.end_frame as i64, n.pitch_midi)).collect();
            assert_eq!(got, expected, "{size} {mode:?}: decode mismatch vs golden notes");
        }
        eprintln!("harmonica_decode_golden {size}: {} notes, exact match in all 3 chunkings", expected.len());
    }
}

// ---------------------------------------------------------------------
// 5. harmonica_future16_rows (CPU, sizes small and large)
// ---------------------------------------------------------------------

#[test]
fn harmonica_future16_rows() {
    let Some(dir) = golden_dir() else { return };
    let audio = read_f32_file(&dir.join("audio_16k.f32"));
    let last = 335usize;
    let needed = last * HOP + 1;
    assert!(audio.len() >= needed, "golden audio too short for block 40/39's window");
    let prefix = &audio[..needed];

    for size in [ModelSize::Small, ModelSize::Large] {
        let (g_onset, g_sustain, g_offset) =
            split_three(read_f32_file(&dir.join(format!("{size}_window_probs.f32"))), CHUNK);

        // f=16, block 39: same "now" (last=335) as f=8's block 40, so its
        // window -- and thus the full [CHUNK, 88] net output -- is
        // identical; only the extracted local row range differs.
        {
            let model = HarmonicaModel::load_on(size, Device::Cpu).unwrap();
            let mut streamer = BlockStreamer::new(model, 16);
            let mut last_block = None;
            streamer.push(prefix, |b| last_block = Some(b)).unwrap();
            let b = last_block.expect("block 39 (f=16) should have been produced");
            assert_eq!(b.first_frame, 39 * 8);
            let d_on = max_abs_diff(&b.parts[0].onset, &g_onset.slice(s![132..140, ..]).to_owned());
            let d_sus = max_abs_diff(&b.parts[0].sustain, &g_sustain.slice(s![132..140, ..]).to_owned());
            let d_off = max_abs_diff(&b.parts[0].offset, &g_offset.slice(s![132..140, ..]).to_owned());
            eprintln!("harmonica_future16_rows {size} f=16 block39: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}");
            assert!(d_on <= 1e-4 && d_sus <= 1e-4 && d_off <= 1e-4);
        }

        // f=8, block 40 -- the other half of the same golden window.
        {
            let model = HarmonicaModel::load_on(size, Device::Cpu).unwrap();
            let mut streamer = BlockStreamer::new(model, 8);
            let mut last_block = None;
            streamer.push(prefix, |b| last_block = Some(b)).unwrap();
            let b = last_block.expect("block 40 (f=8) should have been produced");
            assert_eq!(b.first_frame, 40 * 8);
            let d_on = max_abs_diff(&b.parts[0].onset, &g_onset.slice(s![140..148, ..]).to_owned());
            let d_sus = max_abs_diff(&b.parts[0].sustain, &g_sustain.slice(s![140..148, ..]).to_owned());
            let d_off = max_abs_diff(&b.parts[0].offset, &g_offset.slice(s![140..148, ..]).to_owned());
            eprintln!("harmonica_future16_rows {size} f=8 block40: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}");
            assert!(d_on <= 1e-4 && d_sus <= 1e-4 && d_off <= 1e-4);
        }
    }
}
