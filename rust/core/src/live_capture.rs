//! Loopback capture: stream whatever's currently playing through the
//! default output device, as mono f32 chunks resampled to basic-pitch's
//! expected sample rate. Mirrors chord_poc/live_capture.py.
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
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use crate::streaming_extractor::AUDIO_SAMPLE_RATE;

/// Captures loopback audio and returns a receiver of mono f32 chunks
/// (resampled to AUDIO_SAMPLE_RATE) of approximately `hop_seconds` each.
/// The returned Stream must be kept alive for as long as capture should
/// continue -- dropping it stops the audio callback.
pub fn spawn_loopback_capture(hop_seconds: f32) -> Result<(cpal::Stream, Receiver<Vec<f32>>)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default output device found")?;
    let config = device.default_output_config().map_err(|e| anyhow!("{e}"))?;

    let source_rate = config.sample_rate() as usize;
    let channels = config.channels() as usize;
    let hop_frames_target = (hop_seconds * source_rate as f32).round() as usize;

    let (tx, rx) = sync_channel::<Vec<f32>>(50);

    // The audio callback must never block or do heavy work -- it just
    // downmixes to mono and hands raw native-rate samples off through a
    // channel. Resampling (and everything downstream) happens on the
    // receiving thread instead.
    let (raw_tx, raw_rx) = sync_channel::<Vec<f32>>(200);

    let err_fn = |err| eprintln!("audio stream error: {err}");
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    // The real-time audio callback must never block on I/O, so a dropped
    // chunk (raw_tx full -- the consumer has fallen behind) is only
    // recorded as a lock-free counter here; the resampling thread below
    // polls it and prints a warning on the non-real-time side instead of
    // this failure being completely silent.
    let dropped_chunks = Arc::new(AtomicUsize::new(0));
    let dropped_chunks_cb = Arc::clone(&dropped_chunks);
    let dropped_chunks_cb2 = Arc::clone(&dropped_chunks);

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config.clone(),
            move |data: &[f32], _| {
                let mono = downmix(data, channels);
                if raw_tx.try_send(mono).is_err() {
                    dropped_chunks_cb.fetch_add(1, Ordering::Relaxed);
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config.clone(),
            move |data: &[i16], _| {
                let floats: Vec<f32> = data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                let mono = downmix(&floats, channels);
                if raw_tx.try_send(mono).is_err() {
                    dropped_chunks_cb2.fetch_add(1, Ordering::Relaxed);
                }
            },
            err_fn,
            None,
        ),
        other => return Err(anyhow!("unsupported sample format from loopback device: {other:?}")),
    }
    .map_err(|e| anyhow!("{e}"))?;

    stream.play().map_err(|e| anyhow!("{e}"))?;

    std::thread::spawn(move || {
        let mut resampler = make_resampler(source_rate, AUDIO_SAMPLE_RATE, hop_frames_target);
        let mut pending: Vec<f32> = Vec::new();
        let mut last_reported_drops = 0usize;
        while let Ok(mono_chunk) = raw_rx.recv() {
            let total_drops = dropped_chunks.load(Ordering::Relaxed);
            if total_drops != last_reported_drops {
                eprintln!(
                    "warning: {} audio chunk(s) dropped (consumer falling behind)",
                    total_drops - last_reported_drops
                );
                last_reported_drops = total_drops;
            }

            pending.extend(mono_chunk);
            while pending.len() >= hop_frames_target {
                let chunk: Vec<f32> = pending.drain(..hop_frames_target).collect();
                match resampler.process(&[chunk], None) {
                    Ok(mut out) => {
                        if let Some(resampled) = out.pop() {
                            if tx.send(resampled).is_err() {
                                return; // consumer gone
                            }
                        }
                    }
                    Err(e) => eprintln!("warning: resampler error, dropping this chunk: {e}"),
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
    SincFixedIn::<f32>::new(to_rate as f64 / from_rate as f64, 2.0, params, chunk_size, 1)
        .expect("constructing resampler")
}
