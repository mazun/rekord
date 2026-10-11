//! Playback of a `StereoRing` recording through the default output device,
//! with the transcribed notes synthesized on top of it.
//!
//! Threads: an *engine thread* owns the cpal output stream (it is created and
//! dropped there, like the capture stream) and renders 10 ms blocks ahead into a
//! small bounded queue; the real-time *output callback* only pops blocks, copies
//! them out, and applies a pause/seek fade. No locking or resampling happens
//! in the callback (it only frees finished blocks and queues position reports).
//!
//! Position is reported as `(pos_s, at_epoch_ms)`: the recording time of the
//! frame that is *audible* at the given wall-clock instant (unix epoch, ms).
//! The callback computes it from cpal's own playback latency estimate, so a
//! frontend can extrapolate at exactly 1x from one report without caring about
//! IPC delay or jitter (see `transport.svelte.ts`).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::synth::{render_notes, SynthNote, Voices, Wave, PARTS};
use crate::audio_ring::SharedRing;
use crate::okarina_model::Part;

/// Length of one rendered block.
const BLOCK_S: f64 = 0.010;
/// Blocks rendered ahead of the output callback (the queue bound).
const QUEUE_BLOCKS: usize = 8;
/// Fade length of a pause/resume/seek, in output frames' worth of seconds.
const FADE_S: f64 = 0.005;
/// How often the callback reports the playback position.
const TICK_EVERY_S: f64 = 0.05;
/// How long `pause()` waits for the callback to finish fading out.
const PAUSE_TIMEOUT: Duration = Duration::from_millis(300);

/// How one part's notes sound: a wave and the UI's slider value in `[0, 1]`
/// (0 also stands for a hidden or muted part).
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
pub struct PartVoice {
    pub part: Part,
    pub wave: Wave,
    pub gain: f32,
}

/// Volume and voice settings that can change while playing. The gains are the
/// UI's slider values in `[0, 1]`; the engine squares them (a perceptual-ish
/// taper). `notes_gain` is the master for all synthesized notes, on top of
/// each part's own gain. A part missing from `voices` is silent.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct PlaybackParams {
    pub original_gain: f32,
    pub notes_gain: f32,
    pub voices: Vec<PartVoice>,
}

/// `PlaybackParams` as the renderer uses them: linear gains, one voice per part.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mix {
    original: f32,
    notes: f32,
    voices: Voices,
}

fn taper(slider: f32) -> f32 {
    let v = slider.clamp(0.0, 1.0);
    v * v
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "type")]
pub enum PlaybackEvent {
    /// The recording time `pos_s` is audible at `at_epoch_ms` (unix epoch, ms).
    /// `gen` is the frontend-chosen generation of the play/seek command the
    /// position belongs to; reports of an older generation are stale.
    Position {
        gen: u32,
        pos_s: f64,
        at_epoch_ms: f64,
    },
    /// The last frame of the recording has been heard.
    Ended { gen: u32 },
}

enum Tick {
    Pos {
        gen: u32,
        pos_s: f64,
        at_epoch_ms: f64,
    },
    End {
        gen: u32,
        at_epoch_ms: f64,
    },
}

enum Cmd {
    Seek { pos_s: f64, gen: u32 },
}

struct Shared {
    gen: AtomicU32,
    paused: AtomicBool,
    paused_ack: AtomicBool,
    /// `f64` bits: where the next frame to be played comes from.
    next_pos_bits: AtomicU64,
    stop: AtomicBool,
    /// `f32` bits of the slider values.
    original_gain: AtomicU32,
    notes_gain: AtomicU32,
    /// Per part (`Part as usize`): `Wave` as `u8`, and `f32` bits of the slider value.
    waves: [AtomicU8; PARTS],
    part_gains: [AtomicU32; PARTS],
}

const WAVES: [Wave; 4] = [Wave::Sine, Wave::Triangle, Wave::Square, Wave::Saw];

