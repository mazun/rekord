//! Streaming glue: turns an arbitrarily-chunked live audio stream into
//! per-block onset/sustain/offset probabilities (`BlockStreamer`, a port of
//! harmonica-repro's `harmonica/infer.py::predict_song_stream`) and then
//! into confirmed notes (`NoteTranscriber`, wrapping a `StreamingDecoder`).
//! See harmonica_model.rs and harmonica_decode.rs for the two halves this
//! wires together, and harmonica-repro's docs/phase5-report.md
//! ("ストリーミングの手順" / "音符への変換") for the spec both port.

use std::collections::{HashSet, VecDeque};

use anyhow::Result;
use ndarray::s;

use crate::harmonica_decode::{ConfirmedNote, DecodedNote, NoteEvent, StreamingDecoder};
use crate::harmonica_model::{
    HarmonicaModel, Part, PartProbs, BLOCK_FRAMES, CHUNK, FRAME_S, HISTORY, HOP, WINDOW_SAMPLES,
};

/// How long a finished note stays in `NoteTranscriber::note_events` after it
/// ends. Those events are the context the chord layers (`chord_layers`) look
/// at around the notes being judged, which only needs the last few seconds.
pub const RETAINED_SECONDS: f32 = 3.3;

/// Writes the stream's samples `[w_start, w_start + out.len())` into `out`: 0.0 before the
/// stream started and past what has been received (the "future audio is never used" rule), else
/// the real samples. `buffer` holds the samples from absolute index `buffer_start_abs` on;
/// `total_received` is how many the stream has had in all.
fn fill_history(
    out: &mut [f32],
    buffer: &VecDeque<f32>,
    buffer_start_abs: usize,
    total_received: usize,
    w_start: i64,
) {
    out.fill(0.0);
    let w_end = w_start + out.len() as i64;
    // The absolute range that exists both in the stream and still in the buffer.
    let lo = w_start.max(buffer_start_abs as i64);
    let hi = w_end
        .min(total_received as i64)
        .min((buffer_start_abs + buffer.len()) as i64);
    if hi <= lo {
        return;
    }
    let (from, to) = (
        lo as usize - buffer_start_abs,
        hi as usize - buffer_start_abs,
    );
    let out = &mut out[(lo - w_start) as usize..(hi - w_start) as usize];
    let n = out.len();
    let (front, back) = buffer.as_slices();
    let in_front = front.len().min(to).saturating_sub(from);
    if in_front > 0 {
        out[..in_front].copy_from_slice(&front[from..from + in_front]);
    }
    if in_front < n {
        let back_from = from + in_front - front.len();
        out[in_front..].copy_from_slice(&back[back_from..back_from + n - in_front]);
    }
}

/// One streaming block's output: per output part, onset/sustain/offset
/// probabilities for frames `[first_frame, first_frame + rows)`, normally
/// BLOCK_FRAMES (8) rows, fewer only for the final block(s) emitted by
/// `finish`. `parts` is in the model's `parts()` order.
pub struct BlockProbs {
    pub first_frame: usize,
    pub parts: Vec<PartProbs>,
}

/// Feeds arbitrarily-sized audio chunks to a `HarmonicaModel` and emits
/// `BlockProbs` every BLOCK_FRAMES (8) frames (0.256s), reproducing
/// `predict_song_stream(..., hop_frames=8)` exactly regardless of how the
/// caller chunks its `push` calls. See the module doc for the reference.
pub struct BlockStreamer {
    model: HarmonicaModel,
    future_frames: usize,
    /// Most-recently-received audio, trimmed from the front as it stops
    /// being needed by any future window (see `drop_before`) -- memory use
    /// stays bounded for an endless live stream.
    buffer: VecDeque<f32>,
    /// Absolute sample index of `buffer[0]` (i.e. how many samples have been
    /// dropped from the front so far).
    buffer_start_abs: usize,
    total_received: usize,
    next_block: usize,
    finished: bool,
    /// The model's input window, kept between blocks: `fill_history` rewrites its first HISTORY
    /// samples, the rest stays 0.
    window: Vec<f32>,
}

impl BlockStreamer {
    pub fn new(model: HarmonicaModel, future_frames: usize) -> Self {
        assert!(
            future_frames + BLOCK_FRAMES <= CHUNK,
            "future_frames + BLOCK_FRAMES must fit in a CHUNK window"
        );
        Self {
            model,
            future_frames,
            buffer: VecDeque::new(),
            buffer_start_abs: 0,
            total_received: 0,
            next_block: 0,
            finished: false,
            window: vec![0.0; WINDOW_SAMPLES],
        }
    }

    pub fn blocks_done(&self) -> usize {
        self.next_block
    }

    fn block_last_frame(&self, b: usize) -> usize {
        BLOCK_FRAMES * b + BLOCK_FRAMES - 1 + self.future_frames
    }

