//! Synthesis of the transcribed drum hits, for playback: a pitch-swept sine for the kick and
//! toms, a sine plus noise for the snare, high-passed noise for the hi-hat and cymbal.
//!
//! Pure like `synth`: a block is rendered from the hit list and a start time alone. The noise is a
//! hash of the absolute sample index, so seeking or splitting a span into blocks differently gives
//! the same waveform and no per-voice state is needed.

use std::f64::consts::TAU;

use crate::drum_stream::Drum;

/// A hit to sound.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
pub struct SynthDrumHit {
    /// Audio time of the hit.
    pub t: f64,
    pub drum: Drum,
    /// The model's activation at the hit (0..1); sets how loud it sounds.
    pub strength: f32,
}

/// Peak amplitude of one hit at full strength, before the per-drum level.
const DRUM_AMPLITUDE: f64 = 0.5;
/// The longest any drum rings (the cymbal); hits older than this have fully decayed.
const MAX_HIT_S: f64 = 1.2;
/// Linear fade over the end of a hit's length, so the cut-off doesn't click.
const TAIL_FADE_S: f64 = 0.02;

/// How long a hit sounds.
fn length_s(drum: Drum) -> f64 {
    match drum {
        Drum::Kick => 0.5,
        Drum::Snare => 0.4,
        Drum::Tom => 0.6,
        Drum::HiHat => 0.15,
        Drum::Cymbal => MAX_HIT_S,
    }
}

/// Rough loudness equalization between the drums. Untuned.
fn level(drum: Drum) -> f64 {
    match drum {
        Drum::Kick => 1.0,
        Drum::Snare => 0.8,
        Drum::Tom => 0.9,
        Drum::HiHat => 0.45,
        Drum::Cymbal => 0.4,
    }
}

