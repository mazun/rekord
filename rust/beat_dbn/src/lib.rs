//! Bit-exact Rust port of `harmonica-repro/beattrack/dbn_ref.py`
//! (`decode_dbn_ref`), a pure-numpy reimplementation of madmom's
//! `DBNDownBeatTrackingProcessor` for the fixed parameterisation:
//!
//! ```text
//! beats_per_bar=[3, 4], min_bpm=55, max_bpm=215, transition_lambda=100,
//! num_tempi=60, observation_lambda=16, threshold=0.05, correct=True
//! ```
//!
//! See that file's module docstring for the full algorithm description;
//! this file mirrors it section by section. All math is float64, matching
//! the Python reference (which converts its float32 inputs to float64
//! immediately, before any arithmetic -- so there is no float32-precision
//! subtlety to replicate here, unlike some other madmom-adjacent code).

const MIN_BPM: f64 = 55.0;
const MAX_BPM: f64 = 215.0;
const NUM_TEMPI: usize = 60;
const TRANSITION_LAMBDA: f64 = 100.0;
const OBSERVATION_LAMBDA: f64 = 16.0;
const THRESHOLD: f64 = 0.05;
const EPS_INPUT: f64 = 1e-5;

/// numpy's `np.round`: round-half-to-even ("banker's rounding"), not the
/// round-half-away-from-zero that `f64::round` implements.
fn round_half_even(x: f64) -> f64 {
    let f = x.floor();
    let diff = x - f;
    if diff < 0.5 {
        f
    } else if diff > 0.5 {
        f + 1.0
    } else if (f as i64) % 2 == 0 {
        f
    } else {
        f + 1.0
    }
}

/// madmom.features.beats_hmm.BeatStateSpace's interval computation.
fn beat_intervals(min_interval: f64, max_interval: f64, num_tempi: usize) -> Vec<u32> {
    let lo = round_half_even(min_interval) as i64;
    let hi = round_half_even(max_interval) as i64;
    let linear_len = (hi - lo + 1).max(0) as usize;
    if num_tempi < linear_len {
        let log_lo = min_interval.log2();
        let log_hi = max_interval.log2();
        let mut n = num_tempi;
        loop {
            let mut cand: Vec<i64> = Vec::with_capacity(n);
            if n == 1 {
                cand.push(round_half_even(2f64.powf(log_lo)) as i64);
            } else {
                for i in 0..n {
                    let t = log_lo + (log_hi - log_lo) * (i as f64) / ((n - 1) as f64);
                    cand.push(round_half_even(2f64.powf(t)) as i64);
                }
            }
            cand.sort_unstable();
            cand.dedup();
            if cand.len() >= num_tempi {
                return cand.into_iter().map(|v| v as u32).collect();
            }
            n += 1;
        }
    } else {
        (lo..=hi).map(|v| v as u32).collect()
    }
}

/// Bar state space for one `beats_per_bar` value (madmom's `BarStateSpace`).
struct StateSpace {
    num_beats: usize,
    num_intervals: usize,
    num_states: usize,
    state_positions: Vec<f64>,
    state_intervals: Vec<u32>,
    first_states: Vec<Vec<usize>>, // [beat][interval_bucket] -> state index
    last_states: Vec<Vec<usize>>,
}