impl Shared {
    fn new(gen: u32, from_s: f64, params: PlaybackParams) -> Self {
        let shared = Self {
            gen: AtomicU32::new(gen),
            paused: AtomicBool::new(false),
            paused_ack: AtomicBool::new(false),
            next_pos_bits: AtomicU64::new(from_s.to_bits()),
            stop: AtomicBool::new(false),
            original_gain: AtomicU32::new(0),
            notes_gain: AtomicU32::new(0),
            waves: std::array::from_fn(|_| AtomicU8::new(0)),
            part_gains: std::array::from_fn(|_| AtomicU32::new(0)),
        };
        shared.set_params(&params);
        shared
    }

    fn set_params(&self, p: &PlaybackParams) {
        self.original_gain
            .store(p.original_gain.to_bits(), Ordering::Relaxed);
        self.notes_gain
            .store(p.notes_gain.to_bits(), Ordering::Relaxed);
        let mut gains = [0.0f32; PARTS];
        for v in &p.voices {
            let i = v.part as usize;
            self.waves[i].store(
                WAVES.iter().position(|&w| w == v.wave).unwrap_or(0) as u8,
                Ordering::Relaxed,
            );
            gains[i] = v.gain;
        }
        for (slot, gain) in self.part_gains.iter().zip(gains.iter()) {
            slot.store(gain.to_bits(), Ordering::Relaxed);
        }
    }

    fn mix(&self) -> Mix {
        let gain = |bits: &AtomicU32| taper(f32::from_bits(bits.load(Ordering::Relaxed)));
        Mix {
            original: gain(&self.original_gain),
            notes: gain(&self.notes_gain),
            voices: Voices {
                waves: std::array::from_fn(|i| {
                    WAVES[self.waves[i].load(Ordering::Relaxed) as usize % WAVES.len()]
                }),
                gains: std::array::from_fn(|i| gain(&self.part_gains[i])),
            },
        }
    }
}

fn now_epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Rendering (engine thread)
// ---------------------------------------------------------------------------

/// A block of mixed output in the device's format (interleaved f32).
struct Block {
    gen: u32,
    /// Recording time of the first frame.
    pos_s: f64,
    samples: Vec<f32>,
    /// The recording ends within this block.
    last: bool,
}

/// Turns (recording, notes, position) into blocks. Pure apart from reading the
/// ring, so it is testable without a device. The original audio is converted to
/// the device rate by linear interpolation (only needed when the default output
/// device changed its rate since the recording; the common case is a straight
/// copy).
struct Renderer {
    ring: SharedRing,
    notes: Vec<SynthNote>,
    out_rate: u32,
    out_channels: usize,
    src_rate: u32,
    start_s: f64,
    end_s: f64,
    gen: u32,
    from_s: f64,
    /// Fractional source frame that output frame 0 (of this seek) reads.
    base_src_frame: f64,
    frames_done: u64,
    finished: bool,
    prev_mix: Mix,
    src: Vec<f32>,
    mono: Vec<f32>,
}

