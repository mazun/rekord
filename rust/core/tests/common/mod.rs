//! Helpers for the golden-vector tests (`okarina_golden.rs`).
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use ndarray::{s, Array2};
use rekord_core::harmonica_decode::DecodedNote;
use rekord_core::harmonica_model::{Device, HarmonicaModel, ModelSize, Part, FRAME_S, HOP, MIN_MIDI, N_PITCH};
use rekord_core::harmonica_stream::{BlockProbs, BlockStreamer};

pub fn read_f32_file(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    assert_eq!(bytes.len() % 4, 0, "{}: not a whole number of f32s", path.display());
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
}

pub fn read_meta(dir: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(dir.join("meta.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

pub fn read_meta_t(dir: &Path) -> usize {
    read_meta(dir)["T"].as_u64().unwrap() as usize
}

pub fn read_notes_json(path: &Path) -> Vec<(f64, f64, f64)> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()))
}

pub fn max_abs_diff(a: &Array2<f32>, b: &Array2<f32>) -> f32 {
    assert_eq!(a.dim(), b.dim(), "shape mismatch: {:?} vs {:?}", a.dim(), b.dim());
    a.iter().zip(b.iter()).fold(0.0f32, |m, (&x, &y)| m.max((x - y).abs()))
}

pub fn max_abs_diff_rows(a: &Array2<f32>, b: &Array2<f32>, rows: std::ops::Range<usize>) -> f32 {
    a.slice(s![rows.clone(), ..]).iter().zip(b.slice(s![rows, ..]).iter()).fold(0.0f32, |m, (&x, &y)| m.max((x - y).abs()))
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

pub fn frame_of(seconds: f64) -> i64 {
    (seconds / FRAME_S as f64).round() as i64
}

/// Whether a note-key mismatch between our decode and the golden decode is
/// "explained" by the golden reference being within `tol` of a decision
/// boundary near `frame` for pitch `p`: a near-threshold onset/sustain
/// value, or a near-tied plateau, within +/-2 frames.
pub fn near_tie(onset: &Array2<f32>, sustain: &Array2<f32>, th_on: f32, th_sus: f32, frame: usize, p: usize, tol: f32, t: usize) -> bool {
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

/// Compares note keys (start frame, end frame, midi) to harmonica-repro's
/// golden notes, printing every mismatch and failing only on ones not
/// explained by a near-tie in the golden probabilities (see `near_tie`).
pub fn check_note_keys_match(
    ours_keys: &HashSet<(i64, i64, i32)>,
    golden: &[(f64, f64, f64)],
    g_onset: &Array2<f32>,
    g_sustain: &Array2<f32>,
    th_on: f32,
    th_sus: f32,
    tol: f32,
    t: usize,
    label: &str,
) {
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
    eprintln!("{label}: {} note(s), {mismatch_count} mismatch(es), {} unexplained", ours_keys.len(), unexplained.len());
    assert!(unexplained.is_empty(), "{label}: {} unexplained note mismatches: {unexplained:?}", unexplained.len());
}

/// `check_note_keys_match` for decoder output.
pub fn check_notes_match(
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
    let keys: HashSet<(i64, i64, i32)> = ours.iter().map(|n| (n.start_frame as i64, n.end_frame as i64, n.pitch_midi)).collect();
    check_note_keys_match(&keys, golden, g_onset, g_sustain, th_on, th_sus, tol, t, label);
}

/// Tries to load `size` on DirectML; prints a skip message and returns None
/// if that fails (DirectML unavailable on this machine), rather than
/// failing the test.
pub fn try_load_dml(size: ModelSize) -> Option<HarmonicaModel> {
    match HarmonicaModel::load_on(size, Device::DirectML) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("skipping (DirectML unavailable): {e}");
            None
        }
    }
}

/// One output part's full-stream [T, N_PITCH] probability arrays.
pub struct PartArrays {
    pub part: Part,
    pub onset: Array2<f32>,
    pub sustain: Array2<f32>,
    pub offset: Array2<f32>,
}

pub struct StreamResult {
    pub parts: Vec<PartArrays>,
    pub timings_ms: Vec<f64>,
    pub frames_produced: usize,
}