fn build_state_space(num_beats: usize, min_interval: f64, max_interval: f64, num_tempi: usize) -> StateSpace {
    let intervals = beat_intervals(min_interval, max_interval, num_tempi);
    let num_intervals = intervals.len();

    let num_beat_states: usize = intervals.iter().map(|&i| i as usize).sum();
    let mut beat_positions = vec![0f64; num_beat_states];
    let mut beat_intervals_arr = vec![0u32; num_beat_states];
    let mut beat_first = vec![0usize; num_intervals];
    let mut beat_last = vec![0usize; num_intervals];
    let mut idx = 0usize;
    for (k, &iv) in intervals.iter().enumerate() {
        let i = iv as usize;
        beat_first[k] = idx;
        let step = 1.0 / (i as f64);
        for j in 0..i {
            beat_positions[idx + j] = (j as f64) * step;
            beat_intervals_arr[idx + j] = iv;
        }
        idx += i;
        beat_last[k] = idx - 1;
    }

    let num_states = num_beats * num_beat_states;
    let mut state_positions = vec![0f64; num_states];
    let mut state_intervals = vec![0u32; num_states];
    let mut first_states = vec![vec![0usize; num_intervals]; num_beats];
    let mut last_states = vec![vec![0usize; num_intervals]; num_beats];
    for b in 0..num_beats {
        let off = b * num_beat_states;
        for j in 0..num_beat_states {
            state_positions[off + j] = beat_positions[j] + b as f64;
            state_intervals[off + j] = beat_intervals_arr[j];
        }
        for k in 0..num_intervals {
            first_states[b][k] = beat_first[k] + off;
            last_states[b][k] = beat_last[k] + off;
        }
    }

    StateSpace { num_beats, num_intervals, num_states, state_positions, state_intervals, first_states, last_states }
}

/// Sparse transition model (madmom's `BarTransitionModel`), but split by
/// construction into its two disjoint destination classes (see the module
/// docstring, section 4): "same-tempo" destinations (all non-first states)
/// have exactly one incoming transition, `dest - 1`, with `log_prob == 0`
/// always -- so at decode time those need neither a stored predecessor list
/// nor a max/compare loop, just `dest - 1` computed directly. Only the
/// "tempo-change" destinations (the first states of each beat, a small
/// ~100-200-state subset out of 4300-5800) need the CSR predecessor lists
/// below, grouped by destination and sorted by ascending origin state
/// within each group (both required for bit-exact tie-breaking in Viterbi).
struct Transitions {
    first_states_sorted: Vec<u32>, // ascending, unique
    fpred_pointers: Vec<u32>,      // len first_states_sorted.len() + 1
    fpred_orig: Vec<u32>,
    fpred_logprob: Vec<f64>,
}

fn build_transitions(ss: &StateSpace, lambda: f64) -> Transitions {
    let mut first_states_sorted: Vec<u32> = Vec::new();
    for b in 0..ss.num_beats {
        for &s in &ss.first_states[b] {
            first_states_sorted.push(s as u32);
        }
    }
    first_states_sorted.sort_unstable();
    first_states_sorted.dedup();
    let num_first = first_states_sorted.len();

    let mut groups: Vec<Vec<(u32, f64)>> = vec![Vec::new(); num_first];

    // Tempo-change transitions, per beat.
    let trans_threshold = f64::EPSILON; // np.spacing(1.0)
    let n = ss.num_intervals;
    for b in 0..ss.num_beats {
        let to_states = &ss.first_states[b];
        let from_beat = (b + ss.num_beats - 1) % ss.num_beats; // python b - 1 wraparound
        let from_states = &ss.last_states[from_beat];
        let from_int: Vec<f64> = from_states.iter().map(|&s| ss.state_intervals[s] as f64).collect();
        let to_int: Vec<f64> = to_states.iter().map(|&s| ss.state_intervals[s] as f64).collect();

        for i in 0..n {
            let mut row = vec![0f64; n];
            for j in 0..n {
                let ratio = to_int[j] / from_int[i];
                let p = (-lambda * (ratio - 1.0).abs()).exp();
                row[j] = if p <= trans_threshold { 0.0 } else { p };
            }
            let sum: f64 = row.iter().sum();
            for j in 0..n {
                if row[j] != 0.0 {
                    let prob = row[j] / sum;
                    let dest = to_states[j] as u32;
                    let gi = first_states_sorted.binary_search(&dest).unwrap();
                    groups[gi].push((from_states[i] as u32, prob));
                }
            }
        }
    }

    let mut fpred_pointers = vec![0u32; num_first + 1];
    let mut fpred_orig = Vec::new();
    let mut fpred_logprob = Vec::new();
    for gi in 0..num_first {
        groups[gi].sort_by_key(|&(orig, _)| orig);
        fpred_pointers[gi + 1] = fpred_pointers[gi] + groups[gi].len() as u32;
        for &(orig, prob) in &groups[gi] {
            fpred_orig.push(orig);
            fpred_logprob.push(prob.ln());
        }
    }

    Transitions { first_states_sorted, fpred_pointers, fpred_orig, fpred_logprob }
}