    /// Absolute sample index of the first sample of block `last`'s window
    /// (may be negative for early blocks -- silence before the stream).
    fn window_start_abs(last: usize) -> i64 {
        last as i64 * HOP as i64 - (HISTORY as i64 - 1)
    }

    /// Builds the window for block `b`, runs it through the model, and
    /// returns the BLOCK_FRAMES-frame slice of output it's responsible for.
    /// `t_limit`, only set from `finish`, is the whole-stream frame count T:
    /// frames >= T get masked out of the CQT (same as frames < 0) and
    /// excluded from the returned rows, since there's no more audio coming
    /// to ever fill them in.
    fn process_block(&mut self, b: usize, t_limit: Option<usize>) -> Result<BlockProbs> {
        let last = self.block_last_frame(b);
        let w_start = Self::window_start_abs(last);

        debug_assert!(
            w_start.max(0) >= self.buffer_start_abs as i64,
            "harmonica history underflow: needed sample {w_start} but buffer starts at {}",
            self.buffer_start_abs
        );
        // The trailing WINDOW_SAMPLES - HISTORY samples stay 0 (the buffer is zeroed once, and
        // only its first HISTORY samples are ever written): future audio is never used, whether
        // or not it actually exists yet.
        fill_history(
            &mut self.window[..HISTORY],
            &self.buffer,
            self.buffer_start_abs,
            self.total_received,
            w_start,
        );

        let mut mag = self.model.cqt(&self.window)?;
        let w0 = last as i64 - (CHUNK as i64 - 1);
        for i in 0..CHUNK {
            let frame = w0 + i as i64;
            let out_of_range = frame < 0 || t_limit.is_some_and(|t| frame >= t as i64);
            if out_of_range {
                mag.row_mut(i).fill(0.0);
            }
        }
        let net_parts = self.model.net(mag.view())?;
        let local_start = CHUNK - BLOCK_FRAMES - self.future_frames;
        let local_end = match t_limit {
            // Only rows whose absolute frame (w0 + row) is < T are real.
            // Blocks processed from `push` never need this (see push's doc);
            // this only ever narrows the range during `finish`.
            Some(t) => (t as i64 - w0)
                .clamp(local_start as i64, (CHUNK - self.future_frames) as i64)
                as usize,
            None => CHUNK - self.future_frames,
        };

        let parts = net_parts
            .into_iter()
            .map(|p| PartProbs {
                part: p.part,
                onset: p.onset.slice(s![local_start..local_end, ..]).to_owned(),
                sustain: p.sustain.slice(s![local_start..local_end, ..]).to_owned(),
                offset: p.offset.slice(s![local_start..local_end, ..]).to_owned(),
            })
            .collect();
        Ok(BlockProbs {
            first_frame: BLOCK_FRAMES * b,
            parts,
        })
    }

    /// Drops buffered audio strictly before absolute sample `abs` -- safe
    /// once no window still to be processed starts earlier than that.
    fn drop_before(&mut self, abs: i64) {
        if abs <= self.buffer_start_abs as i64 {
            return;
        }
        let drop_n = (abs as usize - self.buffer_start_abs).min(self.buffer.len());
        self.buffer.drain(0..drop_n);
        self.buffer_start_abs += drop_n;
    }

    /// Appends audio and processes every block that becomes ready as a
    /// result (a single call may trigger several). Every block processed
    /// here has `last <= T-1` for whatever T the stream eventually turns out
    /// to have (its defining sample has, by construction, already arrived),
    /// so unlike `finish`, no frame-mask or row-range needs T here.
    pub fn push(&mut self, audio: &[f32], mut on_block: impl FnMut(BlockProbs)) -> Result<()> {
        assert!(!self.finished, "BlockStreamer::push called after finish");
        self.buffer.extend(audio.iter().copied());
        self.total_received += audio.len();

        loop {
            let b = self.next_block;
            let last = self.block_last_frame(b);
            let needed = last * HOP + 1;
            if self.total_received < needed {
                break;
            }
            let probs = self.process_block(b, None)?;
            on_block(probs);
            self.next_block += 1;

            // Amortized drain: only the next pending block's window start is
            // ever needed from here on.
            let next_last = self.block_last_frame(self.next_block);
            let next_start = Self::window_start_abs(next_last).max(0);
            self.drop_before(next_start);
        }
        Ok(())
    }

    /// Ends the stream: processes every remaining block whose first output
    /// frame is still within bounds, masking out anything at or past the
    /// final frame count T = ceil(samples_received / HOP). No `push` may
    /// follow.
    pub fn finish(&mut self, mut on_block: impl FnMut(BlockProbs)) -> Result<()> {
        assert!(!self.finished, "BlockStreamer::finish called twice");
        let n = self.total_received;
        let t = n.div_ceil(HOP);
        loop {
            let b = self.next_block;
            if BLOCK_FRAMES * b >= t {
                break;
            }
            let probs = self.process_block(b, Some(t))?;
            on_block(probs);
            self.next_block += 1;
        }
        self.finished = true;
        Ok(())
    }
}

