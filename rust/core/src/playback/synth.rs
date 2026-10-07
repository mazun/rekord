//! Sine-wave rendering of the transcribed notes, for playback.
//!
//! Pure: a block is rendered from the note list and a start time alone. The
//! phase is analytic (`sin(2 pi f (t - start))`, no phase accumulators), so
//! seeking to any time, or splitting a span into blocks differently, gives the
//! same waveform and needs no per-voice state.

use std::f64::consts::TAU;

use crate::harmonica_model::Part;

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

/// A note to sound.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
pub struct SynthNote {
    pub start_s: f64,
    pub end_s: f64,
    pub pitch: u8,
    pub velocity: f32,
    pub part: Part,
}

/// Which parts are sounding (the ones the piano roll doesn't hide).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartSet(u8);

impl PartSet {
    pub const ALL: PartSet = PartSet(0b1111);

    /// Every part except `hidden`.
    pub fn all_but(hidden: &[Part]) -> Self {
        PartSet(hidden.iter().fold(Self::ALL.0, |bits, &p| bits & !Self::bit(p)))
    }

    pub fn contains(self, part: Part) -> bool {
        self.0 & Self::bit(part) != 0
    }

    fn bit(part: Part) -> u8 {
        1 << (part as u8)
    }

    pub(super) fn to_bits(self) -> u8 {
        self.0
    }

    pub(super) fn from_bits(bits: u8) -> Self {
        PartSet(bits)
    }
}

fn envelope(note: &SynthNote, t: f64) -> f64 {
    if t < note.start_s {
        return 0.0;
    }
    let attack = ((t - note.start_s) / ATTACK_S).min(1.0);
    let release = if t <= note.end_s { 1.0 } else { (1.0 - (t - note.end_s) / RELEASE_S).max(0.0) };
    attack * release
}

/// Renders `out.len()` mono samples starting at recording time `t0_s`,
/// overwriting `out`. `notes` must be sorted by `start_s`. Notes of parts not in
/// `parts` are skipped. The sum is soft-clipped (`tanh`), so `|out| < 1`.
pub fn render_sines(notes: &[SynthNote], t0_s: f64, rate: u32, parts: PartSet, out: &mut [f32]) {
    out.fill(0.0);
    let n = out.len();
    let dt = 1.0 / rate as f64;
    let t_end = t0_s + n as f64 * dt;
    let candidates = notes.partition_point(|note| note.start_s < t_end);
    let mut voices = 0;
    for note in &notes[..candidates] {
        if note.end_s + RELEASE_S <= t0_s || !parts.contains(note.part) {
            continue;
        }
        voices += 1;
        if voices > MAX_VOICES {
            break;
        }
        let w = TAU * 440.0 * 2f64.powf((note.pitch as f64 - 69.0) / 12.0);
        let amp = NOTE_AMPLITUDE * (0.5 + 0.5 * note.velocity.clamp(0.0, 1.0) as f64);
        let from = (((note.start_s - t0_s) / dt).floor().max(0.0)) as usize;
        let to = ((((note.end_s + RELEASE_S) - t0_s) / dt).ceil().max(0.0) as usize).min(n);
        for (i, sample) in out.iter_mut().enumerate().take(to).skip(from) {
            let t = t0_s + i as f64 * dt;
            let env = envelope(note, t);
            if env > 0.0 {
                *sample += (amp * env * (w * (t - note.start_s)).sin()) as f32;
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
        SynthNote { start_s, end_s, pitch, velocity: 1.0, part }
    }

    fn render(notes: &[SynthNote], t0: f64, n: usize, mask: PartSet) -> Vec<f32> {
        let mut out = vec![0.0; n];
        render_sines(notes, t0, RATE, mask, &mut out);
        out
    }

    #[test]
    fn a440_has_the_right_frequency() {
        let out = render(&[note(0.0, 1.0, 69, Part::Other)], 0.1, (RATE as f64 * 0.8) as usize, PartSet::ALL);
        let crossings = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        // 440 Hz over 0.8 s = 352 rising zero crossings
        assert!((crossings as i32 - 352).abs() <= 1, "crossings = {crossings}");
    }

    #[test]
    fn silent_before_the_onset_and_after_the_release() {
        let notes = [note(1.0, 1.5, 60, Part::Other)];
        assert!(render(&notes, 0.0, 4800, PartSet::ALL).iter().all(|&s| s == 0.0));
        assert!(render(&notes, 1.5 + RELEASE_S + 0.001, 4800, PartSet::ALL).iter().all(|&s| s == 0.0));
        // starts from silence and ends back at it: no click
        let attack = render(&notes, 1.0, 8, PartSet::ALL);
        assert!(attack[0].abs() < 1e-6);
        let tail = render(&notes, 1.5 + RELEASE_S - 0.0001, 4, PartSet::ALL);
        assert!(tail.iter().all(|s| s.abs() < 0.01), "{tail:?}");
    }

    #[test]
    fn block_boundaries_do_not_change_the_waveform() {
        let notes = [note(0.0, 0.9, 57, Part::Other), note(0.2, 0.7, 64, Part::Vocal)];
        let whole = render(&notes, 0.3, 4000, PartSet::ALL);
        let first = render(&notes, 0.3, 1700, PartSet::ALL);
        let second = render(&notes, 0.3 + 1700.0 / RATE as f64, 2300, PartSet::ALL);
        for (i, w) in whole.iter().enumerate() {
            let s = if i < 1700 { first[i] } else { second[i - 1700] };
            assert!((w - s).abs() < 1e-6, "sample {i}: {w} vs {s}");
        }
    }

    #[test]
    fn hidden_parts_are_skipped() {
        let notes = [note(0.0, 1.0, 60, Part::Vocal)];
        assert!(render(&notes, 0.1, 480, PartSet::all_but(&[Part::Vocal])).iter().all(|&s| s == 0.0));
        assert!(render(&notes, 0.1, 480, PartSet::all_but(&[Part::Other, Part::Bass, Part::All])).iter().any(|&s| s != 0.0));
    }

    #[test]
    fn dense_passages_stay_within_range() {
        let notes: Vec<SynthNote> = (0..120).map(|i| note(0.0, 1.0, 30 + (i % 60) as u8, Part::Other)).collect();
        let out = render(&notes, 0.5, 4800, PartSet::ALL);
        assert!(out.iter().all(|s| s.abs() < 1.0));
    }
}
