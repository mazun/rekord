//! Online beat/downbeat tracker: push-audio state machine, ported from
//! harmonica-repro/beattrack/online.py (`OnlineTracker`), which is the executable spec.
//! Also folds in the pieces it depends on from stream_eval.py (`_segments`, `_commit`),
//! song_change.py (`grid_agree`, `_agreement`, constants), decode.py (`_deduplicate_peaks`)
//! and stream_predict.py (`predicted_lines`, `_period`, `_bar_len`).
//!
//! Units: sample rate 22050 Hz, mel frames at 50 fps (hop 441 samples), frame i covers
//! samples [441 i - 512, 441 i + 512) of the stream (512 leading zero samples give Beat
//! This!'s center=True framing).
//!
//! Memory: `new` keeps the whole history of audio, spect and activations, like the Python
//! reference (the golden test compares those). `bounded` drops everything older than
//! `RETAIN_FRAMES` from the front of those buffers (`Tail`), for a live session that runs
//! for hours; the lines it produces are identical (tested). The line lists themselves stay
//! whole-history in both modes: ~2 floats/s per list is ~60 KB/h each.

use rekord_beat_dbn::DbnDecoder;
use crate::beat_model::{BeatModel, HOP, N_FFT, N_MELS, SAMPLE_RATE};
use anyhow::Result;
use ndarray::ArrayView2;
use std::ops::{Index, IndexMut, Range};
use std::time::Instant;

const FPS: f64 = 50.0;
const W: usize = 500; // mel frames, ~10s window for the network
const S: usize = 26; // net stride, frames (0.52s)
const F: usize = 25; // future frames withheld (0.5s)
const PEAK_WIN: usize = 4; // round(0.07 * 50), provisional peak-picking window
const DBN_STEP: usize = 25; // frames between fixed-lag DBN steps
const DBN_W: usize = 500; // DBN window, frames
const LAG: f64 = 4.0; // seconds
const NOV_STEP: usize = 25; // frames between song-change detector steps
const TH_NOV: f64 = 0.5;
const TH_GRID: f64 = 0.7;

// song_change.py constants
const SC_CMIN: f64 = 1.0;
const SC_CMAX: f64 = 3.0;
const SC_LW: f64 = 8.0;
const SC_COOLDOWN: f64 = 8.0;
const SC_W: usize = 500; // grid_agree's own DBN window, frames (coincides with W above)

// stream_predict.py constants (mode is always "prov" here, so COMMIT_DELAY is unused)
const PROV_DELAY: f64 = 0.57;
const MAX_AHEAD: i64 = 2;

// `bounded`: frames of spect/activations kept behind the newest one. The oldest reads are
// the song-change detector's LW+CMAX (550 spect rows) and a reset's DBN window, which starts
// up to DBN_W before a cut that is itself up to CMAX + DBN_STEP behind `filled` (~700
// activation frames); 30s leaves a wide margin.
const RETAIN_FRAMES: usize = 1500;

/// A `Vec` indexed by absolute position whose front can be dropped (`trim_to`), so the
/// tracker's index arithmetic stays the reference's while memory stays bounded. Reading a
/// dropped position panics rather than returning wrong data.
struct Tail<T> {
    base: usize,
    v: Vec<T>,
}

impl<T: Copy> Tail<T> {
    fn new() -> Self {
        Self { base: 0, v: Vec::new() }
    }
    /// Absolute end (one past the last element), like `Vec::len` on the untrimmed buffer.
    fn len(&self) -> usize {
        self.base + self.v.len()
    }
    fn start(&self) -> usize {
        self.base
    }
    fn extend_from_slice(&mut self, x: &[T]) {
        self.v.extend_from_slice(x);
    }
    fn extend(&mut self, x: impl IntoIterator<Item = T>) {
        self.v.extend(x);
    }
    fn resize(&mut self, len: usize, value: T) {
        self.v.resize(len - self.base, value);
    }
    /// Drop everything before absolute position `start`. Only moves memory once the dead
    /// prefix is at least as long as what remains, so trimming on every push is amortized O(1).
    fn trim_to(&mut self, start: usize) {
        let k = start.saturating_sub(self.base).min(self.v.len());
        if k > 0 && 2 * k >= self.v.len() {
            self.v.drain(..k);
            self.base += k;
        }
    }
}

impl<T> Index<Range<usize>> for Tail<T> {
    type Output = [T];
    fn index(&self, r: Range<usize>) -> &[T] {
        &self.v[r.start - self.base..r.end - self.base]
    }
}

impl<T> IndexMut<Range<usize>> for Tail<T> {
    fn index_mut(&mut self, r: Range<usize>) -> &mut [T] {
        &mut self.v[r.start - self.base..r.end - self.base]
    }
}

/// CPU time spent per pipeline stage, accumulated across all push()/flush() calls.
#[derive(Debug, Default, Clone, Copy)]
pub struct Timing {
    pub mel: f64,
    pub net: f64,
    pub net_calls: u64,
    pub dbn: f64,
    pub detect: f64,
}