/// `BlockStreamer` + one `StreamingDecoder` per output part + a short
/// retained-note history for display. There is no confirm margin: the
/// streaming decode never revises a note once it's decided a note has ended,
/// so each is emitted exactly once, the moment that becomes known -- nothing
/// to "confirm" later.
pub struct NoteTranscriber {
    streamer: BlockStreamer,
    /// One decoder per model part, in the model's `parts()` order, each with
    /// that part's own thresholds. All of them see the same frames, so their
    /// frame counters always agree.
    decoders: Vec<(Part, StreamingDecoder)>,
    future_frames: usize,
    /// Recently-finished notes kept around for `note_events`'s rolling
    /// display window. Pruned with a full `retain` rather than a front-only
    /// pop: notes are appended in the order they're *decided* (start_frame
    /// order within a call, but calls themselves arrive in frame-processing
    /// order), not in end_s order, since a long-held note can close well
    /// after a later-starting, shorter one -- the same reasoning as
    /// `KeyEstimator`'s history pruning (see CLAUDE.md). With
    /// RETAINED_SECONDS this small, the retain stays cheap.
    finished_notes: Vec<ConfirmedNote>,
    /// Open notes `take_started_notes` has already reported, as
    /// (part, pitch, start_frame).
    announced: HashSet<(Part, i32, usize)>,
}

/// A note whose onset is decided but which hasn't ended yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartedNote {
    pub start_s: f32,
    pub pitch_midi: i32,
    pub part: Part,
    pub velocity: f32,
}

impl NoteTranscriber {
    pub fn new(model: HarmonicaModel, future_frames: usize) -> Self {
        let decoders = model
            .parts()
            .iter()
            .zip(model.part_thresholds(future_frames))
            .map(|(&part, &(th_on, th_sus))| (part, StreamingDecoder::new(th_on, th_sus)))
            .collect();
        Self {
            streamer: BlockStreamer::new(model, future_frames),
            decoders,
            future_frames,
            finished_notes: Vec::new(),
            announced: HashSet::new(),
        }
    }

    /// Open notes not reported by an earlier call, so a UI can show a note
    /// from its onset instead of waiting for its end. Each note is later
    /// returned by `push`/`finish` with the same `start_s` and `part`.
    pub fn take_started_notes(&mut self) -> Vec<StartedNote> {
        let mut open = HashSet::new();
        let mut started = Vec::new();
        for (part, decoder) in &self.decoders {
            for (start_frame, pitch, velocity) in decoder.open_notes() {
                let key = (*part, pitch, start_frame);
                open.insert(key);
                if self.announced.insert(key) {
                    started.push(StartedNote {
                        start_s: start_frame as f32 * FRAME_S,
                        pitch_midi: pitch,
                        part: *part,
                        velocity,
                    });
                }
            }
        }
        // Forget notes that have ended, so the set stays bounded.
        self.announced = open;
        started.sort_by(|a, b| a.start_s.partial_cmp(&b.start_s).unwrap());
        started
    }

    pub fn blocks_done(&self) -> usize {
        self.streamer.blocks_done()
    }

    /// Seconds of audio whose note state is final -- nothing at or before
    /// this time will ever change again.
    pub fn covered_seconds(&self) -> f32 {
        self.decoders[0].1.decided_frames() as f32 * FRAME_S
    }

    /// Worst case from a note's true end to the moment it's emitted: an
    /// onset in a block's very last frame only ends the previous note once
    /// the next block arrives and decides it, one full block later.
    pub fn max_note_delay_s(&self) -> f32 {
        (BLOCK_FRAMES + self.future_frames) as f32 * FRAME_S
    }

    fn finalize(&mut self, decoded: Vec<(Part, DecodedNote)>) -> Vec<ConfirmedNote> {
        let mut confirmed: Vec<ConfirmedNote> = decoded
            .into_iter()
            .map(|(part, n)| ConfirmedNote {
                start_s: n.start_frame as f32 * FRAME_S,
                end_s: n.end_frame as f32 * FRAME_S,
                pitch_midi: n.pitch_midi,
                part,
                velocity: n.velocity,
            })
            .collect();
        confirmed.sort_by(|a, b| a.start_s.partial_cmp(&b.start_s).unwrap());
        self.finished_notes.extend(confirmed.iter().copied());
        let cutoff = self.covered_seconds() - RETAINED_SECONDS;
        self.finished_notes.retain(|n| n.end_s >= cutoff);
        confirmed
    }