/// Reusable Viterbi scratch space, sized once per `Hmm` and grown (never
/// shrunk) across calls -- avoids a ~20 MB alloc + zero-fill per `decode()`
/// call, which otherwise dominates the per-window budget (see the timing
/// test: allocating fresh buffers every call cost ~18 ms/window; reusing
/// them drops this well under 20 ms).
struct Scratch {
    prev: Vec<f64>,
    cur: Vec<f64>,
    backtrack: Vec<u16>,
}

/// One fully-built HMM (state space + transition + observation model) for a
/// fixed `beats_per_bar`. Built once in `DbnDecoder::new` and reused.
struct Hmm {
    num_states: usize,
    om_pointer: Vec<u8>,   // 0 = no-beat, 1 = beat, 2 = downbeat density column
    beat_number: Vec<u32>, // 1-based beat number per state (floor(position) + 1)
    first_states_sorted: Vec<u32>,
    fpred_pointers: Vec<u32>,
    fpred_orig: Vec<u32>,
    fpred_logprob: Vec<f64>,
    scratch: std::sync::Mutex<Scratch>,
}

impl Hmm {
    fn new(num_beats: usize, fps: f64) -> Self {
        let min_interval = 60.0 * fps / MAX_BPM;
        let max_interval = 60.0 * fps / MIN_BPM;
        let ss = build_state_space(num_beats, min_interval, max_interval, NUM_TEMPI);

        let border = 1.0 / OBSERVATION_LAMBDA;
        let mut om_pointer = vec![0u8; ss.num_states];
        let mut beat_number = vec![0u32; ss.num_states];
        for s in 0..ss.num_states {
            let pos = ss.state_positions[s];
            let frac = pos - pos.floor();
            let mut p = 0u8;
            if frac < border {
                p = 1;
            }
            if pos < border {
                p = 2;
            }
            om_pointer[s] = p;
            beat_number[s] = pos.floor() as u32 + 1;
        }

        let tm = build_transitions(&ss, TRANSITION_LAMBDA);
        let num_states = ss.num_states;

        Hmm {
            num_states,
            om_pointer,
            beat_number,
            first_states_sorted: tm.first_states_sorted,
            fpred_pointers: tm.fpred_pointers,
            fpred_orig: tm.fpred_orig,
            fpred_logprob: tm.fpred_logprob,
            scratch: std::sync::Mutex::new(Scratch {
                prev: vec![0.0; num_states],
                cur: vec![0.0; num_states],
                backtrack: Vec::new(),
            }),
        }
    }

