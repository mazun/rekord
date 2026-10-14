//! Streaming drum onsets: `DrumTranscriber` feeds 44.1 kHz audio to `DrumModel` and turns its
//! activations into hits with `PeakPicker`.
//!
//! The peak picking is ADTOF's: madmom's `NotePeakPickingProcessor` with `pre_avg` 0.1 s,
//! `post_avg` 0.01 s, `pre_max` 0.02 s, `post_max` 0.01 s and `combine` 0.02 s at 100 frames
//! per second -- subtract a moving average of the activation, keep the local maxima that reach
//! the class's threshold, drop a hit within `combine` of the previous one. Only the thresholds
//! differ from the authors' (see `THRESHOLDS`).
//!
//! The network is a bidirectional RNN, and its activations depend on the audio around a frame
//! far beyond the 0.1 s the peak picking looks at: they keep moving toward the whole-file ones
//! as more audio follows (mean difference at the hit frames of a rock song: 0.16 with 0.5 s
//! after, 0.10 with 1 s, 0.03 with 3 s) and as more precedes. So a stream never reproduces the
//! whole-file hits exactly, and a hit whose activation sits near its threshold may come or go
//! (hi-hats of a synth-drum song: 305 whole-file, 195 streamed with 10 s before and 1 s
//! after). `LOOKAHEAD_S`, `CONTEXT_S` and `MIN_BATCH_FRAMES` trade that against latency and
//! CPU. A hit comes out about `LOOKAHEAD_S` + 0.02 s + one batch after the sound.

use std::collections::VecDeque;

use anyhow::Result;

use crate::drum_model::{DrumModel, FRAMES_PER_SECOND, HOP, N_CLASSES, SAMPLE_RATE};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Drum {
    Kick,
    Snare,
    Tom,
    HiHat,
    Cymbal,
}

