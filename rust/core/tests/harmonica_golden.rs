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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ndarray::{s, Array2};
use rekord_core::harmonica_decode::{DecodedNote, StreamingDecoder};
use rekord_core::harmonica_model::{Device, HarmonicaModel, ModelSize, CHUNK, FRAME_S, HOP, MIN_MIDI, N_PITCH, WINDOW_SAMPLES};
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

/// Tries to load `size` on DirectML; prints a skip message and returns None
/// if that fails (DirectML unavailable on this machine), rather than
/// failing the test.
fn try_load_dml(size: ModelSize) -> Option<HarmonicaModel> {
    match HarmonicaModel::load_on(size, Device::DirectML) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("skipping (DirectML unavailable): {e}");
            None
        }
    }
}

fn read_f32_file(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    assert_eq!(bytes.len() % 4, 0, "{}: not a whole number of f32s", path.display());
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn read_meta_t(dir: &Path) -> usize {
    let text = std::fs::read_to_string(dir.join("meta.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["T"].as_u64().unwrap() as usize
}

fn read_notes_json(path: &Path) -> Vec<(f64, f64, f64)> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()))
}

fn max_abs_diff(a: &Array2<f32>, b: &Array2<f32>) -> f32 {
    assert_eq!(a.dim(), b.dim(), "shape mismatch: {:?} vs {:?}", a.dim(), b.dim());
    a.iter().zip(b.iter()).fold(0.0f32, |m, (&x, &y)| m.max((x - y).abs()))
}

