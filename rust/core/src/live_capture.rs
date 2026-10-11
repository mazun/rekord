//! Loopback capture: stream whatever's currently playing through the
//! default output device, as mono f32 chunks resampled to whatever rate(s)
//! a caller needs. Mirrors chord_poc/live_capture.py.
//!
//! Uses cpal (the standard RustAudio I/O crate) rather than a WASAPI-only
//! crate: cpal supports loopback on Windows by simply building an *input*
//! stream from the default *output* device -- it detects this and sets
//! the WASAPI loopback flag internally (see RustAudio/cpal#339). This
//! also avoids the failure mode hit in the Python prototype, where
//! soundcard's name-based device lookup silently resolved to a real line
//! input that happened to share a display name with the loopback
//! endpoint: cpal never looks anything up by name here, it captures
//! directly from the output device handle we already hold.
//!
//! Loopback itself is inherently platform-specific (WASAPI on Windows;
//! macOS/Linux need different mechanisms cpal doesn't yet expose the same
//! way), but keeping capture on cpal means the resampling/buffering code
//! below doesn't have to change if that support lands later.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// How often the capture worker checks whether the device has gone quiet.
const SILENCE_POLL: Duration = Duration::from_millis(50);
/// Silence is only fed in once the device is this far behind real time, so ordinary packet
/// jitter never inserts any.
const MIN_SILENCE_FILL_S: f64 = 0.1;

/// The capture device's native format, as handed to a `CaptureTap`.
#[derive(Clone, Copy, Debug)]
pub struct CaptureFormat {
    pub sample_rate: u32,
    pub channels: usize,
}

/// Sees the raw native-rate interleaved audio before it is downmixed and
/// resampled (the recording for playback is taken here). Called on the capture
/// worker thread -- never inside the real-time audio callback -- from the same
/// chunk stream the pipeline is fed, so a chunk the pipeline never sees (see
/// `dropped_chunks`) is never seen here either, and the two stay time-aligned.
pub trait CaptureTap: Send + 'static {
    /// Once, before the first chunk of a capture.
    fn begin(&mut self, format: CaptureFormat);
    /// Interleaved f32 frames, in capture order.
    fn chunk(&mut self, interleaved: &[f32]);
}