/// A predicted (extrapolated, zero-delay) line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PredictedLine {
    pub time: f64,
    pub is_bar: bool,
}

/// A local beat grid: `anchor_s` is a settled provisional beat's time and `period_s` the
/// seconds-per-beat around it (from the same last-5-beats median `predicted_lines` itself
/// extrapolates from), usable to snap an arbitrary timestamp to the nearest fine subdivision
/// without being limited to `lines()`'s own `MAX_AHEAD`-beat horizon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeatGrid {
    pub anchor_s: f64,
    pub period_s: f64,
}

/// What the piano roll would show at a given time: committed, provisional (after the
/// committed frontier) and predicted lines. Mirrors `OnlineTracker.lines`.
#[derive(Debug, Clone, Default)]
pub struct Lines {
    pub committed_beats: Vec<f64>,
    pub committed_downbeats: Vec<f64>,
    pub provisional_beats: Vec<f64>,
    pub provisional_downbeats: Vec<f64>,
    pub predicted: Vec<PredictedLine>,
}

pub struct BeatTracker {
    model: BeatModel,
    dbn: DbnDecoder,
    reset: bool,
    bounded: bool,
    timing: Timing,

    // input. `audio` starts with N_FFT/2 leading zeros (center=True framing).
    audio: Tail<f32>,
    // flattened (rows, N_MELS) row-major
    spect_flat: Tail<f32>,
    spect_rows: usize,

    // network outputs, one value per mel frame up to `filled`.
    beat: Tail<f32>,
    down: Tail<f32>,
    filled: usize,
    floor_in: usize,
    next_net: usize,

    // provisional lines
    prov_b: Vec<f64>,
    prov_d: Vec<f64>,
    prov_scan: usize,
    pend_d: Vec<f64>,

    // song-change detector
    next_nov: usize,
    last_reset: f64,
    resets: Vec<(f64, f64)>,

    // committed lines
    com_b: Vec<f64>,
    com_d: Vec<f64>,
    committed_until: f64,
    floor_dbn: usize,
    next_dbn: usize,
    pending: Vec<(f64, f64)>,
}

impl BeatTracker {
    /// Keeps the whole history (see the module doc); `reset` enables the song-change detector.
    pub fn new(model: BeatModel, reset: bool) -> Self {
        let mut audio = Tail::new();
        audio.resize(N_FFT / 2, 0.0f32);
        Self {
            model,
            dbn: DbnDecoder::new(FPS),
            reset,
            bounded: false,
            timing: Timing::default(),
            audio,
            spect_flat: Tail::new(),
            spect_rows: 0,
            beat: Tail::new(),
            down: Tail::new(),
            filled: 0,
            floor_in: 0,
            next_net: S,
            prov_b: Vec::new(),
            prov_d: Vec::new(),
            prov_scan: 0,
            pend_d: Vec::new(),
            next_nov: ((SC_LW + SC_CMAX) * FPS) as usize,
            last_reset: f64::NEG_INFINITY,
            resets: Vec::new(),
            com_b: Vec::new(),
            com_d: Vec::new(),
            committed_until: 0.0,
            floor_dbn: 0,
            next_dbn: DBN_STEP,
            pending: Vec::new(),
        }
    }

    /// Same lines as `new`, but only the last `RETAIN_FRAMES` of audio/spect/activations are
    /// kept, so memory stays flat over a long live session.
    pub fn bounded(model: BeatModel, reset: bool) -> Self {
        Self { bounded: true, ..Self::new(model, reset) }
    }

    // ------------------------------------------------------------------ accessors

    pub fn provisional_beats(&self) -> &[f64] {
        &self.prov_b
    }
    pub fn provisional_downbeats(&self) -> &[f64] {
        &self.prov_d
    }
    pub fn committed_beats(&self) -> &[f64] {
        &self.com_b
    }
    pub fn committed_downbeats(&self) -> &[f64] {
        &self.com_d
    }
    pub fn resets(&self) -> &[(f64, f64)] {
        &self.resets
    }
    pub fn committed_until(&self) -> f64 {
        self.committed_until
    }
    pub fn timing(&self) -> Timing {
        self.timing
    }
    /// Activations up to `filled`, from the start of the stream (`new`) or of what is
    /// retained (`bounded`).
    pub fn beat_activations(&self) -> &[f32] {
        &self.beat[self.beat.start()..self.filled.min(self.beat.len())]
    }
    pub fn downbeat_activations(&self) -> &[f32] {
        &self.down[self.down.start()..self.filled.min(self.down.len())]
    }
    pub fn filled(&self) -> usize {
        self.filled
    }
    pub fn spect_rows(&self) -> usize {
        self.spect_rows
    }
    pub fn audio_len(&self) -> usize {
        self.audio.len()
    }

    /// Default `now` for `lines()`: audio pushed so far, in seconds, minus the half-frame
    /// leading pad. Mirrors `OnlineTracker.lines`'s default.
    pub fn now(&self) -> f64 {
        (self.audio.len() as f64 - (N_FFT as f64 / 2.0)) / SAMPLE_RATE as f64
    }