impl Drum {
    pub const ALL: [Drum; N_CLASSES] = [
        Drum::Kick,
        Drum::Snare,
        Drum::Tom,
        Drum::HiHat,
        Drum::Cymbal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Drum::Kick => "kick",
            Drum::Snare => "snare",
            Drum::Tom => "tom",
            Drum::HiHat => "hihat",
            Drum::Cymbal => "cymbal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct DrumHit {
    /// Audio time of the hit, like every other timestamp of the pipeline.
    pub t: f32,
    pub drum: Drum,
    /// The class's activation at the hit's frame (0..1).
    pub strength: f32,
}

/// Per-class peak-picking thresholds in `Drum::ALL` order. The authors' values are 0.22, 0.24,
/// 0.32, 0.22, 0.30; tom is raised to 0.38 because on real mixes it fired on things that are
/// not toms (an anime song gave 61 per minute) -- picked by ear on five songs, not swept.
pub const THRESHOLDS: [f32; N_CLASSES] = [0.22, 0.24, 0.38, 0.22, 0.30];

const PRE_AVG: usize = 10;
const POST_AVG: usize = 1;
const PRE_MAX: usize = 2;
const POST_MAX: usize = 1;
/// A hit closer than this many frames after the previous one of its class is dropped
/// (madmom keeps one only if the gap is *more* than `combine`).
const COMBINE_FRAMES: usize = 2;
/// Frames after a frame that its decision reads.
const LOOKAHEAD_FRAMES: usize = POST_AVG + POST_MAX;
/// Frames before a frame that its decision reads.
const LOOKBEHIND_FRAMES: usize = PRE_AVG + PRE_MAX;

/// Madmom's peak picking, incrementally: `push` activation frames, get the hits that became
/// decidable. Gives exactly the hits one pass over all the frames would (`finish` stands in for
/// the zeros madmom pads the end with).
pub struct PeakPicker {
    frames: VecDeque<[f32; N_CLASSES]>,
    /// Frame index of `frames[0]`.
    first: usize,
    /// Next frame to decide.
    next: usize,
    last_hit: [Option<usize>; N_CLASSES],
}

impl PeakPicker {
    pub fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            first: 0,
            next: 0,
            last_hit: [None; N_CLASSES],
        }
    }

    fn end(&self) -> usize {
        self.first + self.frames.len()
    }

    /// Zero outside what has been pushed, like madmom's constant padding.
    fn act(&self, frame: isize, class: usize) -> f32 {
        if frame < self.first as isize {
            return 0.0;
        }
        self.frames
            .get(frame as usize - self.first)
            .map_or(0.0, |f| f[class])
    }

    /// Activation minus the mean over `[frame - PRE_AVG, frame + POST_AVG]`.
    fn detection(&self, frame: isize, class: usize) -> f32 {
        let lo = frame - PRE_AVG as isize;
        let hi = frame + POST_AVG as isize;
        let sum: f32 = (lo..=hi).map(|g| self.act(g, class)).sum();
        self.act(frame, class) - sum / (PRE_AVG + POST_AVG + 1) as f32
    }

    fn decide(&mut self, frame: usize, hits: &mut Vec<DrumHit>) {
        let f = frame as isize;
        for (class, drum) in Drum::ALL.into_iter().enumerate() {
            let d = self.detection(f, class);
            if d < THRESHOLDS[class] {
                continue;
            }
            let is_max = (f - PRE_MAX as isize..=f + POST_MAX as isize)
                .all(|g| self.detection(g, class) <= d);
            if !is_max {
                continue;
            }
            if self.last_hit[class].is_some_and(|l| frame - l <= COMBINE_FRAMES) {
                continue;
            }
            self.last_hit[class] = Some(frame);
            hits.push(DrumHit {
                t: frame as f32 / FRAMES_PER_SECOND,
                drum,
                strength: self.act(f, class),
            });
        }
    }

    /// Frames before this index are decided (their hits returned, none to come).
    pub fn decided_frames(&self) -> usize {
        self.next
    }

    fn drop_old(&mut self) {
        let keep_from = self.next.saturating_sub(LOOKBEHIND_FRAMES);
        while self.first < keep_from {
            self.frames.pop_front();
            self.first += 1;
        }
    }

    /// Hits are returned in frame order.
    pub fn push(&mut self, new: &[[f32; N_CLASSES]]) -> Vec<DrumHit> {
        self.frames.extend(new.iter().copied());
        let mut hits = Vec::new();
        while self.next + LOOKAHEAD_FRAMES < self.end() {
            self.decide(self.next, &mut hits);
            self.next += 1;
        }
        self.drop_old();
        hits
    }

    /// The end of the stream: decides the frames still waiting for their look-ahead.
    pub fn finish(&mut self) -> Vec<DrumHit> {
        let real_end = self.end();
        self.frames.extend([[0.0; N_CLASSES]; LOOKAHEAD_FRAMES]);
        let mut hits = Vec::new();
        while self.next < real_end {
            self.decide(self.next, &mut hits);
            self.next += 1;
        }
        hits
    }
}

impl Default for PeakPicker {
    fn default() -> Self {
        Self::new()
    }
}

/// How soon, and so how reliably, a hit is decided. Serialized as the lowercase name, like
/// `ChordLayerId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DrumLayerId {
    /// 0.3 s of audio after a frame: a first guess that misses quiet hits (hi-hats most).
    Instant,
    /// 1 s after.
    Settled,
    /// 3 s after: the closest to what the whole-file run gives.
    Confirmed,
}

impl DrumLayerId {
    pub const ALL: [DrumLayerId; 3] = [
        DrumLayerId::Instant,
        DrumLayerId::Settled,
        DrumLayerId::Confirmed,
    ];

    /// Audio that must follow a frame before this layer takes the model's activations for it.
    pub fn lookahead_s(self) -> f32 {
        match self {
            DrumLayerId::Instant => 0.3,
            DrumLayerId::Settled => 1.0,
            DrumLayerId::Confirmed => 3.0,
        }
    }
}