fn max_abs_diff_rows(a: &Array2<f32>, b: &Array2<f32>, rows: std::ops::Range<usize>) -> f32 {
    a.slice(s![rows.clone(), ..]).iter().zip(b.slice(s![rows, ..]).iter()).fold(0.0f32, |m, (&x, &y)| m.max((x - y).abs()))
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

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn frame_of(seconds: f64) -> i64 {
    (seconds / FRAME_S as f64).round() as i64
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

        let [onset, sustain, offset] = model.net(cqt_mag.view()).unwrap();
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

struct StreamResult {
    onset: Array2<f32>,
    sustain: Array2<f32>,
    offset: Array2<f32>,
    timings_ms: Vec<f64>,
    frames_produced: usize,
}

/// Feeds `audio` through a fresh `BlockStreamer` in the given (cyclically
/// repeated) chunk sizes, then `finish()`s it, reassembling the full
/// [T, N_PITCH] probability arrays and per-block wall-clock timings.
fn collect_stream(model: HarmonicaModel, audio: &[f32], future_frames: usize, chunk_sizes: &[usize]) -> StreamResult {
    let t_hint = audio.len().div_ceil(HOP);
    let mut onset = Array2::<f32>::zeros((t_hint, N_PITCH));
    let mut sustain = Array2::<f32>::zeros((t_hint, N_PITCH));
    let mut offset = Array2::<f32>::zeros((t_hint, N_PITCH));
    let mut frames_produced = 0usize;
    let mut timings_ms = Vec::new();
    let mut last_instant = Instant::now();

    let mut on_block = |block: rekord_core::harmonica_stream::BlockProbs| {
        let now = Instant::now();
        timings_ms.push(now.duration_since(last_instant).as_secs_f64() * 1000.0);
        last_instant = now;
        assert_eq!(block.first_frame, frames_produced, "blocks must arrive contiguously from 0");
        let n = block.onset.nrows();
        onset.slice_mut(s![block.first_frame..block.first_frame + n, ..]).assign(&block.onset);
        sustain.slice_mut(s![block.first_frame..block.first_frame + n, ..]).assign(&block.sustain);
        offset.slice_mut(s![block.first_frame..block.first_frame + n, ..]).assign(&block.offset);
        frames_produced += n;
    };

    let mut streamer = BlockStreamer::new(model, future_frames);
    let mut pos = 0usize;
    let mut cycle = chunk_sizes.iter().cycle();
    while pos < audio.len() {
        let want = *cycle.next().unwrap();
        let take = want.min(audio.len() - pos);
        streamer.push(&audio[pos..pos + take], &mut on_block).unwrap();
        pos += take;
    }
    streamer.finish(&mut on_block).unwrap();
    drop(on_block);

    assert_eq!(frames_produced, t_hint, "expected frames 0..{t_hint} contiguously, got {frames_produced}");
    StreamResult { onset, sustain, offset, timings_ms, frames_produced }
}

/// Whether a note-key mismatch between our decode and the golden decode is
/// "explained" by the golden reference being within `tol` of a decision
/// boundary near `frame` for pitch `p`: a near-threshold onset/sustain
/// value, or a near-tied plateau, within +/-2 frames.
fn near_tie(onset: &Array2<f32>, sustain: &Array2<f32>, th_on: f32, th_sus: f32, frame: usize, p: usize, tol: f32, t: usize) -> bool {
    if t == 0 || p >= N_PITCH {
        return false;
    }
    let frame = frame.min(t - 1);
    let lo = frame.saturating_sub(2);
    let hi = (frame + 2).min(t - 1);
    for f in lo..=hi {
        let on = onset[[f, p]];
        if (on - th_on).abs() < tol {
            return true;
        }
        if f + 1 < t && (on - onset[[f + 1, p]]).abs() < tol {
            return true;
        }
        if f > 0 && (on - onset[[f - 1, p]]).abs() < tol {
            return true;
        }
        if (sustain[[f, p]] - th_sus).abs() < tol {
            return true;
        }
    }
    false
}

/// Compares our decoded notes to harmonica-repro's `<size>_notes.json`,
/// printing every mismatch and failing only on ones not explained by a
/// near-tie in the golden probabilities (see `near_tie`).
fn check_notes_match(
    ours: &[DecodedNote],
    golden: &[(f64, f64, f64)],
    g_onset: &Array2<f32>,
    g_sustain: &Array2<f32>,
    th_on: f32,
    th_sus: f32,
    tol: f32,
    t: usize,
    label: &str,
) {
    let ours_keys: HashSet<(i64, i64, i32)> = ours.iter().map(|n| (n.start_frame as i64, n.end_frame as i64, n.pitch_midi)).collect();
    let golden_keys: HashSet<(i64, i64, i32)> =
        golden.iter().map(|&(on_s, off_s, midi)| (frame_of(on_s), frame_of(off_s), midi.round() as i32)).collect();

    let mut unexplained = Vec::new();
    let mut mismatch_count = 0;
    for key in ours_keys.symmetric_difference(&golden_keys) {
        mismatch_count += 1;
        let (start, end, midi) = *key;
        let p = (midi - MIN_MIDI).max(0) as usize;
        let explained = near_tie(g_onset, g_sustain, th_on, th_sus, start.max(0) as usize, p, tol, t)
            || near_tie(g_onset, g_sustain, th_on, th_sus, end.max(0) as usize, p, tol, t);
        let which = if ours_keys.contains(key) { "ours-only" } else { "golden-only" };
        eprintln!("{label}: note mismatch [{which}] start={start} end={end} midi={midi} explained={explained}");
        if !explained {
            unexplained.push(*key);
        }
    }
    eprintln!("{label}: {} note(s), {mismatch_count} mismatch(es), {} unexplained", ours.len(), unexplained.len());
    assert!(unexplained.is_empty(), "{label}: {} unexplained note mismatches: {unexplained:?}", unexplained.len());
}

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

        let (g_onset, g_sustain, g_offset) = split_three(read_f32_file(&dir.join(format!("{size}_probs.f32"))), t);
        let d_on = max_abs_diff(&result.onset, &g_onset);
        let d_sus = max_abs_diff(&result.sustain, &g_sustain);
        let d_off = max_abs_diff(&result.offset, &g_offset);
        eprintln!("harmonica_stream {device:?} {size}: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}");

        // Frames < STARTUP_FRAMES (f=8: blocks 0-17, whose window still has
        // rows zeroed for frames before the stream started) get a looser
        // bound than `tol`. The cause is the ONNX Runtime binary, not this
        // code: `ort` statically links pyke's own build of ONNX Runtime 1.28
        // (+directml), while Microsoft's Python onnxruntime -- 1.24.4, which
        // made the golden vectors, and 1.28.0 alike -- reproduces them
        // exactly. Checked (2026-09-27) by running the same masked CQT
        // through Python onnxruntime 1.28.0 and through net() here: they
        // differ by up to 8.7e-4 at block 0, 1.4e-4 at block 5, 1.5e-5 at
        // block 12 and 1.9e-6 at block 40, and BlockStreamer's blocks 0/5/12
        // equal net() on the Python-built input bit for bit, so the window,
        // mask and row choice are right. Windows that are mostly zero rows
        // amplify small numeric differences between builds (the model
        // normalizes over the whole window). Notes are unaffected.
        const STARTUP_FRAMES: usize = 144;
        const STARTUP_TOL: f32 = 2e-3;
        let d_on_startup = max_abs_diff_rows(&result.onset, &g_onset, 0..STARTUP_FRAMES);
        let d_sus_startup = max_abs_diff_rows(&result.sustain, &g_sustain, 0..STARTUP_FRAMES);
        let d_off_startup = max_abs_diff_rows(&result.offset, &g_offset, 0..STARTUP_FRAMES);
        let d_on_steady = max_abs_diff_rows(&result.onset, &g_onset, STARTUP_FRAMES..t);
        let d_sus_steady = max_abs_diff_rows(&result.sustain, &g_sustain, STARTUP_FRAMES..t);
        let d_off_steady = max_abs_diff_rows(&result.offset, &g_offset, STARTUP_FRAMES..t);
        eprintln!(
            "harmonica_stream {device:?} {size}: startup (frames<{STARTUP_FRAMES}) max|delta| onset={d_on_startup:.2e} sustain={d_sus_startup:.2e} offset={d_off_startup:.2e}"
        );
        eprintln!(
            "harmonica_stream {device:?} {size}: steady-state (frames>={STARTUP_FRAMES}) max|delta| onset={d_on_steady:.2e} sustain={d_sus_steady:.2e} offset={d_off_steady:.2e}"
        );
        assert!(d_on_startup <= STARTUP_TOL, "{size} {device:?} startup onset max|delta| {d_on_startup} > {STARTUP_TOL}");
        assert!(d_sus_startup <= STARTUP_TOL, "{size} {device:?} startup sustain max|delta| {d_sus_startup} > {STARTUP_TOL}");
        assert!(d_off_startup <= STARTUP_TOL, "{size} {device:?} startup offset max|delta| {d_off_startup} > {STARTUP_TOL}");
        assert!(d_on_steady <= tol, "{size} {device:?} steady-state onset max|delta| {d_on_steady} > {tol}");
        assert!(d_sus_steady <= tol, "{size} {device:?} steady-state sustain max|delta| {d_sus_steady} > {tol}");
        assert!(d_off_steady <= tol, "{size} {device:?} steady-state offset max|delta| {d_off_steady} > {tol}");

        let mut sorted_ms = result.timings_ms.clone();
        sorted_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!(
            "harmonica_stream {device:?} {size}: per-block ms median={:.2} p99={:.2} (n={})",
            percentile(&sorted_ms, 0.5),
            percentile(&sorted_ms, 0.99),
            sorted_ms.len()
        );

        let (th_on, th_sus) = size.thresholds();
        let mut decoder = StreamingDecoder::new(th_on, th_sus);
        let mut notes = decoder.push(result.onset.view(), result.sustain.view());
        notes.extend(decoder.finish());
        let golden_notes = read_notes_json(&dir.join(format!("{size}_notes.json")));
        check_notes_match(&notes, &golden_notes, &g_onset, &g_sustain, th_on, th_sus, tol, t, &format!("harmonica_stream {device:?} {size}"));

        if device == Device::Cpu && size == ModelSize::Small {
            small_cpu_result = Some(result);
        }
    }

    if let Some(chunked) = small_cpu_result {
        let model = HarmonicaModel::load_on(ModelSize::Small, Device::Cpu).unwrap();
        let whole = collect_stream(model, &audio, future_frames, &[audio.len()]);
        let d_on = max_abs_diff(&whole.onset, &chunked.onset);
        let d_sus = max_abs_diff(&whole.sustain, &chunked.sustain);
        let d_off = max_abs_diff(&whole.offset, &chunked.offset);
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
        let (th_on, th_sus) = size.thresholds();
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
            let d_on = max_abs_diff(&b.onset, &g_onset.slice(s![132..140, ..]).to_owned());
            let d_sus = max_abs_diff(&b.sustain, &g_sustain.slice(s![132..140, ..]).to_owned());
            let d_off = max_abs_diff(&b.offset, &g_offset.slice(s![132..140, ..]).to_owned());
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
            let d_on = max_abs_diff(&b.onset, &g_onset.slice(s![140..148, ..]).to_owned());
            let d_sus = max_abs_diff(&b.sustain, &g_sustain.slice(s![140..148, ..]).to_owned());
            let d_off = max_abs_diff(&b.offset, &g_offset.slice(s![140..148, ..]).to_owned());
            eprintln!("harmonica_future16_rows {size} f=8 block40: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}");
            assert!(d_on <= 1e-4 && d_sus <= 1e-4 && d_off <= 1e-4);
        }
    }
}