impl Renderer {
    /// Starts at `from_s` (clamped into the recording), as generation `gen`.
    fn new(
        ring: SharedRing,
        notes: Vec<SynthNote>,
        out_rate: u32,
        out_channels: usize,
        mix: Mix,
        from_s: f64,
        gen: u32,
    ) -> Result<Self> {
        let info = ring
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| r.info())
            .context("no recording")?;
        let mut notes = notes;
        notes.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
        let mut r = Self {
            ring,
            notes,
            out_rate,
            out_channels,
            src_rate: info.sample_rate,
            start_s: info.start_s,
            end_s: info.end_s,
            gen: 0,
            from_s: info.start_s,
            base_src_frame: 0.0,
            frames_done: 0,
            finished: false,
            prev_mix: mix,
            src: Vec::new(),
            mono: Vec::new(),
        };
        r.seek(from_s, gen);
        Ok(r)
    }

    fn seek(&mut self, from_s: f64, gen: u32) {
        self.gen = gen;
        self.from_s = from_s.clamp(self.start_s, self.end_s);
        let frame = self.from_s * self.src_rate as f64;
        // Same rate: a whole-frame start makes the copy bit-exact (no
        // fractional-delay smoothing from the interpolation below).
        self.base_src_frame = if self.src_rate == self.out_rate {
            frame.round()
        } else {
            frame
        };
        self.frames_done = 0;
        self.finished = false;
    }

    fn next_block(&mut self, n: usize, mix: &Mix) -> Block {
        let pos_s = self.from_s + self.frames_done as f64 / self.out_rate as f64;
        let ratio = self.src_rate as f64 / self.out_rate as f64;
        let src0 = self.base_src_frame + self.frames_done as f64 * ratio;
        let i0 = src0.floor();
        let frac0 = src0 - i0;
        let need = (n as f64 * ratio).ceil() as usize + 2;
        self.src.clear();
        self.src.resize(need * 2, 0.0);
        if let Some(ring) = self.ring.lock().unwrap().as_ref() {
            ring.read_f32(i0 as u64, &mut self.src);
        }
        self.mono.resize(n, 0.0);
        render_notes(
            &self.notes,
            pos_s,
            self.out_rate,
            &mix.voices,
            &self.prev_mix.voices.gains,
            &mut self.mono,
        );

        let ch = self.out_channels;
        let mut samples = vec![0.0f32; n * ch];
        let (go0, gs0) = (self.prev_mix.original, self.prev_mix.notes);
        for i in 0..n {
            // Gains ramp linearly across the block: no zipper noise on a slider drag.
            let a = (i + 1) as f32 / n as f32;
            let go = go0 + (mix.original - go0) * a;
            let gs = gs0 + (mix.notes - gs0) * a;
            let s = frac0 + i as f64 * ratio;
            let j = s.floor() as usize;
            let f = (s - j as f64) as f32;
            let l = self.src[2 * j] * (1.0 - f) + self.src[2 * j + 2] * f;
            let r = self.src[2 * j + 1] * (1.0 - f) + self.src[2 * j + 3] * f;
            let notes = self.mono[i] * gs;
            let l = (l * go + notes).clamp(-1.0, 1.0);
            let r = (r * go + notes).clamp(-1.0, 1.0);
            let frame = &mut samples[i * ch..(i + 1) * ch];
            match ch {
                1 => frame[0] = (l + r) * 0.5,
                _ => {
                    frame[0] = l;
                    frame[1] = r;
                }
            }
        }
        self.prev_mix = *mix;
        self.frames_done += n as u64;
        let next_pos_s = self.from_s + self.frames_done as f64 / self.out_rate as f64;
        self.finished = next_pos_s >= self.end_s;
        Block {
            gen: self.gen,
            pos_s,
            samples,
            last: self.finished,
        }
    }
}

// ---------------------------------------------------------------------------
// The output callback (real-time thread)
// ---------------------------------------------------------------------------

struct OutputState {
    rx: Receiver<Block>,
    tick_tx: Sender<Tick>,
    shared: Arc<Shared>,
    channels: usize,
    out_rate: u32,
    cur: Option<Block>,
    /// Frames of `cur` already played.
    offset: usize,
    /// Fade gain, 0..=1, moving towards 1 while playing and 0 while paused or
    /// when `cur` belongs to an older generation (a seek).
    ramp: f32,
    ramp_step: f32,
    frames_since_tick: u32,
    tick_every: u32,
    scratch: Vec<f32>,
}