/// White noise in `[-1, 1)` that depends on the sample index alone (SplitMix64).
fn noise(k: i64) -> f64 {
    let mut z = (k as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// High-passed noise (the first difference), the body of the hi-hat and cymbal.
fn bright_noise(k: i64) -> f64 {
    0.5 * (noise(k) - noise(k - 1))
}

/// A sine whose frequency falls exponentially from `f0` to `f1` (time constant `tau`), at age
/// `a`. The phase is the integral of the frequency, so it is analytic in `a`.
fn sweep(a: f64, f0: f64, f1: f64, tau: f64) -> f64 {
    let phase = f1 * a + (f0 - f1) * tau * (1.0 - (-a / tau).exp());
    (TAU * phase).sin()
}

/// One drum's waveform at age `a` seconds after its hit, at sample index `k`.
fn voice(drum: Drum, a: f64, k: i64) -> f64 {
    match drum {
        Drum::Kick => sweep(a, 180.0, 62.0, 0.03) * (-a / 0.12).exp(),
        Drum::Snare => {
            0.5 * sweep(a, 190.0, 170.0, 0.05) * (-a / 0.06).exp()
                + 0.8 * noise(k) * (-a / 0.08).exp()
        }
        Drum::Tom => sweep(a, 190.0, 100.0, 0.05) * (-a / 0.15).exp(),
        Drum::HiHat => bright_noise(k) * (-a / 0.02).exp(),
        Drum::Cymbal => bright_noise(k) * (-a / 0.25).exp(),
    }
}

/// Renders `out.len()` mono samples starting at recording time `t0_s`, overwriting `out`. `hits`
/// must be sorted by `t`. The sum is soft-clipped (`tanh`), so `|out| < 1`.
pub fn render_drums(hits: &[SynthDrumHit], t0_s: f64, rate: u32, out: &mut [f32]) {
    out.fill(0.0);
    let n = out.len();
    let dt = 1.0 / rate as f64;
    let t_end = t0_s + n as f64 * dt;
    let candidates = hits.partition_point(|hit| hit.t < t_end);
    for hit in hits[..candidates].iter().rev() {
        if hit.t + MAX_HIT_S <= t0_s {
            break; // sorted: every earlier hit has decayed too
        }
        let len = length_s(hit.drum);
        let amp =
            DRUM_AMPLITUDE * level(hit.drum) * (0.4 + 0.6 * hit.strength.clamp(0.0, 1.0) as f64);
        let from = (((hit.t - t0_s) / dt).ceil().max(0.0)) as usize;
        let to = ((((hit.t + len) - t0_s) / dt).ceil().max(0.0) as usize).min(n);
        for (i, sample) in out.iter_mut().enumerate().take(to).skip(from) {
            let t = t0_s + i as f64 * dt;
            let a = t - hit.t;
            let fade = ((len - a) / TAIL_FADE_S).clamp(0.0, 1.0);
            let k = (t * rate as f64).round() as i64;
            *sample += (amp * fade * voice(hit.drum, a, k)) as f32;
        }
    }
    for sample in out.iter_mut() {
        *sample = sample.tanh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn hit(t: f64, drum: Drum) -> SynthDrumHit {
        SynthDrumHit {
            t,
            drum,
            strength: 1.0,
        }
    }

    fn render(hits: &[SynthDrumHit], t0: f64, n: usize) -> Vec<f32> {
        let mut out = vec![0.0; n];
        render_drums(hits, t0, RATE, &mut out);
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn silent_before_the_hit_and_after_its_length() {
        for drum in Drum::ALL {
            let hits = [hit(1.0, drum)];
            assert!(
                render(&hits, 0.0, 4800).iter().all(|&s| s == 0.0),
                "{drum:?}"
            );
            let after = 1.0 + length_s(drum) + 0.001;
            assert!(
                render(&hits, after, 4800).iter().all(|&s| s == 0.0),
                "{drum:?}"
            );
            let sounding = render(&hits, 1.0, 2400);
            assert!(rms(&sounding) > 0.01, "{drum:?} is audible");
            assert!(sounding.iter().all(|s| s.abs() < 1.0));
        }
    }

    #[test]
    fn block_boundaries_do_not_change_the_waveform() {
        let hits = [
            hit(0.0, Drum::Kick),
            hit(0.05, Drum::Snare),
            hit(0.1, Drum::HiHat),
            hit(0.12, Drum::Cymbal),
        ];
        let whole = render(&hits, 0.0, 9000);
        let first = render(&hits, 0.0, 3100);
        let second = render(&hits, 3100.0 / RATE as f64, 5900);
        for (i, w) in whole.iter().enumerate() {
            let s = if i < 3100 { first[i] } else { second[i - 3100] };
            assert!((w - s).abs() < 1e-6, "sample {i}: {w} vs {s}");
        }
    }

    #[test]
    fn a_hit_decays_and_a_weak_one_is_quieter() {
        let loud = render(&[hit(0.0, Drum::Snare)], 0.0, 14_400);
        assert!(
            rms(&loud[..480]) > rms(&loud[12_000..12_480]) * 3.0,
            "decays"
        );
        let mut weak = hit(0.0, Drum::Snare);
        weak.strength = 0.1;
        let quiet = render(&[weak], 0.0, 14_400);
        assert!(rms(&quiet) < rms(&loud));
    }

    #[test]
    fn the_kick_is_low_and_the_hi_hat_is_bright() {
        let crossings = |x: &[f32]| x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let kick = render(&[hit(0.0, Drum::Kick)], 0.1, 4800);
        let hat = render(&[hit(0.0, Drum::HiHat)], 0.0, 4800);
        assert!(crossings(&kick) < 20, "{}", crossings(&kick));
        assert!(crossings(&hat) > 500, "{}", crossings(&hat));
    }

    #[test]
    fn long_ringing_hits_before_the_block_still_sound() {
        let hits = [hit(0.0, Drum::Cymbal), hit(5.0, Drum::Kick)];
        assert!(
            rms(&render(&hits, 0.5, 4800)) > 0.001,
            "the cymbal is still ringing"
        );
        assert!(render(&hits, 3.0, 4800).iter().all(|&s| s == 0.0));
    }
}