/// Captures loopback audio and returns a receiver of `N`-tuples of mono f32
/// chunks (resampled to each of `rates`, in order), each covering
/// approximately `hop_seconds` of audio. Every element of a given tuple
/// covers the same span of source audio -- they're produced together from
/// one native-rate chunk, so a caller feeding element `i` to a consumer that
/// expects `rates[i]` never needs to re-derive timing across the two.
/// Resampling once per target rate from the native rate (rather than, say,
/// deriving one rate from another) avoids stacking a second resampling
/// stage's error/latency on top of the first for whichever rate would
/// otherwise be secondary.
///
/// `tap`, if given, additionally sees every native-rate chunk first.
///
/// The returned Stream must be kept alive for as long as capture should
/// continue -- dropping it stops the audio callback.
pub fn spawn_loopback_capture<const N: usize>(
    hop_seconds: f32,
    rates: [usize; N],
    mut tap: Option<Box<dyn CaptureTap>>,
) -> Result<(cpal::Stream, Receiver<[Vec<f32>; N]>)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default output device found")?;
    let config = device.default_output_config().map_err(|e| anyhow!("{e}"))?;

    let source_rate = config.sample_rate() as usize;
    let channels = config.channels() as usize;
    let hop_frames_target = (hop_seconds * source_rate as f32).round() as usize;

    let (tx, rx) = sync_channel::<[Vec<f32>; N]>(50);

    // The audio callback must never block or do heavy work -- it just hands
    // the raw interleaved native-rate samples off through a channel.
    // Downmixing, resampling (and everything downstream) happen on the
    // receiving thread instead.
    let (raw_tx, raw_rx) = sync_channel::<Vec<f32>>(200);

    // cpal reports `Xrun` whenever WASAPI flags a packet as a data
    // discontinuity. On loopback that is routine (capture starting while the
    // endpoint is idle, playback stopping/starting) and says nothing about
    // our consumer -- real overload shows up as `dropped_chunks` below -- so
    // it is not worth printing on every launch.
    let err_fn = |err: cpal::Error| {
        if err.kind() != cpal::ErrorKind::Xrun {
            eprintln!("audio stream error: {err}");
        }
    };
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    // The real-time audio callback must never block on I/O, so a dropped
    // chunk (raw_tx full -- the consumer has fallen behind) is only
    // recorded as a lock-free counter here; the resampling thread below
    // polls it and prints a warning on the non-real-time side instead of
    // this failure being completely silent.
    let dropped_chunks = Arc::new(AtomicUsize::new(0));
    let dropped_chunks_cb = Arc::clone(&dropped_chunks);

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config,
            move |data: &[f32], _| {
                if raw_tx.try_send(data.to_vec()).is_err() {
                    dropped_chunks_cb.fetch_add(1, Ordering::Relaxed);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _| {
                let floats: Vec<f32> = data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                if raw_tx.try_send(floats).is_err() {
                    dropped_chunks_cb.fetch_add(1, Ordering::Relaxed);
                }
            },
            err_fn,
            None,
        ),
        other => {
            return Err(anyhow!(
                "unsupported sample format from loopback device: {other:?}"
            ))
        }
    }
    .map_err(|e| anyhow!("{e}"))?;

    stream.play().map_err(|e| anyhow!("{e}"))?;

    std::thread::spawn(move || {
        let mut resamplers: [SincFixedIn<f32>; N] =
            std::array::from_fn(|i| make_resampler(source_rate, rates[i], hop_frames_target));
        let mut pending: Vec<f32> = Vec::new();
        let mut last_reported_drops = 0usize;
        if let Some(tap) = tap.as_mut() {
            tap.begin(CaptureFormat {
                sample_rate: source_rate as u32,
                channels,
            });
        }
        // Loopback sends nothing while nothing is playing, which would freeze the audio time
        // (every timestamp downstream) while the wall clock goes on, so the roll would draw the
        // next sound that far in the past. Silence is fed in for whatever the device didn't
        // deliver, keeping audio time = time since capture start.
        let started_at = Instant::now();
        let mut fed_frames = 0u64;
        loop {
            let delivered = match raw_rx.recv_timeout(SILENCE_POLL) {
                Ok(raw_chunk) => Some(raw_chunk),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let due_frames = (started_at.elapsed().as_secs_f64() * source_rate as f64) as u64;
            let missing_frames = due_frames.saturating_sub(fed_frames);
            let mut chunks: Vec<Vec<f32>> = Vec::new();
            if missing_frames as f64 >= MIN_SILENCE_FILL_S * source_rate as f64 {
                chunks.push(vec![0.0; missing_frames as usize * channels]);
            }
            chunks.extend(delivered);

            for raw_chunk in chunks {
                fed_frames += (raw_chunk.len() / channels) as u64;
                let total_drops = dropped_chunks.load(Ordering::Relaxed);
                if total_drops != last_reported_drops {
                    eprintln!(
                        "warning: {} audio chunk(s) dropped (consumer falling behind)",
                        total_drops - last_reported_drops
                    );
                    last_reported_drops = total_drops;
                }

                if let Some(tap) = tap.as_mut() {
                    tap.chunk(&raw_chunk);
                }
                pending.extend(downmix(&raw_chunk, channels));
                while pending.len() >= hop_frames_target {
                    let chunk: Vec<f32> = pending.drain(..hop_frames_target).collect();
                    // Every resampler consumes the same native-rate chunk, so
                    // they stay in lockstep -- one output tuple per chunk,
                    // never a partial one if a single rate's resampler errors.
                    let mut outputs: [Vec<f32>; N] = std::array::from_fn(|_| Vec::new());
                    let mut ok = true;
                    for (resampler, out) in resamplers.iter_mut().zip(outputs.iter_mut()) {
                        match resampler.process(&[chunk.as_slice()], None) {
                            Ok(mut result) => *out = result.pop().unwrap_or_default(),
                            Err(e) => {
                                eprintln!("warning: resampler error, dropping this chunk: {e}");
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok && tx.send(outputs).is_err() {
                        return; // consumer gone
                    }
                }
            }
        }
    });

    Ok((stream, rx))
}

fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn make_resampler(from_rate: usize, to_rate: usize, chunk_size: usize) -> SincFixedIn<f32> {
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    SincFixedIn::<f32>::new(
        to_rate as f64 / from_rate as f64,
        2.0,
        params,
        chunk_size,
        1,
    )
    .expect("constructing resampler")
}
