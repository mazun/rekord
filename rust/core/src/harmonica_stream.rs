//! Streaming glue: turns an arbitrarily-chunked live audio stream into
//! per-block onset/sustain/offset probabilities (`BlockStreamer`, a port of
//! harmonica-repro's `harmonica/infer.py::predict_song_stream`) and then
//! into confirmed notes (`NoteTranscriber`, wrapping a `StreamingDecoder`).
//! See harmonica_model.rs and harmonica_decode.rs for the two halves this
//! wires together, and harmonica-repro's docs/phase5-report.md
//! ("ストリーミングの手順" / "音符への変換") for the spec both port.

use std::collections::VecDeque;

use anyhow::Result;
use ndarray::{s, Array2};

use crate::harmonica_decode::{ConfirmedNote, DecodedNote, NoteEvent, StreamingDecoder};
use crate::harmonica_model::{HarmonicaModel, BLOCK_FRAMES, CHUNK, FRAME_S, HISTORY, HOP, WINDOW_SAMPLES};

/// How long a finished note stays in `NoteTranscriber::note_events` after it
/// ends. Those events are the context chord matching (`pipeline.rs`) and the
/// melody/bass labeling (`melody_bass.rs`) look at around the notes being
/// judged, which only needs the last few seconds; the value keeps what the
/// basic-pitch extractor used to retain.
pub const RETAINED_SECONDS: f32 = 3.3;

/// One streaming block's output: onset/sustain/offset probabilities for
/// frames `[first_frame, first_frame + rows)`, normally BLOCK_FRAMES (8)
/// rows, fewer only for the final block(s) emitted by `finish`.
pub struct BlockProbs {
    pub first_frame: usize,
    pub onset: Array2<f32>,
    pub sustain: Array2<f32>,
    pub offset: Array2<f32>,
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
}

impl BlockStreamer {
    pub fn new(model: HarmonicaModel, future_frames: usize) -> Self {
        assert!(future_frames + BLOCK_FRAMES <= CHUNK, "future_frames + BLOCK_FRAMES must fit in a CHUNK window");
        Self {
            model,
            future_frames,
            buffer: VecDeque::new(),
            buffer_start_abs: 0,
            total_received: 0,
            next_block: 0,
            finished: false,
        }
    }

    pub fn model(&self) -> &HarmonicaModel {
        &self.model
    }

    pub fn blocks_done(&self) -> usize {
        self.next_block
    }

    pub fn samples_received(&self) -> usize {
        self.total_received
    }