    /// The current best-known beat grid as of `now` (typically `self.now()`), or `None`
    /// before enough provisional beats have settled -- same settled-beat filter and
    /// last-5-diffs median `predicted_lines` uses, just returned as a reusable anchor+period
    /// instead of a couple of lookahead points.
    pub fn beat_grid(&self, now: f64) -> Option<BeatGrid> {
        let pb_known: Vec<f64> = self.prov_b.iter().cloned().filter(|&x| x + PROV_DELAY <= now).collect();
        if pb_known.len() < 5 {
            return None;
        }
        let last5 = &pb_known[pb_known.len() - 5..];
        let diffs: Vec<f64> = last5.windows(2).map(|w| w[1] - w[0]).collect();
        Some(BeatGrid { anchor_s: *pb_known.last().unwrap(), period_s: median(diffs) })
    }

    // ------------------------------------------------------------------ input

    pub fn push(&mut self, x: &[f32]) -> Result<()> {
        self.audio.extend_from_slice(x);
        let n_have = self.spect_rows;
        if self.audio.len() < N_FFT {
            return Ok(());
        }
        let total_possible = (self.audio.len() - N_FFT) / HOP + 1;
        if total_possible <= n_have {
            return Ok(());
        }
        let n_new = total_possible - n_have;
        let t0 = Instant::now();
        let seg_start = n_have * HOP;
        let seg_end = (n_have + n_new - 1) * HOP + N_FFT;
        let m = self.model.mel(&self.audio[seg_start..seg_end])?;
        self.timing.mel += t0.elapsed().as_secs_f64();
        debug_assert_eq!(m.nrows(), n_new);
        for i in 0..n_new {
            let row = m.row(i);
            self.spect_flat.extend(row.iter().copied());
            self.spect_rows += 1;
            let t = self.spect_rows;
            if t == self.next_net {
                self.net_step(t)?;
                self.next_net += S;
            }
            if t == self.next_nov {
                self.detect(t)?;
                self.next_nov += NOV_STEP;
            }
        }
        if self.bounded {
            let keep = self.spect_rows.saturating_sub(RETAIN_FRAMES);
            self.audio.trim_to(self.spect_rows * HOP); // next mel segment starts here
            self.spect_flat.trim_to(keep * N_MELS);
            let keep = self.filled.saturating_sub(RETAIN_FRAMES);
            self.beat.trim_to(keep);
            self.down.trim_to(keep);
        }
        Ok(())
    }

    /// End of stream: fill the last F frames and commit everything (as the offline eval).
    pub fn flush(&mut self) -> Result<()> {
        let t = self.spect_rows;
        if self.filled < t {
            let a = self.floor_in.max(t.saturating_sub(W));
            self.run_net(a, t, self.filled, t)?;
        }
        self.provisional(true);
        self.dbn_steps(true);
        Ok(())
    }

    // ------------------------------------------------------------------ network

    fn run_net(&mut self, a: usize, b: usize, lo: usize, hi: usize) -> Result<()> {
        let t0 = Instant::now();
        let b_clamped = b.min(self.spect_rows);
        let a_clamped = a.min(b_clamped);
        let view = ArrayView2::from_shape(
            (b_clamped - a_clamped, N_MELS),
            &self.spect_flat[a_clamped * N_MELS..b_clamped * N_MELS],
        )
        .unwrap();
        let (pb, pd) = self.model.net(view)?;
        self.timing.net += t0.elapsed().as_secs_f64();
        self.timing.net_calls += 1;
        if self.beat.len() < hi {
            self.beat.resize(hi, 0.0);
            self.down.resize(hi, 0.0);
        }
        self.beat[lo..hi].copy_from_slice(&pb[lo - a..hi - a]);
        self.down[lo..hi].copy_from_slice(&pd[lo - a..hi - a]);
        self.filled = self.filled.max(hi);
        Ok(())
    }

    fn net_step(&mut self, t: usize) -> Result<()> {
        let a = self.floor_in.max(t.saturating_sub(W));
        let lo = t.saturating_sub(S + F).max(a).max(self.filled);
        let hi = t - F;
        if hi > lo {
            self.run_net(a, t, lo, hi)?;
            self.provisional(false);
            self.dbn_steps(false);
        }
        Ok(())
    }

    // ------------------------------------------------------------------ provisional