    /// Feeds audio; returns notes that ended while processing it.
    pub fn push(&mut self, audio: &[f32]) -> Result<Vec<ConfirmedNote>> {
        let mut decoded = Vec::new();
        let decoders = &mut self.decoders;
        self.streamer.push(audio, |block: BlockProbs| {
            push_block(decoders, &block, &mut decoded)
        })?;
        Ok(self.finalize(decoded))
    }

    /// Ends the stream; returns notes that ended as a result, including
    /// every still-open note (closed at the final frame). No `push` may
    /// follow.
    pub fn finish(&mut self) -> Result<Vec<ConfirmedNote>> {
        let mut decoded = Vec::new();
        let decoders = &mut self.decoders;
        self.streamer
            .finish(|block: BlockProbs| push_block(decoders, &block, &mut decoded))?;
        for (part, decoder) in &mut self.decoders {
            decoded.extend(decoder.finish().into_iter().map(|n| (*part, n)));
        }
        Ok(self.finalize(decoded))
    }

    /// Finished notes still within the rolling display window, plus every
    /// currently-open note (shown ending "now", at `covered_seconds()`),
    /// sorted by start.
    pub fn note_events(&self) -> Vec<NoteEvent> {
        let covered = self.covered_seconds();
        let cutoff = covered - RETAINED_SECONDS;
        let mut events: Vec<NoteEvent> = self
            .finished_notes
            .iter()
            .filter(|n| n.end_s >= cutoff)
            .map(|n| (n.start_s, n.end_s, n.pitch_midi, n.velocity, n.part))
            .collect();
        for (part, decoder) in &self.decoders {
            for (start_frame, pitch, vel) in decoder.open_notes() {
                events.push((start_frame as f32 * FRAME_S, covered, pitch, vel, *part));
            }
        }
        events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        events
    }
}

/// Feeds one block's per-part probabilities to the matching decoders (the
/// block's `parts` and `decoders` are both in the model's part order).
fn push_block(
    decoders: &mut [(Part, StreamingDecoder)],
    block: &BlockProbs,
    decoded: &mut Vec<(Part, DecodedNote)>,
) {
    for ((part, decoder), probs) in decoders.iter_mut().zip(&block.parts) {
        debug_assert_eq!(*part, probs.part);
        decoded.extend(
            decoder
                .push(probs.onset.view(), probs.sustain.view())
                .into_iter()
                .map(|n| (*part, n)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The per-sample read `process_block` used to build its window from.
    fn read_sample(
        buffer: &VecDeque<f32>,
        buffer_start_abs: usize,
        total_received: usize,
        abs: i64,
    ) -> f32 {
        if abs < 0 || abs as usize >= total_received {
            return 0.0;
        }
        let idx = (abs as usize).wrapping_sub(buffer_start_abs);
        if idx < buffer.len() {
            buffer[idx]
        } else {
            0.0
        }
    }

    /// A buffer holding samples `[start, start + len)` whose value is the absolute index (+1, so
    /// that a copied sample is never mistaken for silence), laid out in two slices when `wrapped`.
    fn buffer(start: usize, len: usize, wrapped: bool) -> VecDeque<f32> {
        let mut b = VecDeque::with_capacity(len + 8);
        if wrapped {
            // pushing at the front first moves the head, so the contents wrap around the end
            for abs in (start..start + len / 2).rev() {
                b.push_front((abs + 1) as f32);
            }
            for abs in start + len / 2..start + len {
                b.push_back((abs + 1) as f32);
            }
            assert!(
                len < 2 || !b.as_slices().1.is_empty(),
                "the test buffer should be split in two slices"
            );
        } else {
            b.extend((start..start + len).map(|abs| (abs + 1) as f32));
        }
        b
    }

    #[test]
    fn fill_history_equals_reading_sample_by_sample() {
        for wrapped in [false, true] {
            // (buffer start, buffer len, total received): fully buffered, a trimmed front,
            // and a stream that has received less than the buffer would hold windows for
            for (start, len) in [(0usize, 300usize), (120, 180), (0, 7), (50, 1)] {
                let buf = buffer(start, len, wrapped);
                let total_received = start + len;
                for w_start in -40i64..(total_received as i64 + 40) {
                    // windows start no earlier than the buffer, as the streamer guarantees
                    if w_start >= 0 && (w_start as usize) < start {
                        continue;
                    }
                    let mut out = vec![-1.0f32; 64];
                    fill_history(&mut out, &buf, start, total_received, w_start);
                    let want: Vec<f32> = (0..64)
                        .map(|i| read_sample(&buf, start, total_received, w_start + i))
                        .collect();
                    assert_eq!(
                        out, want,
                        "wrapped {wrapped}, buffer {start}+{len}, window at {w_start}"
                    );
                }
            }
        }
    }
}
