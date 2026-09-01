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

use std::sync::mpsc::{sync_channel, Receiver};

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

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config.clone(),
            move |data: &[f32], _| {
                let mono = downmix(data, channels);
                let _ = raw_tx.try_send(mono);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config.clone(),
            move |data: &[i16], _| {
                let floats: Vec<f32> = data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                let mono = downmix(&floats, channels);
                let _ = raw_tx.try_send(mono);
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
        while let Ok(mono_chunk) = raw_rx.recv() {
            pending.extend(mono_chunk);
            while pending.len() >= hop_frames_target {
                let chunk: Vec<f32> = pending.drain(..hop_frames_target).collect();
                if let Some(resampled) = resampler.process(&[chunk], None).ok().and_then(|mut out| out.pop()) {
                    if tx.send(resampled).is_err() {
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
    SincFixedIn::<f32>::new(to_rate as f64 / from_rate as f64, 2.0, params, chunk_size, 1)
        .expect("constructing resampler")
}