impl OutputState {
    fn fill(&mut self, out: &mut [f32], latency: Duration) {
        let ch = self.channels;
        let frames = out.len() / ch;
        let gen = self.shared.gen.load(Ordering::Acquire);
        let paused = self.shared.paused.load(Ordering::Acquire);
        let rate = self.out_rate as f64;
        let mut first: Option<(f64, usize)> = None;
        let mut ended_at: Option<usize> = None;
        let mut next_pos = f64::from_bits(self.shared.next_pos_bits.load(Ordering::Relaxed));

        for fi in 0..frames {
            let stale = self.cur.as_ref().is_some_and(|b| b.gen != gen);
            let target = if paused || stale { 0.0 } else { 1.0 };
            if self.ramp < target {
                self.ramp = (self.ramp + self.ramp_step).min(target);
            } else if self.ramp > target {
                self.ramp = (self.ramp - self.ramp_step).max(target);
            }
            let frame_out = &mut out[fi * ch..(fi + 1) * ch];
            if self.ramp <= 0.0 && target == 0.0 {
                frame_out.fill(0.0);
                if stale {
                    self.cur = None;
                    self.offset = 0;
                }
                continue;
            }
            if self
                .cur
                .as_ref()
                .is_none_or(|b| self.offset >= b.samples.len() / ch)
            {
                self.cur = None;
                self.offset = 0;
                while let Ok(b) = self.rx.try_recv() {
                    if b.gen == gen {
                        next_pos = b.pos_s;
                        self.cur = Some(b);
                        break;
                    }
                }
            }
            let Some(b) = self.cur.as_ref() else {
                frame_out.fill(0.0); // underrun (or waiting for the first block of a seek)
                continue;
            };
            let ramp = self.ramp;
            let base = self.offset * ch;
            for (o, s) in frame_out.iter_mut().zip(&b.samples[base..base + ch]) {
                *o = s * ramp;
            }
            // A block of an older generation is only being faded out after a seek; it must not
            // produce position reports, which would carry the new generation.
            let current = b.gen == gen;
            if first.is_none() && current {
                first = Some((b.pos_s + self.offset as f64 / rate, fi));
            }
            self.offset += 1;
            next_pos = b.pos_s + self.offset as f64 / rate;
            if current && b.last && self.offset >= b.samples.len() / ch {
                ended_at = Some(fi + 1);
            }
        }

        self.shared
            .next_pos_bits
            .store(next_pos.to_bits(), Ordering::Relaxed);
        self.shared
            .paused_ack
            .store(paused && self.ramp <= 0.0, Ordering::Release);

        let heard_ms = |frame_index: usize| {
            now_epoch_ms() + (latency.as_secs_f64() + frame_index as f64 / rate) * 1000.0
        };
        if let Some((pos_s, index)) = first {
            self.frames_since_tick += frames as u32;
            if self.frames_since_tick >= self.tick_every {
                self.frames_since_tick = 0;
                let _ = self.tick_tx.send(Tick::Pos {
                    gen,
                    pos_s,
                    at_epoch_ms: heard_ms(index),
                });
            }
        }
        if let Some(index) = ended_at {
            let _ = self.tick_tx.send(Tick::End {
                gen,
                at_epoch_ms: heard_ms(index),
            });
        }
    }