    fn provisional(&mut self, final_: bool) {
        let a = self.prov_scan.saturating_sub(2 * PEAK_WIN + 2);
        let end = if final_ { self.filled } else { self.filled.saturating_sub(PEAK_WIN + 1) };
        if end <= self.prov_scan {
            return;
        }
        let b = &self.beat[a..self.filled];
        let d = &self.down[a..self.filled];
        let bf = peaks_dedup(b, PEAK_WIN, a);
        let df = peaks_dedup(d, PEAK_WIN, a);
        let (lo, hi) = (self.prov_scan as f64, end as f64);
        for x in bf.into_iter().filter(|&x| lo <= x && x < hi) {
            self.prov_b.push(x / FPS);
        }
        for x in df.into_iter().filter(|&x| lo <= x && x < hi) {
            self.pend_d.push(x);
        }
        self.prov_scan = end;
        // A downbeat peak is moved to the nearest beat once no later beat can be nearer:
        // beats found from now on are >= end, so the nearest known one is final when its
        // distance is <= end - x (ties go to the earlier beat, as argmin).
        loop {
            if self.pend_d.is_empty() || self.prov_b.is_empty() {
                break;
            }
            let x = self.pend_d[0];
            let mut best_k = 0usize;
            let mut best_dist = f64::INFINITY;
            for (k, &y) in self.prov_b.iter().enumerate() {
                let dist = (y * FPS - x).abs();
                if dist < best_dist {
                    best_dist = dist;
                    best_k = k;
                }
            }
            if !final_ && best_dist > end as f64 - x {
                break;
            }
            self.pend_d.remove(0);
            let y = self.prov_b[best_k];
            if self.prov_d.is_empty() || y > *self.prov_d.last().unwrap() {
                self.prov_d.push(y);
            }
        }
        if final_ && self.prov_b.is_empty() {
            for x in self.pend_d.drain(..) {
                self.prov_d.push(x / FPS);
            }
        }
    }

    // ------------------------------------------------------------------ detector

    fn detect(&mut self, t: usize) -> Result<()> {
        if !self.reset {
            return Ok(());
        }
        if (t as f64) - self.last_reset * FPS < SC_COOLDOWN * FPS {
            return Ok(());
        }
        let t0 = Instant::now();
        let lw_cmax = ((SC_LW + SC_CMAX) * FPS) as usize; // 550
        let off = t.saturating_sub(lw_cmax);
        let n_rows = t - off;
        let x64: Vec<f64> = self.spect_flat[off * N_MELS..t * N_MELS].iter().map(|&v| v as f64).collect();
        let cmax_f = (SC_CMAX * FPS) as usize; // 150
        let cmin_f = (SC_CMIN * FPS) as usize; // 50
        let lw_f = (SC_LW * FPS) as usize; // 400
        let mut best_score = 0.0f64;
        let mut best_c: Option<usize> = None;
        let mut c = t - cmax_f;
        let c_hi = t - cmin_f;
        while c <= c_hi {
            let li = c - off;
            let l_lo = li - lw_f;
            let (lmean, lvar) = col_mean_var(&x64, l_lo, li, N_MELS);
            let (rmean, rvar) = col_mean_var(&x64, li, n_rows, N_MELS);
            let s = novelty_score(&lmean, &lvar, &rmean, &rvar);
            if s > best_score {
                best_score = s;
                best_c = Some(c);
            }
            c += 5;
        }
        self.timing.detect += t0.elapsed().as_secs_f64();
        let mut ok = false;
        if best_score > TH_NOV {
            if let Some(c) = best_c {
                let tt = t as f64 / FPS;
                let cc = c as f64 / FPS;
                if let Some(g) = self.grid_agree(tt, cc) {
                    ok = g < TH_GRID;
                }
            }
        }
        if ok {
            self.reset_at(t, best_c.unwrap())?;
        }
        Ok(())
    }

    fn grid_agree(&mut self, t: f64, c: f64) -> Option<f64> {
        let c0 = py_round(c * FPS).max(0) as usize;
        let a = c0.saturating_sub(SC_W);
        let (b, _d) = self.dbn_window(a, c0);
        let prov: Vec<f64> = self.prov_b.iter().cloned().filter(|&x| x > c + 0.3 && x < t - PROV_DELAY).collect();
        if b.len() < 5 || prov.len() < 3 {
            return None;
        }
        let last5 = &b[b.len() - 5..];
        let diffs: Vec<f64> = last5.windows(2).map(|w| w[1] - w[0]).collect();
        let big_t = median(diffs);
        let last_b = b[b.len() - 1];
        let k0 = ((c - last_b) / big_t).ceil() as i64;
        let n_terms = ((t - c) / big_t) as i64 + 3;
        let mut grid = Vec::with_capacity(n_terms.max(0) as usize);
        for k in k0..k0 + n_terms {
            grid.push(last_b + big_t * k as f64);
        }
        Some(agreement(&prov, &grid, 0.1))
    }

    fn reset_at(&mut self, t: usize, c: usize) -> Result<()> {
        self.last_reset = t as f64 / FPS;
        self.resets.push((t as f64 / FPS, c as f64 / FPS));
        self.floor_in = c;
        let hi = t - F;
        if hi > c {
            self.run_net(c, t, c, hi)?;
        }
        self.pending.push((t as f64 / FPS, c as f64 / FPS));
        Ok(())
    }

    // ------------------------------------------------------------------ committed