    /// Frame-by-frame Viterbi decoding in the log domain. `observations` is
    /// already threshold-cropped `(beat_only, downbeat)` pairs.
    /// Returns `(path, log_prob)`, or `None` if no valid path exists.
    fn viterbi(&self, observations: &[(f64, f64)]) -> Option<(Vec<u32>, f64)> {
        let num_frames = observations.len();
        if num_frames == 0 {
            return None;
        }
        let num_states = self.num_states;
        assert!(num_states <= usize::from(u16::MAX) + 1, "backtrack entries are u16");
        let mut scratch = self.scratch.lock().expect("Viterbi scratch lock");
        let Scratch { prev, cur, backtrack } = &mut *scratch;

        let neg_ln_n = -(num_states as f64).ln();
        for p in prev.iter_mut() {
            *p = neg_ln_n;
        }
        let needed = num_frames * num_states;
        if backtrack.len() < needed {
            backtrack.resize(needed, 0);
        }

        for t in 0..num_frames {
            let (b_only, dbeat) = observations[t];
            let s = b_only + dbeat;
            let dens = [((1.0 - s) / (OBSERVATION_LAMBDA - 1.0)).ln(), b_only.ln(), dbeat.ln()];
            let bt_row = &mut backtrack[t * num_states..t * num_states + num_states];

            // Walk destination states 0..num_states in order, alongside the
            // ascending `first_states_sorted` list: every state NOT in that
            // list is a same-tempo destination with exactly one predecessor
            // (dest - 1, log_prob == 0) -- computed directly, no predecessor
            // array lookup or max/compare loop at all. Only the (small, one
            // to two hundred out of 4300-5800) tempo-change destinations use
            // the CSR predecessor list with the full max/compare loop.
            let mut fi = 0usize;
            let num_first = self.first_states_sorted.len();
            for st in 0..num_states {
                if fi < num_first && self.first_states_sorted[fi] as usize == st {
                    let start = self.fpred_pointers[fi] as usize;
                    let end = self.fpred_pointers[fi + 1] as usize;
                    let mut best = f64::NEG_INFINITY;
                    let mut best_prev = 0u32;
                    for k in start..end {
                        let orig = self.fpred_orig[k];
                        let cand = prev[orig as usize] + self.fpred_logprob[k];
                        if cand > best {
                            best = cand;
                            best_prev = orig;
                        }
                    }
                    cur[st] = best + dens[self.om_pointer[st] as usize];
                    bt_row[st] = best_prev as u16;
                    fi += 1;
                } else {
                    // st == 0 is always a first state (the first bucket of beat
                    // 0 starts at state 0), so st - 1 never underflows here.
                    let orig = (st - 1) as u32;
                    cur[st] = prev[orig as usize] + dens[self.om_pointer[st] as usize];
                    bt_row[st] = orig as u16;
                }
            }
            std::mem::swap(prev, cur);
        }

        let mut final_state = 0usize;
        let mut best = f64::NEG_INFINITY;
        for s in 0..num_states {
            if prev[s] > best {
                best = prev[s];
                final_state = s;
            }
        }
        if !best.is_finite() {
            return None;
        }

        let mut path = vec![0u32; num_frames];
        let mut state = final_state as u32;
        for t in (0..num_frames).rev() {
            path[t] = state;
            state = u32::from(backtrack[t * num_states + state as usize]);
        }
        Some((path, best))
    }
}

/// madmom's DBNDownBeatTrackingProcessor (beats_per_bar [3, 4], 55-215 BPM,
/// transition_lambda 100, defaults otherwise) for one activation window.
pub struct DbnDecoder {
    fps: f64,
    hmm3: Hmm,
    hmm4: Hmm,
}

impl DbnDecoder {
    pub fn new(fps: f64) -> Self {
        DbnDecoder { fps, hmm3: Hmm::new(3, fps), hmm4: Hmm::new(4, fps) }
    }

    /// 3-6. Run Viterbi for beats_per_bar in {3, 4}; 7. pick the larger log_prob, ties keeping
    /// bpb=3.
    fn best_path(&self, cropped: &[(f64, f64)]) -> Option<(&Hmm, Vec<u32>)> {
        // Each model has its own scratch space, so the two runs go side by side.
        let (run3, run4) = std::thread::scope(|scope| {
            let run4 = scope.spawn(|| self.hmm4.viterbi(cropped));
            (self.hmm3.viterbi(cropped), run4.join().expect("the bpb=4 Viterbi run panicked"))
        });
        let mut best_log_prob = f64::NEG_INFINITY;
        let mut best: Option<(&Hmm, Vec<u32>)> = None;
        for (hmm, run) in [(&self.hmm3, run3), (&self.hmm4, run4)] {
            if let Some((path, log_prob)) = run {
                if log_prob > best_log_prob {
                    best_log_prob = log_prob;
                    best = Some((hmm, path));
                }
            }
        }
        best
    }