    fn fill_i16(&mut self, out: &mut [i16], latency: Duration) {
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(out.len(), 0.0);
        self.fill(&mut scratch, latency);
        for (o, s) in out.iter_mut().zip(&scratch) {
            *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        }
        self.scratch = scratch;
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// A running (or paused) playback. Dropping it stops the audio and joins the
/// engine thread.
pub struct PlaybackSession {
    shared: Arc<Shared>,
    cmd_tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

/// Starts playing `ring` from `from_s` (clamped into the recording) with the
/// given notes sounded as sines. Blocks until the output stream is running, or
/// returns why it could not be opened (no device, unsupported sample format).
pub fn start(
    ring: SharedRing,
    notes: Vec<SynthNote>,
    from_s: f64,
    params: PlaybackParams,
    gen: u32,
    on_event: impl FnMut(PlaybackEvent) + Send + 'static,
) -> Result<PlaybackSession> {
    let info = ring
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.info())
        .context("no recording to play")?;
    let shared = Arc::new(Shared::new(
        gen,
        from_s.clamp(info.start_s, info.end_s),
        params,
    ));
    let (cmd_tx, cmd_rx) = channel::<Cmd>();
    let (init_tx, init_rx) = sync_channel::<Result<(), String>>(1);

    let thread_shared = Arc::clone(&shared);
    let thread = std::thread::spawn(move || {
        engine_thread(
            ring,
            notes,
            from_s,
            gen,
            thread_shared,
            cmd_rx,
            init_tx,
            on_event,
        );
    });
    match init_rx.recv() {
        Ok(Ok(())) => Ok(PlaybackSession {
            shared,
            cmd_tx,
            thread: Some(thread),
        }),
        Ok(Err(message)) => {
            let _ = thread.join();
            Err(anyhow!(message))
        }
        Err(_) => {
            let _ = thread.join();
            bail!("playback thread died while starting")
        }
    }
}

impl PlaybackSession {
    /// Fades out and holds. Returns the recording time playback will resume
    /// from (the first frame that was not played).
    pub fn pause(&self) -> f64 {
        self.shared.paused_ack.store(false, Ordering::SeqCst);
        self.shared.paused.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + PAUSE_TIMEOUT;
        while !self.shared.paused_ack.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        f64::from_bits(self.shared.next_pos_bits.load(Ordering::Relaxed))
    }

    /// Plays from `pos_s` (also resumes a paused playback). `gen` identifies the
    /// position reports that belong to this jump. Fails once the recording has
    /// been played to its end (the session is over; start a new one).
    pub fn seek(&self, pos_s: f64, gen: u32) -> Result<()> {
        if self.is_finished() {
            bail!("playback has ended");
        }
        self.shared.gen.store(gen, Ordering::Release);
        self.cmd_tx
            .send(Cmd::Seek { pos_s, gen })
            .map_err(|_| anyhow!("playback has ended"))?;
        self.shared.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn set_params(&self, params: &PlaybackParams) {
        self.shared.set_params(params);
    }

    /// The engine thread has exited (the recording was played to its end).
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
}

impl Drop for PlaybackSession {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn engine_thread(
    ring: SharedRing,
    notes: Vec<SynthNote>,
    from_s: f64,
    gen: u32,
    shared: Arc<Shared>,
    cmd_rx: Receiver<Cmd>,
    init_tx: SyncSender<Result<(), String>>,
    mut on_event: impl FnMut(PlaybackEvent),
) {
    let opened = open_stream(ring, notes, from_s, gen, &shared);
    let (stream, mut renderer, block_tx, tick_rx) = match opened {
        Ok(o) => {
            let _ = init_tx.send(Ok(()));
            o
        }
        Err(e) => {
            let _ = init_tx.send(Err(format!("{e:#}")));
            return;
        }
    };
    let block_frames = ((renderer.out_rate as f64 * BLOCK_S).round() as usize).max(1);
    let mut pending: Option<Block> = None;

    'engine: while !shared.stop.load(Ordering::Relaxed) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Seek { pos_s, gen } => {
                    renderer.seek(pos_s, gen);
                    pending = None;
                }
            }
        }
        // Keep the queue full.
        loop {
            if pending.is_none() && !renderer.finished {
                pending = Some(renderer.next_block(block_frames, &shared.mix()));
            }
            let Some(block) = pending.take() else { break };
            match block_tx.try_send(block) {
                Ok(()) => {}
                Err(TrySendError::Full(block)) => {
                    pending = Some(block);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => break 'engine,
            }
        }
        while let Ok(tick) = tick_rx.try_recv() {
            match tick {
                Tick::Pos {
                    gen,
                    pos_s,
                    at_epoch_ms,
                } => on_event(PlaybackEvent::Position {
                    gen,
                    pos_s,
                    at_epoch_ms,
                }),
                Tick::End { gen, at_epoch_ms } => {
                    if gen != shared.gen.load(Ordering::Acquire) {
                        continue; // a seek happened after the callback queued this
                    }
                    let wait = (at_epoch_ms - now_epoch_ms()).clamp(0.0, 1000.0);
                    std::thread::sleep(Duration::from_secs_f64(wait / 1000.0));
                    on_event(PlaybackEvent::Ended { gen });
                    break 'engine;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    drop(stream);
}

type Opened = (cpal::Stream, Renderer, SyncSender<Block>, Receiver<Tick>);

fn open_stream(
    ring: SharedRing,
    notes: Vec<SynthNote>,
    from_s: f64,
    gen: u32,
    shared: &Arc<Shared>,
) -> Result<Opened> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default output device found")?;
    let config = device.default_output_config().map_err(|e| anyhow!("{e}"))?;
    let out_rate = config.sample_rate();
    let channels = config.channels() as usize;
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let renderer = Renderer::new(ring, notes, out_rate, channels, shared.mix(), from_s, gen)?;

    let (block_tx, block_rx) = sync_channel::<Block>(QUEUE_BLOCKS);
    let (tick_tx, tick_rx) = channel::<Tick>();
    let mut state = OutputState {
        rx: block_rx,
        tick_tx,
        shared: Arc::clone(shared),
        channels,
        out_rate,
        cur: None,
        offset: 0,
        ramp: 0.0,
        ramp_step: (1.0 / (FADE_S * out_rate as f64)) as f32,
        frames_since_tick: 0,
        tick_every: (TICK_EVERY_S * out_rate as f64) as u32,
        scratch: Vec::new(),
    };
    // Same filter as the capture stream: a WASAPI "discontinuity" is routine.
    let err_fn = |err: cpal::Error| {
        if err.kind() != cpal::ErrorKind::Xrun {
            eprintln!("audio output error: {err}");
        }
    };
    let latency_of = |info: &cpal::OutputCallbackInfo| {
        let ts = info.timestamp();
        ts.playback.saturating_duration_since(ts.callback)
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            stream_config,
            move |data: &mut [f32], info| state.fill(data, latency_of(info)),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            stream_config,
            move |data: &mut [i16], info| state.fill_i16(data, latency_of(info)),
            err_fn,
            None,
        ),
        other => bail!("unsupported sample format on the output device: {other:?}"),
    }
    .map_err(|e| anyhow!("{e}"))?;
    stream.play().map_err(|e| anyhow!("{e}"))?;
    Ok((stream, renderer, block_tx, tick_rx))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::audio_ring::StereoRing;

    /// Original and notes at the given linear gains, every part a sine at full gain.
    fn mix(original: f32, notes: f32) -> Mix {
        Mix {
            original,
            notes,
            voices: Voices::uniform(Wave::Sine, 1.0),
        }
    }

    /// A ring of `frames` frames at `rate` whose left channel ramps up
    /// (frame / 1000) and whose right channel is its negative.
    fn ramp_ring(rate: u32, frames: usize) -> SharedRing {
        let mut ring = StereoRing::new(rate, frames);
        let data: Vec<f32> = (0..frames)
            .flat_map(|i| [i as f32 / 1000.0, -(i as f32) / 1000.0])
            .collect();
        ring.push_f32(&data, 2);
        Arc::new(Mutex::new(Some(ring)))
    }

    #[test]
    fn same_rate_copies_the_recording_exactly() {
        let mut r =
            Renderer::new(ramp_ring(1000, 500), vec![], 1000, 2, mix(1.0, 0.0), 0.0, 0).unwrap();
        r.seek(0.1, 7);
        let block = r.next_block(10, &mix(1.0, 0.0));
        assert_eq!(block.gen, 7);
        assert!((block.pos_s - 0.1).abs() < 1e-9);
        for i in 0..10 {
            let want = (100 + i) as f32 / 1000.0;
            assert!((block.samples[2 * i] - want).abs() < 1e-3, "frame {i}");
            assert!((block.samples[2 * i + 1] + want).abs() < 1e-3);
        }
        assert!(!block.last);
    }

    #[test]
    fn marks_the_last_block_and_pads_with_silence() {
        let mut r =
            Renderer::new(ramp_ring(1000, 25), vec![], 1000, 2, mix(1.0, 0.0), 0.0, 0).unwrap();
        r.seek(0.0, 1);
        assert!(!r.next_block(10, &mix(1.0, 0.0)).last);
        assert!(!r.next_block(10, &mix(1.0, 0.0)).last);
        let tail = r.next_block(10, &mix(1.0, 0.0));
        assert!(tail.last);
        assert!(r.finished);
        assert!(
            tail.samples[2 * 5..].iter().all(|&s| s == 0.0),
            "past the end is silent"
        );
        assert!(tail.samples[2 * 4].abs() > 0.0);
    }

    #[test]
    fn resamples_to_the_device_rate() {
        // A constant recording stays constant when played at another rate.
        let mut ring = StereoRing::new(1000, 1000);
        ring.push_f32(&vec![0.5; 2000], 2);
        let ring: SharedRing = Arc::new(Mutex::new(Some(ring)));
        let mut r = Renderer::new(ring, vec![], 1500, 2, mix(1.0, 0.0), 0.0, 0).unwrap();
        r.seek(0.2, 1);
        for _ in 0..3 {
            let block = r.next_block(15, &mix(1.0, 0.0));
            assert!(
                block.samples.iter().all(|s| (s - 0.5).abs() < 1e-3),
                "{:?}",
                block.samples
            );
        }
    }

    #[test]
    fn mono_devices_get_the_channel_average_and_notes_are_mixed_in() {
        let note = SynthNote {
            start_s: 0.0,
            end_s: 1.0,
            pitch: 69,
            velocity: 1.0,
            part: Part::Other,
        };
        let mut r = Renderer::new(
            ramp_ring(48_000, 48_000),
            vec![note],
            48_000,
            1,
            mix(0.0, 1.0),
            0.0,
            0,
        )
        .unwrap();
        r.seek(0.5, 1);
        let block = r.next_block(480, &mix(0.0, 1.0));
        assert_eq!(block.samples.len(), 480);
        assert!(
            block.samples.iter().any(|s| s.abs() > 0.05),
            "the notes are audible with the original muted"
        );
        let mut silent = mix(0.0, 1.0);
        silent.voices.gains = [0.0; PARTS];
        let muted = r.next_block(480, &silent);
        // the gain ramps down across the block that mutes, then stays at zero
        assert!(muted.samples[470].abs() < 0.01);
        let muted = r.next_block(480, &silent);
        assert!(
            muted.samples.iter().all(|&s| s == 0.0),
            "no original, every part muted: silence"
        );
    }

    #[test]
    fn params_round_trip_through_the_shared_state() {
        let params = PlaybackParams {
            original_gain: 0.5,
            notes_gain: 1.0,
            voices: vec![
                PartVoice {
                    part: Part::Bass,
                    wave: Wave::Saw,
                    gain: 0.5,
                },
                PartVoice {
                    part: Part::Vocal,
                    wave: Wave::Triangle,
                    gain: 1.0,
                },
            ],
        };
        let mix = Shared::new(0, 0.0, params).mix();
        assert_eq!(mix.original, 0.25);
        assert_eq!(mix.notes, 1.0);
        assert_eq!(mix.voices.waves[Part::Bass as usize], Wave::Saw);
        assert_eq!(mix.voices.waves[Part::Vocal as usize], Wave::Triangle);
        assert_eq!(mix.voices.gains[Part::Bass as usize], 0.25);
        assert_eq!(
            mix.voices.gains[Part::Other as usize],
            0.0,
            "a part that wasn't listed is silent"
        );
    }
}