/// What one layer newly decided: the hits (in time order) and `decided_s`, the time up to which
/// the layer has decided everything -- no hit before it will follow, and none already sent changes.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerHits {
    pub layer: DrumLayerId,
    pub hits: Vec<DrumHit>,
    pub decided_s: f32,
}

/// Audio kept before the oldest frame still to be decided, as context for the model.
const CONTEXT_S: f32 = 4.0;
/// The model runs once this many frames have become final for the instant layer (0.5 s). On
/// one thread a run costs ~1% of real time per second of audio in the window (~7.5 s here).
const MIN_BATCH_FRAMES: usize = 50;

struct Layer {
    id: DrumLayerId,
    picker: PeakPicker,
    /// First frame whose activations have not been given to the picker.
    next_frame: usize,
}

/// Streaming drum transcription of mono 44.1 kHz audio, in three layers that decide the same
/// frames at increasing delay (see `DrumLayerId`). One model run per batch serves all of them:
/// each layer takes the activations of the frames that have just got its look-ahead. Hits are on
/// the timeline of the first sample pushed.
pub struct DrumTranscriber {
    model: DrumModel,
    layers: Vec<Layer>,
    /// Audio from frame `buf_start_frame` on (frame `i` starts at sample `441 i`).
    buf: Vec<f32>,
    buf_start_frame: usize,
    total_samples: usize,
}

impl DrumTranscriber {
    pub fn new(model: DrumModel) -> Self {
        Self {
            model,
            layers: DrumLayerId::ALL
                .into_iter()
                .map(|id| Layer {
                    id,
                    picker: PeakPicker::new(),
                    next_frame: 0,
                })
                .collect(),
            buf: Vec::new(),
            buf_start_frame: 0,
            total_samples: 0,
        }
    }

    /// Frames before this one have their look-ahead for `layer`.
    fn final_end(&self, layer: DrumLayerId) -> usize {
        let lookahead = (layer.lookahead_s() * SAMPLE_RATE as f32) as usize;
        self.total_samples.saturating_sub(lookahead) / HOP
    }

    pub fn push(&mut self, audio: &[f32]) -> Result<Vec<LayerHits>> {
        self.buf.extend_from_slice(audio);
        self.total_samples += audio.len();
        let first = &self.layers[0];
        if self.final_end(first.id) < first.next_frame + MIN_BATCH_FRAMES {
            return Ok(Vec::new());
        }
        let ends = self.layers.iter().map(|l| self.final_end(l.id)).collect();
        self.run(ends, false)
    }

    /// Decides everything still waiting for its look-ahead, in every layer. Call once, at the
    /// end of the audio.
    pub fn finish(&mut self) -> Result<Vec<LayerHits>> {
        let end = self.total_samples.div_ceil(HOP);
        self.run(vec![end; self.layers.len()], true)
    }