    fn dbn_window(&mut self, a: usize, b: usize) -> (Vec<f64>, Vec<f64>) {
        let t0 = Instant::now();
        let cap = self.beat.len().min(self.down.len());
        let b = b.min(cap);
        let a = a.min(b);
        let ba = &self.beat[a..b];
        let da = &self.down[a..b];
        let segs = segments(ba, FPS, 0.1, 1.5, 1.0);
        let mut bs = Vec::new();
        let mut ds = Vec::new();
        for (p, q) in segs {
            let (pb, pd) = self.dbn.decode(&ba[p..q], &da[p..q]);
            let off = (a + p) as f64 / FPS;
            bs.extend(pb.iter().map(|&x| x + off));
            ds.extend(pd.iter().map(|&x| x + off));
        }
        self.timing.dbn += t0.elapsed().as_secs_f64();
        (bs, ds)
    }

    fn dbn_steps(&mut self, final_: bool) {
        loop {
            let cond = self.next_dbn <= self.filled || (final_ && self.next_dbn - DBN_STEP < self.filled);
            if !cond {
                break;
            }
            let end = self.next_dbn.min(self.filled);
            while !self.pending.is_empty() && self.pending[0].0 <= end as f64 / FPS {
                let (_, cut) = self.pending.remove(0);
                let c = py_round(cut * FPS).max(0) as usize;
                if c > self.floor_dbn {
                    if cut > self.committed_until {
                        let a = self.floor_dbn.max(c.saturating_sub(DBN_W));
                        let (b, d) = self.dbn_window(a, c);
                        commit(&mut self.com_b, &mut self.com_d, &b, &d, self.committed_until, cut);
                        self.committed_until = cut;
                    }
                    self.floor_dbn = c;
                }
            }
            let a = self.floor_dbn.max(end.saturating_sub(DBN_W));
            let (b, d) = self.dbn_window(a, end);
            let last = final_ && end >= self.filled;
            let limit = if last { end as f64 / FPS + 1.0 } else { end as f64 / FPS - LAG };
            commit(&mut self.com_b, &mut self.com_d, &b, &d, self.committed_until, limit);
            self.committed_until = self.committed_until.max(limit);
            self.next_dbn += DBN_STEP;
            if last {
                break;
            }
        }
    }

    // ------------------------------------------------------------------ display

    /// What the piano roll shows at `now`: committed, provisional (after the committed
    /// frontier) and predicted lines. Mirrors `OnlineTracker.lines`.
    pub fn lines(&self, now: f64) -> Lines {
        self.lines_since(now, f64::NEG_INFINITY)
    }

    /// `lines(now)` restricted to lines at or after `from` (what can still be on screen), so
    /// a long session doesn't copy its whole history on every update. All four line lists
    /// are sorted (they are only ever appended to past their last element).
    pub fn lines_since(&self, now: f64, from: f64) -> Lines {
        let (pred, _last) = predicted_lines(&self.prov_b, &self.prov_d, now + PROV_DELAY + 1.0);
        let tail = |v: &[f64], lo: f64| v[v.partition_point(|&x| x < lo)..].to_vec();
        let prov_from = from.max(self.committed_until);
        Lines {
            committed_beats: tail(&self.com_b, from),
            committed_downbeats: tail(&self.com_d, from),
            provisional_beats: tail(&self.prov_b, prov_from),
            provisional_downbeats: tail(&self.prov_d, prov_from),
            predicted: pred.into_iter().filter(|p| p.time <= now && p.time >= from).collect(),
        }
    }
}

// =============================================================================================
// free functions ported from beattrack.decode / stream_eval / song_change / stream_predict
// =============================================================================================

