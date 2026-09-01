//! WAV loading + resampling to basic-pitch's expected sample rate. Mirrors
//! chord_poc/audio_io.py's load_audio, but for WAV files only (our test
//! fixtures are all WAV; broader format support can come later).

use anyhow::{Context, Result};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use crate::streaming_extractor::AUDIO_SAMPLE_RATE;

pub fn load_audio(path: &str) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).with_context(|| format!("opening {path}"))?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_value = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max_value))
                .collect::<std::result::Result<Vec<f32>, _>>()?
        }
        hound::SampleFormat::Float => {
            reader.samples::<f32>().collect::<std::result::Result<Vec<f32>, _>>()?
        }
    };

    let channels = spec.channels as usize;
    let mono: Vec<f32> = if channels == 1 {
        samples
    } else {
        samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    if spec.sample_rate == AUDIO_SAMPLE_RATE as u32 {
        return Ok(mono);
    }
    resample(&mono, spec.sample_rate as usize, AUDIO_SAMPLE_RATE)
}

fn resample(input: &[f32], from_rate: usize, to_rate: usize) -> Result<Vec<f32>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler = SincFixedIn::<f32>::new(
        to_rate as f64 / from_rate as f64,
        2.0,
        params,
        input.len(),
        1,
    )?;
    let waves_in = vec![input.to_vec()];
    let waves_out = resampler.process(&waves_in, None)?;
    Ok(waves_out.into_iter().next().unwrap_or_default())
}