    /// Sample `abs` (an absolute index into the whole stream, possibly
    /// negative or not-yet-received): 0.0 before the stream started or past
    /// what's been received (the "future audio is never used" rule), else
    /// the real sample.
    fn read_sample(&self, abs: i64) -> f32 {
        if abs < 0 {
            return 0.0;
        }
        let abs = abs as usize;
        if abs >= self.total_received {
            return 0.0;
        }
        debug_assert!(
            abs >= self.buffer_start_abs,
            "harmonica history underflow: needed sample {abs} but buffer starts at {}",
            self.buffer_start_abs
        );
        let idx = abs.wrapping_sub(self.buffer_start_abs);
        if idx < self.buffer.len() {
            self.buffer[idx]
        } else {
            0.0
        }
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

        let mut window = vec![0f32; WINDOW_SAMPLES];
        for (i, sample) in window.iter_mut().enumerate().take(HISTORY) {
            *sample = self.read_sample(w_start + i as i64);
        }
        // The trailing WINDOW_SAMPLES - HISTORY samples stay 0: future audio
        // is never used, whether or not it actually exists yet.

        let mut mag = self.model.cqt(&window)?;
        let w0 = last as i64 - (CHUNK as i64 - 1);
        for i in 0..CHUNK {
            let frame = w0 + i as i64;
            let out_of_range = frame < 0 || t_limit.is_some_and(|t| frame >= t as i64);
            if out_of_range {
                mag.row_mut(i).fill(0.0);
            }
        }
        let [onset, sustain, offset] = self.model.net(mag.view())?;
        let local_start = CHUNK - BLOCK_FRAMES - self.future_frames;
        let local_end = match t_limit {
            // Only rows whose absolute frame (w0 + row) is < T are real.
            // Blocks processed from `push` never need this (see push's doc);
            // this only ever narrows the range during `finish`.
            Some(t) => (t as i64 - w0).clamp(local_start as i64, (CHUNK - self.future_frames) as i64) as usize,
            None => CHUNK - self.future_frames,
        };

        Ok(BlockProbs {
            first_frame: BLOCK_FRAMES * b,
            onset: onset.slice(s![local_start..local_end, ..]).to_owned(),
            sustain: sustain.slice(s![local_start..local_end, ..]).to_owned(),
            offset: offset.slice(s![local_start..local_end, ..]).to_owned(),
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

/// `BlockStreamer` + `StreamingDecoder` + a short retained-note history for
/// display. There is no confirm margin (unlike basic-pitch's
/// `NoteStitcher`): the streaming decode never revises a note once it's
/// decided a note has ended, so each is emitted exactly once, the moment
/// that becomes known -- nothing to "confirm" later.
pub struct NoteTranscriber {
    streamer: BlockStreamer,
    decoder: StreamingDecoder,
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
}

impl NoteTranscriber {
    pub fn new(model: HarmonicaModel, future_frames: usize) -> Self {
        let (th_on, th_sus) = model.size().thresholds();
        Self {
            streamer: BlockStreamer::new(model, future_frames),
            decoder: StreamingDecoder::new(th_on, th_sus),
            future_frames,
            finished_notes: Vec::new(),
        }
    }

    pub fn model(&self) -> &HarmonicaModel {
        self.streamer.model()
    }

    pub fn future_frames(&self) -> usize {
        self.future_frames
    }

    pub fn blocks_done(&self) -> usize {
        self.streamer.blocks_done()
    }

    /// Seconds of audio whose note state is final -- nothing at or before
    /// this time will ever change again.
    pub fn covered_seconds(&self) -> f32 {
        self.decoder.decided_frames() as f32 * FRAME_S
    }

    /// Worst case from a note's true end to the moment it's emitted: an
    /// onset in a block's very last frame only ends the previous note once
    /// the next block arrives and decides it, one full block later.
    pub fn max_note_delay_s(&self) -> f32 {
        (BLOCK_FRAMES + self.future_frames) as f32 * FRAME_S
    }

    fn finalize(&mut self, decoded: Vec<DecodedNote>) -> Vec<ConfirmedNote> {
        let mut confirmed: Vec<ConfirmedNote> = decoded
            .into_iter()
            .map(|n| ConfirmedNote { start_s: n.start_frame as f32 * FRAME_S, end_s: n.end_frame as f32 * FRAME_S, pitch_midi: n.pitch_midi, velocity: n.velocity })
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
        let decoder = &mut self.decoder;
        self.streamer.push(audio, |block: BlockProbs| {
            decoded.extend(decoder.push(block.onset.view(), block.sustain.view()));
        })?;
        Ok(self.finalize(decoded))
    }

    /// Ends the stream; returns notes that ended as a result, including
    /// every still-open note (closed at the final frame). No `push` may
    /// follow.
    pub fn finish(&mut self) -> Result<Vec<ConfirmedNote>> {
        let mut decoded = Vec::new();
        let decoder = &mut self.decoder;
        self.streamer.finish(|block: BlockProbs| {
            decoded.extend(decoder.push(block.onset.view(), block.sustain.view()));
        })?;
        decoded.extend(self.decoder.finish());
        Ok(self.finalize(decoded))
    }

    /// Finished notes still within the rolling display window, plus every
    /// currently-open note (shown ending "now", at `covered_seconds()`),
    /// sorted by start.
    pub fn note_events(&self) -> Vec<NoteEvent> {
        let covered = self.covered_seconds();
        let cutoff = covered - RETAINED_SECONDS;
        let mut events: Vec<NoteEvent> =
            self.finished_notes.iter().filter(|n| n.end_s >= cutoff).map(|n| (n.start_s, n.end_s, n.pitch_midi, n.velocity)).collect();
        for (start_frame, pitch, vel) in self.decoder.open_notes() {
            events.push((start_frame as f32 * FRAME_S, covered, pitch, vel));
        }
        events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        events
    }
}