/// Python's `round()`: round-half-to-even ("banker's rounding").
fn py_round(x: f64) -> i64 {
    let floor = x.floor();
    let diff = x - floor;
    let fl = floor as i64;
    if diff < 0.5 {
        fl
    } else if diff > 0.5 {
        fl + 1
    } else if fl % 2 == 0 {
        fl
    } else {
        fl + 1
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n == 0 {
        return f64::NAN;
    }
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// beattrack.decode._deduplicate_peaks: merge peak indices within `width` of each other,
/// averaging their positions (running mean).
fn dedup_peaks(peaks: &[i64], width: i64) -> Vec<f64> {
    let mut result = Vec::new();
    let mut it = peaks.iter();
    let Some(&p0) = it.next() else { return result };
    let mut p = p0 as f64;
    let mut c: i64 = 1;
    for &p2i in it {
        let p2 = p2i as f64;
        if p2 - p <= width as f64 {
            c += 1;
            p += (p2 - p) / c as f64;
        } else {
            result.push(p);
            p = p2;
            c = 1;
        }
    }
    result.push(p);
    result
}

/// `peaks()` from `OnlineTracker._provisional`: local-max frames > 0.5 within +/- `win`,
/// deduplicated (width=1) and shifted by `offset` to absolute frame positions.
fn peaks_dedup(act: &[f32], win: usize, offset: usize) -> Vec<f64> {
    let n = act.len();
    let mut idx = Vec::new();
    for i in 0..n {
        let lo = i.saturating_sub(win);
        let hi = (i + win + 1).min(n);
        let m = act[lo..hi].iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        if act[i] > 0.5 && act[i] == m {
            idx.push(i as i64);
        }
    }
    dedup_peaks(&idx, 1).into_iter().map(|x| x + offset as f64).collect()
}

/// stream_eval._segments: split [0, n) at runs of >= min_gap s with beat_act < low; drop
/// parts < min_len s.
fn segments(beat_act: &[f32], fps: f64, low: f32, min_gap: f64, min_len: f64) -> Vec<(usize, usize)> {
    let n = beat_act.len();
    let quiet: Vec<bool> = beat_act.iter().map(|&v| v < low).collect();
    let g = (min_gap * fps) as usize;
    let mut segs = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < n {
        if quiet[i] {
            let mut j = i;
            while j < n && quiet[j] {
                j += 1;
            }
            if j - i >= g {
                if i > start {
                    segs.push((start, i));
                }
                start = j;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    if n > start {
        segs.push((start, n));
    }
    segs.into_iter().filter(|&(a, b)| (b - a) as f64 >= min_len * fps).collect()
}

/// stream_eval._commit: append newly decoded beats in [lo, hi) that are more than 0.1s
/// after the last committed one; a beat is a downbeat if it exactly matches an entry of `d`.
fn commit(beats: &mut Vec<f64>, downs: &mut Vec<f64>, b: &[f64], d: &[f64], lo: f64, hi: f64) {
    for &x in b {
        if x >= lo && x < hi {
            if beats.is_empty() || x - beats[beats.len() - 1] > 0.1 {
                beats.push(x);
                if d.iter().any(|&dd| (dd - x).abs() < 1e-6) {
                    downs.push(x);
                }
            }
        }
    }
}

/// stream_eval._matched, as a fraction of `refs` matched within `tol` of some `est` (0 if
/// `est` is empty).
fn matched_fraction(refs: &[f64], est: &[f64], tol: f64) -> f64 {
    if refs.is_empty() {
        return 0.0;
    }
    if est.is_empty() {
        return 0.0;
    }
    let matched = refs.iter().filter(|&&r| est.iter().any(|&e| (e - r).abs() <= tol)).count();
    matched as f64 / refs.len() as f64
}

/// song_change._agreement.
fn agreement(p: &[f64], b: &[f64], tol: f64) -> f64 {
    if p.is_empty() || b.is_empty() {
        return 0.0;
    }
    matched_fraction(p, b, tol).max(matched_fraction(b, p, tol))
}

fn col_mean_var(x: &[f64], r0: usize, r1: usize, cols: usize) -> (Vec<f64>, Vec<f64>) {
    let n = (r1 - r0) as f64;
    let mut mean = vec![0.0f64; cols];
    for r in r0..r1 {
        let row = &x[r * cols..(r + 1) * cols];
        for c in 0..cols {
            mean[c] += row[c];
        }
    }
    for m in mean.iter_mut() {
        *m /= n;
    }
    let mut var = vec![0.0f64; cols];
    for r in r0..r1 {
        let row = &x[r * cols..(r + 1) * cols];
        for c in 0..cols {
            let dd = row[c] - mean[c];
            var[c] += dd * dd;
        }
    }
    for v in var.iter_mut() {
        *v /= n;
    }
    (mean, var)
}

fn novelty_score(lmean: &[f64], lvar: &[f64], rmean: &[f64], rvar: &[f64]) -> f64 {
    let mut s = 0.0;
    for j in 0..lmean.len() {
        let dd = rmean[j] - lmean[j];
        s += dd * dd / (lvar[j] + rvar[j] + 1e-3);
    }
    s / lmean.len() as f64
}

/// stream_predict.predicted_lines, specialised to mode="prov" (the only mode online.py
/// ever calls it with -- `cb`/COMMIT_DELAY are unused in that mode, matching the Python).
fn predicted_lines(pb: &[f64], pd: &[f64], t: f64) -> (Vec<PredictedLine>, Option<f64>) {
    let pb_known: Vec<f64> = pb.iter().cloned().filter(|&x| x + PROV_DELAY <= t).collect();
    if pb_known.is_empty() {
        return (Vec::new(), None);
    }
    if pb_known.len() < 5 {
        return (Vec::new(), None);
    }
    let last5 = &pb_known[pb_known.len() - 5..];
    let diffs: Vec<f64> = last5.windows(2).map(|w| w[1] - w[0]).collect();
    let big_t = median(diffs);
    let pd_known: Vec<f64> = pd.iter().cloned().filter(|&x| x + PROV_DELAY <= t).collect();
    let bar_len = bar_len(&pb_known, &pd_known);
    let last = *pb_known.last().unwrap();
    let since: Option<i64> = if !pd_known.is_empty() {
        let last_pd = *pd_known.last().unwrap();
        Some(pb_known.iter().filter(|&&x| x > last_pd + 1e-4).count() as i64)
    } else {
        None
    };
    let mut out = Vec::new();
    for k in 1..=MAX_AHEAD {
        let x = last + k as f64 * big_t;
        if x > t {
            break;
        }
        let is_bar = since.map(|s| (s + k) % bar_len == 0).unwrap_or(false);
        out.push(PredictedLine { time: x, is_bar });
    }
    (out, Some(last))
}

fn lower_bound(a: &[f64], v: f64) -> usize {
    a.partition_point(|&x| x < v)
}

fn bar_len(pb_known: &[f64], pd_known: &[f64]) -> i64 {
    if pd_known.len() < 2 {
        return 4;
    }
    let last5 = if pd_known.len() >= 5 { &pd_known[pd_known.len() - 5..] } else { pd_known };
    let idx: Vec<i64> = last5.iter().map(|&v| lower_bound(pb_known, v - 1e-4) as i64).collect();
    let diffs: Vec<i64> = idx.windows(2).map(|w| w[1] - w[0]).collect();
    let lens: Vec<i64> = diffs.into_iter().filter(|&x| x == 2 || x == 3 || x == 4 || x == 6).collect();
    if lens.is_empty() {
        return 4;
    }
    most_common(&lens)
}

/// Counter(v).most_common(1): highest count, ties broken by first-seen order.
fn most_common(v: &[i64]) -> i64 {
    let mut counts: Vec<(i64, i64)> = Vec::new();
    for &x in v {
        if let Some(e) = counts.iter_mut().find(|(val, _)| *val == x) {
            e.1 += 1;
        } else {
            counts.push((x, 1));
        }
    }
    let mut best = counts[0];
    for &c in &counts[1..] {
        if c.1 > best.1 {
            best = c;
        }
    }
    best.0
}

// =============================================================================================
// tests
// =============================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beat_model::golden_dir;
    use ndarray::{Array1, Array2, Ix1, Ix2};
    use ndarray_npy::NpzReader;

    #[test]
    fn matches_python_golden_stream() {
        let Some(dir) = golden_dir() else { return };
        let mut npz = NpzReader::new(std::fs::File::open(dir.join("stream_s00_90s.npz")).unwrap()).unwrap();
        let audio: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>("audio").unwrap();
        let spect_ref: Array2<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix2>("spect").unwrap();
        let beat_ref: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>("beat").unwrap();
        let down_ref: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>("down").unwrap();
        let prov_b_ref: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>("prov_b").unwrap();
        let prov_d_ref: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>("prov_d").unwrap();
        let com_b_ref: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>("com_b").unwrap();
        let com_d_ref: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>("com_d").unwrap();
        let resets_ref: Array2<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix2>("resets").unwrap();
        let chunk = 1000usize; // golden() always pushes in 1000-sample chunks (see online.py)

        let model = BeatModel::load(1).unwrap();
        let mut tr = BeatTracker::new(model, true);

        let wall0 = Instant::now();
        let mut i = 0;
        while i < audio.len() {
            let end = (i + chunk).min(audio.len());
            tr.push(&audio.as_slice().unwrap()[i..end]).unwrap();
            i = end;
        }
        tr.flush().unwrap();
        let wall = wall0.elapsed().as_secs_f64();

        // spect
        assert_eq!(tr.spect_rows(), spect_ref.nrows());

        // activations, up to `filled` in the golden (down/beat arrays were saved trimmed
        // to tr.filled in Python, i.e. len(beat_ref) == python's filled).
        let filled = beat_ref.len();
        assert_eq!(tr.filled(), filled, "filled frame count mismatch");
        let ours_beat = tr.beat_activations();
        let ours_down = tr.downbeat_activations();
        let dmax_beat = ours_beat
            .iter()
            .zip(beat_ref.iter())
            .fold(0.0f32, |acc, (&a, &b)| acc.max((a - b).abs()));
        let dmax_down = ours_down
            .iter()
            .zip(down_ref.iter())
            .fold(0.0f32, |acc, (&a, &b)| acc.max((a - b).abs()));
        println!("max |beat_act diff| = {dmax_beat:.6}, max |down_act diff| = {dmax_down:.6}");
        assert!(dmax_beat < 1e-2, "beat activation diff too large: {dmax_beat}");
        assert!(dmax_down < 1e-2, "down activation diff too large: {dmax_down}");

        // resets: equal
        let resets = tr.resets();
        assert_eq!(resets.len(), resets_ref.nrows(), "reset count mismatch");
        for (i, &(t, c)) in resets.iter().enumerate() {
            assert!((t - resets_ref[[i, 0]]).abs() < 1e-9, "reset {i} detect time mismatch");
            assert!((c - resets_ref[[i, 1]]).abs() < 1e-9, "reset {i} cut time mismatch");
        }

        // provisional / committed lines: same count, times within 1e-6
        assert_eq!(tr.provisional_beats().len(), prov_b_ref.len(), "prov_b count mismatch");
        for (i, (&a, &b)) in tr.provisional_beats().iter().zip(prov_b_ref.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "prov_b[{i}] {a} vs {b}");
        }
        assert_eq!(tr.provisional_downbeats().len(), prov_d_ref.len(), "prov_d count mismatch");
        for (i, (&a, &b)) in tr.provisional_downbeats().iter().zip(prov_d_ref.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "prov_d[{i}] {a} vs {b}");
        }
        assert_eq!(tr.committed_beats().len(), com_b_ref.len(), "com_b count mismatch");
        for (i, (&a, &b)) in tr.committed_beats().iter().zip(com_b_ref.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "com_b[{i}] {a} vs {b}");
        }
        assert_eq!(tr.committed_downbeats().len(), com_d_ref.len(), "com_d count mismatch");
        for (i, (&a, &b)) in tr.committed_downbeats().iter().zip(com_d_ref.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "com_d[{i}] {a} vs {b}");
        }

        let secs = audio.len() as f64 / SAMPLE_RATE as f64;
        let t = tr.timing();
        println!(
            "CPU ms/s of audio: mel {:.2}  net {:.2} ({} calls)  dbn {:.2}  detect {:.2}  (wall {:.2}, total {:.1}s audio)",
            t.mel / secs * 1e3,
            t.net / secs * 1e3,
            t.net_calls,
            t.dbn / secs * 1e3,
            t.detect / secs * 1e3,
            wall / secs * 1e3,
            secs,
        );
    }

    #[test]
    fn bounded_matches_python_golden_lines() {
        // Same stream as above through `bounded`: the 90s golden is 3x RETAIN_FRAMES, so
        // the buffers are trimmed many times (including across both resets) and every line
        // must still come out as in the reference.
        let Some(dir) = golden_dir() else { return };
        let mut npz = NpzReader::new(std::fs::File::open(dir.join("stream_s00_90s.npz")).unwrap()).unwrap();
        let audio: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>("audio").unwrap();
        let mut tr = BeatTracker::bounded(BeatModel::load(1).unwrap(), true);
        for chunk in audio.as_slice().unwrap().chunks(1000) {
            tr.push(chunk).unwrap();
            assert!(tr.beat.v.len() <= 2 * RETAIN_FRAMES + 2 * S, "activations not trimmed");
            assert!(tr.spect_flat.v.len() <= (2 * RETAIN_FRAMES + 2 * S) * N_MELS, "spect not trimmed");
            assert!(tr.audio.v.len() <= 2 * 2000 + N_FFT + 2 * HOP, "audio not trimmed");
        }
        tr.flush().unwrap();
        assert!(tr.beat.start() > 0 && tr.spect_flat.start() > 0);
        let resets_ref: Array2<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix2>("resets").unwrap();
        assert_eq!(tr.resets().len(), resets_ref.nrows(), "reset count mismatch");
        for (name, ours) in [
            ("prov_b", tr.provisional_beats()),
            ("prov_d", tr.provisional_downbeats()),
            ("com_b", tr.committed_beats()),
            ("com_d", tr.committed_downbeats()),
        ] {
            let r: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>(name).unwrap();
            assert_eq!(ours.len(), r.len(), "{name} count mismatch");
            for (i, (&a, &b)) in ours.iter().zip(r.iter()).enumerate() {
                assert!((a - b).abs() < 1e-6, "{name}[{i}] {a} vs {b}");
            }
        }
    }

    #[test]
    fn decode_only_matches_dbn_windows() {
        // Exercises the DBN decoding path (dbn_window's segments + DbnDecoder) directly on
        // the golden `beat`/`down` arrays, independent of ORT numerics. This test only
        // needs DbnDecoder to be implemented; it does not load an ONNX model.
        let Some(dir) = golden_dir() else { return };
        let path = dir.join("dbn_windows.npz");
        if !path.exists() {
            eprintln!("skipping: no dbn_windows.npz at {}", path.display());
            return;
        }
        let mut npz = NpzReader::new(std::fs::File::open(path).unwrap()).unwrap();
        let dbn = DbnDecoder::new(50.0);
        for i in 0..8 {
            let beat: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>(&format!("beat_{i}")).unwrap();
            let down: Array1<f32> = npz.by_name::<ndarray::OwnedRepr<f32>, Ix1>(&format!("down_{i}")).unwrap();
            let beats_ref: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>(&format!("beats_{i}")).unwrap();
            let downs_ref: Array1<f64> = npz.by_name::<ndarray::OwnedRepr<f64>, Ix1>(&format!("downs_{i}")).unwrap();
            let (b, d) = dbn.decode(beat.as_slice().unwrap(), down.as_slice().unwrap());
            assert_eq!(b.len(), beats_ref.len(), "window {i} beat count");
            for (k, (&a, &r)) in b.iter().zip(beats_ref.iter()).enumerate() {
                assert!((a - r).abs() < 1e-6, "window {i} beat {k}: {a} vs {r}");
            }
            assert_eq!(d.len(), downs_ref.len(), "window {i} downbeat count");
            for (k, (&a, &r)) in d.iter().zip(downs_ref.iter()).enumerate() {
                assert!((a - r).abs() < 1e-6, "window {i} downbeat {k}: {a} vs {r}");
            }
        }
    }
}