    /// One model run over the buffer; layer `i` takes its frames up to `ends[i]`.
    fn run(&mut self, ends: Vec<usize>, finishing: bool) -> Result<Vec<LayerHits>> {
        let run_model = self
            .layers
            .iter()
            .zip(&ends)
            .any(|(l, &e)| e > l.next_frame);
        let acts = if run_model {
            self.model.activations(&self.buf)?
        } else {
            Vec::new()
        };
        let mut out = Vec::new();
        for (layer, &end) in self.layers.iter_mut().zip(&ends) {
            let from = layer.next_frame - self.buf_start_frame;
            let to = (end - self.buf_start_frame).min(acts.len());
            let decided_before = layer.picker.decided_frames();
            let mut hits = layer.picker.push(&acts[from.min(to)..to]);
            layer.next_frame = layer.next_frame.max(end);
            if finishing {
                hits.extend(layer.picker.finish());
            }
            let decided = layer.picker.decided_frames();
            if decided != decided_before {
                out.push(LayerHits {
                    layer: layer.id,
                    hits,
                    decided_s: decided as f32 / FRAMES_PER_SECOND,
                });
            }
        }

        let oldest = self.layers.iter().map(|l| l.next_frame).min().unwrap_or(0);
        let keep_from = oldest.saturating_sub((CONTEXT_S * FRAMES_PER_SECOND) as usize);
        if keep_from > self.buf_start_frame {
            let drop = ((keep_from - self.buf_start_frame) * HOP).min(self.buf.len());
            self.buf.drain(..drop);
            self.buf_start_frame = keep_from;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// madmom's algorithm written out on whole arrays, to compare `PeakPicker` against.
    fn reference(acts: &[[f32; N_CLASSES]]) -> Vec<(usize, usize)> {
        let n = acts.len();
        let at = |g: isize, c: usize| {
            if g < 0 || g as usize >= n {
                0.0
            } else {
                acts[g as usize][c]
            }
        };
        let mut out = Vec::new();
        for (c, &threshold) in THRESHOLDS.iter().enumerate() {
            let d = |g: isize| {
                let avg: f32 = (g - 10..=g + 1).map(|h| at(h, c)).sum::<f32>() / 12.0;
                at(g, c) - avg
            };
            let mut last: Option<usize> = None;
            for f in 0..n {
                let fi = f as isize;
                let max = (fi - 2..=fi + 1).map(d).fold(f32::MIN, f32::max);
                if d(fi) >= threshold && d(fi) == max && last.is_none_or(|l| f - l > 2) {
                    out.push((f, c));
                    last = Some(f);
                }
            }
        }
        out.sort();
        out
    }

    fn pseudo_random_acts(n: usize) -> Vec<[f32; N_CLASSES]> {
        let mut x = 12345u32;
        let mut next = || {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 8) as f32 / (1u32 << 24) as f32
        };
        (0..n)
            .map(|_| std::array::from_fn(|_| if next() < 0.12 { next() } else { next() * 0.1 }))
            .collect()
    }

    fn as_pairs(hits: &[DrumHit]) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = hits
            .iter()
            .map(|h| {
                (
                    (h.t * FRAMES_PER_SECOND).round() as usize,
                    Drum::ALL.iter().position(|&d| d == h.drum).unwrap(),
                )
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn isolated_peak_is_a_hit_and_a_small_one_is_not() {
        let mut acts = vec![[0.0; N_CLASSES]; 60];
        acts[30][1] = 0.9; // snare
        acts[40][0] = 0.15; // kick below its threshold
        let mut p = PeakPicker::new();
        let mut hits = p.push(&acts);
        hits.extend(p.finish());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].drum, Drum::Snare);
        assert!((hits[0].t - 0.30).abs() < 1e-6);
        assert!((hits[0].strength - 0.9).abs() < 1e-6);
    }

    #[test]
    fn plateau_after_a_hit_is_combined_and_a_sustained_level_is_not_a_hit() {
        let mut acts = vec![[0.0; N_CLASSES]; 120];
        acts[30][3] = 0.8;
        acts[32][3] = 0.8; // 2 frames later: dropped by combine
        acts[36][3] = 0.8; // 6 frames later: kept
        for a in acts.iter_mut().skip(60) {
            a[4] = 0.5; // a steady level: the moving average cancels it
        }
        let mut p = PeakPicker::new();
        let mut hits = p.push(&acts);
        hits.extend(p.finish());
        let got: Vec<_> = as_pairs(&hits)
            .into_iter()
            .filter(|&(_, c)| c == 3)
            .collect();
        assert_eq!(got, vec![(30, 3), (36, 3)]);
        assert!(hits.iter().all(|h| h.drum != Drum::Cymbal || h.t < 0.7));
    }

    #[test]
    fn matches_the_reference_for_any_chunking() {
        let acts = pseudo_random_acts(3000);
        let want = reference(&acts);
        assert!(want.len() > 50, "test data should contain hits");
        for chunk in [1, 2, 7, 12, 500, 3000] {
            let mut p = PeakPicker::new();
            let mut hits = Vec::new();
            for c in acts.chunks(chunk) {
                hits.extend(p.push(c));
            }
            hits.extend(p.finish());
            assert_eq!(as_pairs(&hits), want, "chunk size {chunk}");
        }
    }

    /// Streamed hits against the model run over the whole file, on a WAV given by
    /// DRUM_TEST_AUDIO (any music with drums; none is kept in the repo). Skips without it or
    /// without the model.
    #[test]
    fn streaming_matches_the_whole_file() {
        let Ok(path) = std::env::var("DRUM_TEST_AUDIO") else {
            eprintln!("skipping: DRUM_TEST_AUDIO not set");
            return;
        };
        let Ok(mut model) = DrumModel::load(1) else {
            eprintln!("skipping: no drum model");
            return;
        };
        let audio = crate::audio_io::load_audio(&path, SAMPLE_RATE).unwrap();
        let audio = &audio[..audio.len().min(SAMPLE_RATE * 90)];

        let mut p = PeakPicker::new();
        let mut want = p.push(&model.activations(audio).unwrap());
        want.extend(p.finish());

        let mut stream = DrumTranscriber::new(DrumModel::load(1).unwrap());
        let mut got: [Vec<DrumHit>; 3] = Default::default();
        let mut decided = [0.0f32; 3];
        let mut take = |layers: Vec<LayerHits>| {
            for l in layers {
                let i = DrumLayerId::ALL.iter().position(|&x| x == l.layer).unwrap();
                assert!(l.decided_s > decided[i], "decided_s must advance");
                assert!(l.hits.iter().all(|h| h.t < l.decided_s));
                assert!(l.hits.iter().all(|h| h.t >= decided[i] - 0.03));
                decided[i] = l.decided_s;
                got[i].extend(l.hits);
            }
        };
        for chunk in audio.chunks((0.064 * SAMPLE_RATE as f32) as usize) {
            take(stream.push(chunk).unwrap());
        }
        take(stream.finish().unwrap());
        let frames = audio.len().div_ceil(441) as f32 / FRAMES_PER_SECOND;
        assert!(decided.iter().all(|&d| (d - frames).abs() < 0.02));

        let want = as_pairs(&want);
        let per_class = |hits: &[(usize, usize)]| {
            Drum::ALL.map(|d| {
                let c = Drum::ALL.iter().position(|&x| x == d).unwrap();
                hits.iter().filter(|h| h.1 == c).count()
            })
        };
        eprintln!(
            "whole file: {} hits, per class (kick snare tom hihat cymbal) {:?}",
            want.len(),
            per_class(&want)
        );
        let mut agreement = [0.0f32; 3];
        for (i, layer) in DrumLayerId::ALL.into_iter().enumerate() {
            let pairs = as_pairs(&got[i]);
            let matched = want
                .iter()
                .filter(|&&(f, c)| pairs.iter().any(|&(g, d)| d == c && g.abs_diff(f) <= 1))
                .count();
            agreement[i] = matched as f32 / want.len() as f32;
            eprintln!(
                "{layer:?}: {} hits, {matched} matched, per class {:?}",
                pairs.len(),
                per_class(&pairs)
            );
        }
        // Hits near a threshold come and go (see the module doc), so this only catches gross
        // errors such as shifted or missing batches, and that later layers are closer to the
        // whole file. The confirmed layer agrees 80-96% on the five test songs.
        assert!(want.len() > 20);
        assert!(agreement[2] >= 0.75, "confirmed {agreement:?}");
        assert!(agreement[2] + 0.02 >= agreement[1] && agreement[1] + 0.02 >= agreement[0]);
        assert!((got[2].len() as f32 - want.len() as f32).abs() <= 0.25 * want.len() as f32);
    }
}