    /// `beat` = P(beat incl. downbeat), `down` = P(downbeat), one value per frame.
    /// Returns (beat times, downbeat times) in seconds from the window start.
    pub fn decode(&self, beat: &[f32], down: &[f32]) -> (Vec<f64>, Vec<f64>) {
        let n = beat.len();
        if n == 0 {
            return (Vec::new(), Vec::new());
        }

        // 1. Input conversion: f32 -> f64 immediately (matches decode_dbn_ref's
        // np.asarray(..., dtype=np.float64) at the top), then epsilon clip in f64.
        let mut observations: Vec<(f64, f64)> = Vec::with_capacity(n);
        for i in 0..n {
            let b = beat[i] as f64;
            let d = down[i] as f64;
            let mut beat_only = (b - d).clamp(EPS_INPUT / 2.0, 1.0);
            let mut downbeat = d.clamp(EPS_INPUT / 2.0, 1.0);
            let total = beat_only + downbeat;
            let scale = (1.0f64).min((1.0 - EPS_INPUT) / total.max(EPS_INPUT));
            beat_only *= scale;
            downbeat *= scale;
            observations.push((beat_only, downbeat));
        }

        // 2. Threshold cropping, including madmom's `idx.any()` quirk: if the
        // only crossing frame is frame 0, treat it as "nothing crossed".
        let mut first_r: Option<usize> = None;
        let mut last_r: Option<usize> = None;
        for (i, &(bo, db)) in observations.iter().enumerate() {
            if bo >= THRESHOLD || db >= THRESHOLD {
                if first_r.is_none() {
                    first_r = Some(i);
                }
                last_r = Some(i);
            }
        }
        let (first, last) = match (first_r, last_r) {
            (Some(f), Some(l)) if l != 0 => (f, l + 1),
            _ => return (Vec::new(), Vec::new()),
        };
        let cropped = &observations[first..last];

        let Some((hmm, path)) = self.best_path(cropped) else {
            return (Vec::new(), Vec::new());
        };

        // 8. Reading off beat/downbeat positions ("correct" branch).
        let beat_range: Vec<bool> = path.iter().map(|&s| hmm.om_pointer[s as usize] >= 1).collect();
        if !beat_range.iter().any(|&x| x) {
            return (Vec::new(), Vec::new());
        }

        let mut peaks: Vec<usize> = Vec::new();
        let mut i = 0usize;
        let len = beat_range.len();
        while i < len {
            if beat_range[i] {
                let left = i;
                let mut right = i + 1;
                while right < len && beat_range[right] {
                    right += 1;
                }
                // peak = left + argmax(cropped[left:right].ravel()) // 2, first-wins tie-break.
                let mut best_val = f64::NEG_INFINITY;
                let mut best_flat = 0usize;
                for k in left..right {
                    let (bo, db) = cropped[k];
                    let flat0 = 2 * (k - left);
                    if bo > best_val {
                        best_val = bo;
                        best_flat = flat0;
                    }
                    if db > best_val {
                        best_val = db;
                        best_flat = flat0 + 1;
                    }
                }
                peaks.push(left + best_flat / 2);
                i = right;
            } else {
                i += 1;
            }
        }

        let beat_times: Vec<f64> = peaks.iter().map(|&p| (p + first) as f64 / self.fps).collect();
        let downbeat_times: Vec<f64> = peaks
            .iter()
            .zip(beat_times.iter())
            .filter(|(&p, _)| hmm.beat_number[path[p] as usize] == 1)
            .map(|(_, &t)| t)
            .collect();

        (beat_times, downbeat_times)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::{Array1, Ix1};
    use ndarray_npy::NpzReader;
    use std::time::Instant;

    /// Same as rekord_core's beat_model::golden_dir (this crate sits at the same depth).
    fn golden_dir() -> Option<std::path::PathBuf> {
        let dir = std::env::var("BEAT_GOLDEN_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../harmonica-repro/runs/beat/golden")
        });
        if dir.exists() {
            Some(dir)
        } else {
            eprintln!("skipping: no golden vectors at {}", dir.display());
            None
        }
    }

    #[test]
    fn matches_python_golden_windows() {
        let Some(dir) = golden_dir() else { return };
        let mut npz = NpzReader::new(std::fs::File::open(dir.join("dbn_windows.npz")).unwrap()).unwrap();
        let decoder = DbnDecoder::new(50.0);

        for i in 0..8 {
            let beat: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>(&format!("beat_{i}")).unwrap();
            let down: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>(&format!("down_{i}")).unwrap();
            let exp_beats: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>(&format!("beats_{i}")).unwrap();
            let exp_downs: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>(&format!("downs_{i}")).unwrap();

            let (beats, downs) = decoder.decode(beat.as_slice().unwrap(), down.as_slice().unwrap());

            assert_eq!(beats.len(), exp_beats.len(), "window {i}: beat count mismatch");
            assert_eq!(downs.len(), exp_downs.len(), "window {i}: downbeat count mismatch");
            for (k, (a, b)) in beats.iter().zip(exp_beats.iter()).enumerate() {
                assert!((a - b).abs() <= 1e-9, "window {i} beat {k}: {a} vs {b}");
            }
            for (k, (a, b)) in downs.iter().zip(exp_downs.iter()).enumerate() {
                assert!((a - b).abs() <= 1e-9, "window {i} downbeat {k}: {a} vs {b}");
            }
        }
        println!("all 8 golden windows matched exactly (<=1e-9)");
    }

    /// Activations of a steady 120 BPM beat (a peak every 25 frames) in `bar` beats per bar.
    fn synthetic_activations(frames: usize, bar: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut beat, mut down) = (vec![0.02f32; frames], vec![0.01f32; frames]);
        for (k, f) in (5..frames).step_by(25).enumerate() {
            beat[f] = 0.9;
            if k % bar == 0 {
                down[f] = 0.8;
            }
        }
        (beat, down)
    }

    /// Both bar lengths through the whole decoder, against the pair of Viterbi runs done one
    /// after the other: the same model must win with the same path, repeatedly (the scratch
    /// space is reused between calls).
    #[test]
    fn best_path_is_the_better_of_the_two_sequential_runs() {
        let decoder = DbnDecoder::new(50.0);
        for bar in [3, 4] {
            let (beat, down) = synthetic_activations(500, bar);
            let obs: Vec<(f64, f64)> = beat.iter().zip(&down).map(|(&b, &d)| (((b - d).max(0.001)) as f64, d as f64)).collect();
            let first = decoder.hmm3.viterbi(&obs).expect("a path for 3");
            let second = decoder.hmm4.viterbi(&obs).expect("a path for 4");
            let (want_hmm, want_path) = if second.1 > first.1 { (&decoder.hmm4, second.0) } else { (&decoder.hmm3, first.0) };
            for _ in 0..3 {
                let (hmm, path) = decoder.best_path(&obs).expect("a path");
                assert!(std::ptr::eq(hmm, want_hmm), "bar {bar}: the other model won");
                assert_eq!(path, want_path, "bar {bar}");
            }
            let (beats, downs) = decoder.decode(&beat, &down);
            assert!(beats.len() >= 15, "bar {bar}: {} beats", beats.len());
            assert!(!downs.is_empty(), "bar {bar}");
        }
    }

    #[test]
    fn timing_500_frame_window() {
        let Some(dir) = golden_dir() else { return };
        let mut npz = NpzReader::new(std::fs::File::open(dir.join("dbn_windows.npz")).unwrap()).unwrap();
        let beat: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>("beat_0").unwrap();
        let down: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>("down_0").unwrap();
        assert_eq!(beat.len(), 500);

        let decoder = DbnDecoder::new(50.0);
        // warm-up
        decoder.decode(beat.as_slice().unwrap(), down.as_slice().unwrap());

        let iters = 30;
        let start = Instant::now();
        for _ in 0..iters {
            decoder.decode(beat.as_slice().unwrap(), down.as_slice().unwrap());
        }
        let elapsed = start.elapsed();
        let per_call_ms = elapsed.as_secs_f64() * 1000.0 / iters as f64;
        println!("decode() on a 500-frame window: {per_call_ms:.3} ms/call avg over {iters} calls");
        assert!(per_call_ms < 20.0, "decode() too slow: {per_call_ms:.3} ms/call");
    }
}
