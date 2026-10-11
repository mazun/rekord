//! Synthesis of the transcribed notes (sine, triangle, square or sawtooth
//! waves, chosen per part), for playback.
//!
//! Pure: a block is rendered from the note list and a start time alone. The
//! phase is analytic (`frequency * (t - start)`, no phase accumulators), so
//! seeking to any time, or splitting a span into blocks differently, gives the
//! same waveform and needs no per-voice state. The square and sawtooth are
//! band-limited with PolyBLEP, which is also a function of the phase alone, so
//! high notes don't alias.

use std::f64::consts::TAU;

use crate::okarina_model::Part;

/// Linear fade-in from a note's onset (avoids a click at the start).
pub const ATTACK_S: f64 = 0.008;
/// Linear fade-out after a note's end.
pub const RELEASE_S: f64 = 0.040;
/// Notes sounding at once beyond this are dropped (the lowest-indexed, i.e.
/// earliest-starting ones win) -- bounds CPU and keeps a dense passage from
/// being all soft-clip.
pub const MAX_VOICES: usize = 64;

/// Peak amplitude of one note at full velocity.
const NOTE_AMPLITUDE: f64 = 0.12;

/// Number of `Part` variants (`Vocal`, `Bass`, `Other`).
pub const PARTS: usize = 3;

/// A note to sound.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
pub struct SynthNote {
    pub start_s: f64,
    pub end_s: f64,
    pub pitch: u8,
    pub velocity: f32,
    pub part: Part,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wave {
    Sine,
    Triangle,
    Square,
    Saw,
}

impl Wave {
    /// Rough loudness equalization between the waves (a square or sawtooth of
    /// the same peak is far louder than a sine). Untuned.
    fn level(self) -> f64 {
        match self {
            Wave::Sine => 1.0,
            Wave::Triangle => 1.15,
            Wave::Square => 0.45,
            Wave::Saw => 0.6,
        }
    }