/// Feeds `audio` through a fresh `BlockStreamer` in the given (cyclically
/// repeated) chunk sizes, then `finish()`s it, reassembling the full
/// per-part [T, N_PITCH] probability arrays and per-block wall-clock timings.
pub fn collect_stream(model: HarmonicaModel, audio: &[f32], future_frames: usize, chunk_sizes: &[usize]) -> StreamResult {
    let t_hint = audio.len().div_ceil(HOP);
    let mut parts: Vec<PartArrays> = model
        .parts()
        .iter()
        .map(|&part| PartArrays {
            part,
            onset: Array2::zeros((t_hint, N_PITCH)),
            sustain: Array2::zeros((t_hint, N_PITCH)),
            offset: Array2::zeros((t_hint, N_PITCH)),
        })
        .collect();
    let mut frames_produced = 0usize;
    let mut timings_ms = Vec::new();
    let mut last_instant = Instant::now();

    let mut on_block = |block: BlockProbs| {
        let now = Instant::now();
        timings_ms.push(now.duration_since(last_instant).as_secs_f64() * 1000.0);
        last_instant = now;
        assert_eq!(block.first_frame, frames_produced, "blocks must arrive contiguously from 0");
        let n = block.parts[0].onset.nrows();
        for (dst, src) in parts.iter_mut().zip(&block.parts) {
            assert_eq!(dst.part, src.part);
            let rows = s![block.first_frame..block.first_frame + n, ..];
            dst.onset.slice_mut(rows).assign(&src.onset);
            dst.sustain.slice_mut(rows).assign(&src.sustain);
            dst.offset.slice_mut(rows).assign(&src.offset);
        }
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
    StreamResult { parts, timings_ms, frames_produced }
}

/// Compares one part's streamed probabilities to the golden ones, printing
/// the deltas and asserting `tol` on steady-state frames and a looser bound
/// on the startup frames.
pub fn check_stream_probs(label: &str, result: &PartArrays, golden: &(Array2<f32>, Array2<f32>, Array2<f32>), t: usize, tol: f32) {
    let (g_onset, g_sustain, g_offset) = golden;
    let d_on = max_abs_diff(&result.onset, g_onset);
    let d_sus = max_abs_diff(&result.sustain, g_sustain);
    let d_off = max_abs_diff(&result.offset, g_offset);
    eprintln!("{label}: max|delta| onset={d_on:.2e} sustain={d_sus:.2e} offset={d_off:.2e}");

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
    let d_on_startup = max_abs_diff_rows(&result.onset, g_onset, 0..STARTUP_FRAMES);
    let d_sus_startup = max_abs_diff_rows(&result.sustain, g_sustain, 0..STARTUP_FRAMES);
    let d_off_startup = max_abs_diff_rows(&result.offset, g_offset, 0..STARTUP_FRAMES);
    let d_on_steady = max_abs_diff_rows(&result.onset, g_onset, STARTUP_FRAMES..t);
    let d_sus_steady = max_abs_diff_rows(&result.sustain, g_sustain, STARTUP_FRAMES..t);
    let d_off_steady = max_abs_diff_rows(&result.offset, g_offset, STARTUP_FRAMES..t);
    eprintln!("{label}: startup (frames<{STARTUP_FRAMES}) max|delta| onset={d_on_startup:.2e} sustain={d_sus_startup:.2e} offset={d_off_startup:.2e}");
    eprintln!("{label}: steady-state (frames>={STARTUP_FRAMES}) max|delta| onset={d_on_steady:.2e} sustain={d_sus_steady:.2e} offset={d_off_steady:.2e}");
    assert!(d_on_startup <= STARTUP_TOL, "{label} startup onset max|delta| {d_on_startup} > {STARTUP_TOL}");
    assert!(d_sus_startup <= STARTUP_TOL, "{label} startup sustain max|delta| {d_sus_startup} > {STARTUP_TOL}");
    assert!(d_off_startup <= STARTUP_TOL, "{label} startup offset max|delta| {d_off_startup} > {STARTUP_TOL}");
    assert!(d_on_steady <= tol, "{label} steady-state onset max|delta| {d_on_steady} > {tol}");
    assert!(d_sus_steady <= tol, "{label} steady-state sustain max|delta| {d_sus_steady} > {tol}");
    assert!(d_off_steady <= tol, "{label} steady-state offset max|delta| {d_off_steady} > {tol}");
}

pub fn print_block_timings(label: &str, timings_ms: &[f64]) {
    let mut sorted_ms = timings_ms.to_vec();
    sorted_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!("{label}: per-block ms median={:.2} p99={:.2} (n={})", percentile(&sorted_ms, 0.5), percentile(&sorted_ms, 0.99), sorted_ms.len());
}