    /// The wave at `phase` in `[0, 1)`, `dt` being how far the phase advances
    /// per sample. All start at (or, for the sawtooth, next to) zero.
    fn sample(self, phase: f64, dt: f64) -> f64 {
        match self {
            Wave::Sine => (TAU * phase).sin(),
            Wave::Triangle => 1.0 - 2.0 * (2.0 * (phase + 0.25).fract() - 1.0).abs(),
            Wave::Square => {
                let naive = if phase < 0.5 { 1.0 } else { -1.0 };
                naive + poly_blep(phase, dt) - poly_blep((phase + 0.5).fract(), dt)
            }
            Wave::Saw => 2.0 * phase - 1.0 - poly_blep(phase, dt),
        }
    }
}

/// The correction that smooths a unit step at phase 0 over one sample either side.
fn poly_blep(phase: f64, dt: f64) -> f64 {
    if phase < dt {
        let t = phase / dt;
        t + t - t * t - 1.0
    } else if phase > 1.0 - dt {
        let t = (phase - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// How each part sounds: its wave and a linear gain (0 = silent, which is how
/// a hidden or muted part is expressed). Indexed by `Part as usize`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voices {
    pub waves: [Wave; PARTS],
    pub gains: [f32; PARTS],
}

impl Voices {
    pub fn uniform(wave: Wave, gain: f32) -> Self {
        Self {
            waves: [wave; PARTS],
            gains: [gain; PARTS],
        }
    }
}

fn envelope(note: &SynthNote, t: f64) -> f64 {
    if t < note.start_s {
        return 0.0;
    }
    let attack = ((t - note.start_s) / ATTACK_S).min(1.0);
    let release = if t <= note.end_s {
        1.0
    } else {
        (1.0 - (t - note.end_s) / RELEASE_S).max(0.0)
    };
    attack * release
}

/// Renders `out.len()` mono samples starting at recording time `t0_s`,
/// overwriting `out`. `notes` must be sorted by `start_s`. Each part sounds as
/// `voices` says; its gain moves linearly from `from_gains` (where the previous
/// block ended) to `voices.gains` across the block, so changing a volume or
/// muting doesn't click. Notes of a part silent at both ends are skipped. The
/// sum is soft-clipped (`tanh`), so `|out| < 1`.
pub fn render_notes(
    notes: &[SynthNote],
    t0_s: f64,
    rate: u32,
    voices: &Voices,
    from_gains: &[f32; PARTS],
    out: &mut [f32],
) {
    out.fill(0.0);
    let n = out.len();
    let dt = 1.0 / rate as f64;
    let t_end = t0_s + n as f64 * dt;
    let candidates = notes.partition_point(|note| note.start_s < t_end);
    let mut sounding = 0;
    for note in &notes[..candidates] {
        let part = note.part as usize;
        if note.end_s + RELEASE_S <= t0_s || (voices.gains[part] == 0.0 && from_gains[part] == 0.0)
        {
            continue;
        }
        sounding += 1;
        if sounding > MAX_VOICES {
            break;
        }
        let freq = 440.0 * 2f64.powf((note.pitch as f64 - 69.0) / 12.0);
        let wave = voices.waves[part];
        let amp =
            NOTE_AMPLITUDE * wave.level() * (0.5 + 0.5 * note.velocity.clamp(0.0, 1.0) as f64);
        let (g0, g1) = (from_gains[part] as f64, voices.gains[part] as f64);
        let from = (((note.start_s - t0_s) / dt).floor().max(0.0)) as usize;
        let to = ((((note.end_s + RELEASE_S) - t0_s) / dt).ceil().max(0.0) as usize).min(n);
        for (i, sample) in out.iter_mut().enumerate().take(to).skip(from) {
            let t = t0_s + i as f64 * dt;
            let env = envelope(note, t);
            if env > 0.0 {
                let gain = g0 + (g1 - g0) * (i + 1) as f64 / n as f64;
                let phase = (freq * (t - note.start_s)).fract();
                *sample += (amp * gain * env * wave.sample(phase, freq * dt)) as f32;
            }
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

    fn note(start_s: f64, end_s: f64, pitch: u8, part: Part) -> SynthNote {
        SynthNote {
            start_s,
            end_s,
            pitch,
            velocity: 1.0,
            part,
        }
    }

    fn render_with(notes: &[SynthNote], t0: f64, n: usize, voices: &Voices) -> Vec<f32> {
        let mut out = vec![0.0; n];
        render_notes(notes, t0, RATE, voices, &voices.gains, &mut out);
        out
    }

    fn render(notes: &[SynthNote], t0: f64, n: usize) -> Vec<f32> {
        render_with(notes, t0, n, &Voices::uniform(Wave::Sine, 1.0))
    }

    #[test]
    fn a440_has_the_right_frequency_in_every_wave() {
        for wave in [Wave::Sine, Wave::Triangle, Wave::Square, Wave::Saw] {
            let out = render_with(
                &[note(0.0, 1.0, 69, Part::Other)],
                0.1,
                (RATE as f64 * 0.8) as usize,
                &Voices::uniform(wave, 1.0),
            );
            let crossings = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
            // 440 Hz over 0.8 s = 352 rising zero crossings
            assert!(
                (crossings as i32 - 352).abs() <= 1,
                "{wave:?}: crossings = {crossings}"
            );
        }
    }

    #[test]
    fn square_and_saw_have_the_expected_shape() {
        // Away from the jumps a square sits at +-level and a sawtooth ramps up.
        let square = render_with(
            &[note(0.0, 1.0, 45, Part::Other)],
            0.5,
            480,
            &Voices::uniform(Wave::Square, 1.0),
        );
        let level = (NOTE_AMPLITUDE * Wave::Square.level()).tanh() as f32;
        assert!(square.iter().all(|s| s.abs() <= level * 1.05), "overshoot");
        assert!(
            square.iter().filter(|s| s.abs() > level * 0.95).count() > 400,
            "mostly flat"
        );
        let saw = render_with(
            &[note(0.0, 1.0, 45, Part::Other)],
            0.5,
            200,
            &Voices::uniform(Wave::Saw, 1.0),
        );
        let rising = saw.windows(2).filter(|w| w[1] > w[0]).count();
        assert!(rising > 190, "ramps up between jumps");
    }

    #[test]
    fn silent_before_the_onset_and_after_the_release() {
        let notes = [note(1.0, 1.5, 60, Part::Other)];
        assert!(render(&notes, 0.0, 4800).iter().all(|&s| s == 0.0));
        assert!(render(&notes, 1.5 + RELEASE_S + 0.001, 4800)
            .iter()
            .all(|&s| s == 0.0));
        // starts from silence and ends back at it: no click
        let attack = render(&notes, 1.0, 8);
        assert!(attack[0].abs() < 1e-6);
        let tail = render(&notes, 1.5 + RELEASE_S - 0.0001, 4);
        assert!(tail.iter().all(|s| s.abs() < 0.01), "{tail:?}");
    }

    #[test]
    fn block_boundaries_do_not_change_the_waveform() {
        let notes = [
            note(0.0, 0.9, 57, Part::Other),
            note(0.2, 0.7, 64, Part::Vocal),
        ];
        let mut voices = Voices::uniform(Wave::Saw, 1.0);
        voices.waves[Part::Vocal as usize] = Wave::Square;
        let whole = render_with(&notes, 0.3, 4000, &voices);
        let first = render_with(&notes, 0.3, 1700, &voices);
        let second = render_with(&notes, 0.3 + 1700.0 / RATE as f64, 2300, &voices);
        for (i, w) in whole.iter().enumerate() {
            let s = if i < 1700 { first[i] } else { second[i - 1700] };
            assert!((w - s).abs() < 1e-6, "sample {i}: {w} vs {s}");
        }
    }

    #[test]
    fn silent_parts_are_skipped_and_waves_are_per_part() {
        let notes = [note(0.0, 1.0, 60, Part::Vocal)];
        let mut voices = Voices::uniform(Wave::Sine, 1.0);
        voices.gains[Part::Vocal as usize] = 0.0;
        assert!(render_with(&notes, 0.1, 480, &voices)
            .iter()
            .all(|&s| s == 0.0));
        // another part's wave doesn't change this one
        let mut other = Voices::uniform(Wave::Sine, 1.0);
        other.waves[Part::Bass as usize] = Wave::Square;
        assert_eq!(
            render_with(&notes, 0.1, 480, &other),
            render(&notes, 0.1, 480)
        );
    }

    #[test]
    fn a_gain_change_ramps_across_the_block() {
        let notes = [note(0.0, 1.0, 60, Part::Other)];
        let voices = Voices::uniform(Wave::Square, 1.0);
        let mut out = vec![0.0; 480];
        render_notes(&notes, 0.5, RATE, &voices, &[0.0; PARTS], &mut out);
        assert!(out[0].abs() < 0.001, "starts near silence: {}", out[0]);
        assert!(out[470].abs() > 0.03, "ends at full level: {}", out[470]);
    }

    #[test]
    fn dense_passages_stay_within_range() {
        let notes: Vec<SynthNote> = (0..120)
            .map(|i| note(0.0, 1.0, 30 + (i % 60) as u8, Part::Other))
            .collect();
        for wave in [Wave::Sine, Wave::Square, Wave::Saw] {
            let out = render_with(&notes, 0.5, 4800, &Voices::uniform(wave, 1.0));
            assert!(out.iter().all(|s| s.abs() < 1.0));
        }
    }
}
