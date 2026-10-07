//! Rolling recording of the loopback capture: the last `RECORDING_HISTORY_S`
//! seconds of audio, stereo, at the capture device's native sample rate.
//!
//! The ring is indexed by *absolute frame number since the session began*, and
//! frame `n` is pipeline time `n / rate`. That is the same clock the note
//! pipeline runs on (it counts the audio it is handed), so a note's `start_s`
//! maps straight to a frame with no wall-clock bookkeeping: both are fed from
//! the same chunks on the capture thread (`live_capture.rs`), so a chunk lost
//! before that (the consumer falling behind) is missing from both and they stay
//! aligned. (A chunk the resampler then fails on is dropped for the pipeline
//! only; that is rare and shifts the notes by one hop.)

use std::sync::{Arc, Mutex};

use crate::live_capture::{CaptureFormat, CaptureTap};

/// How much audio is kept. Mirrors `HISTORY_S` in the piano roll's
/// `recording.ts`, which keeps the notes/chords for the same span.
pub const RECORDING_HISTORY_S: usize = 300;

/// What a finished (or running) recording covers, in pipeline seconds.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct RecordingInfo {
    pub sample_rate: u32,
    pub start_s: f64,
    pub end_s: f64,
}

/// i16 interleaved stereo ring (mono sources are duplicated into both
/// channels, sources with more than two channels keep the first two). i16
/// halves the memory against f32 (57 MB for 5 minutes at 48 kHz); the data
/// arrives at 16--24 bit effective resolution anyway. Grows lazily up to the
/// capacity, then overwrites the oldest frames.
pub struct StereoRing {
    data: Vec<i16>,
    cap_frames: usize,
    written: u64,
    rate: u32,
}

impl StereoRing {
    pub fn new(rate: u32, cap_frames: usize) -> Self {
        assert!(cap_frames > 0, "ring capacity must be positive");
        Self { data: Vec::new(), cap_frames, written: 0, rate }
    }

    /// First frame still held (older ones were overwritten).
    pub fn start_frame(&self) -> u64 {
        self.written.saturating_sub(self.cap_frames as u64)
    }

    /// One past the newest frame.
    pub fn end_frame(&self) -> u64 {
        self.written
    }

    pub fn info(&self) -> RecordingInfo {
        RecordingInfo {
            sample_rate: self.rate,
            start_s: self.start_frame() as f64 / self.rate as f64,
            end_s: self.end_frame() as f64 / self.rate as f64,
        }
    }

    /// Appends interleaved f32 frames of `channels` channels.
    pub fn push_f32(&mut self, interleaved: &[f32], channels: usize) {
        for frame in interleaved.chunks_exact(channels) {
            let l = to_i16(frame[0]);
            let r = if channels >= 2 { to_i16(frame[1]) } else { l };
            if self.data.len() < self.cap_frames * 2 {
                self.data.push(l);
                self.data.push(r);
            } else {
                let i = (self.written % self.cap_frames as u64) as usize * 2;
                self.data[i] = l;
                self.data[i + 1] = r;
            }
            self.written += 1;
        }
    }

    /// Fills `out` (interleaved stereo f32) with the frames starting at
    /// absolute frame `frame`. Frames before the start of the ring come out as
    /// silence; reading stops at the end of the recording. Returns how many
    /// frames of `out` were filled (the rest is left untouched).
    pub fn read_f32(&self, frame: u64, out: &mut [f32]) -> usize {
        let start = self.start_frame();
        let end = self.end_frame();
        let wanted = out.len() / 2;
        let mut n = 0;
        while n < wanted {
            let f = frame + n as u64;
            if f >= end {
                break;
            }
            if f < start {
                out[2 * n] = 0.0;
                out[2 * n + 1] = 0.0;
            } else {
                let i = (f % self.cap_frames as u64) as usize * 2;
                out[2 * n] = self.data[i] as f32 / 32768.0;
                out[2 * n + 1] = self.data[i + 1] as f32 / 32768.0;
            }
            n += 1;
        }
        n
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// The ring shared between the capture thread (writer) and playback (reader).
/// `None` until the first session starts. The two never run at the same time
/// (capture is stopped during playback), so the lock is uncontended.
pub type SharedRing = Arc<Mutex<Option<StereoRing>>>;

/// The `CaptureTap` that fills a `SharedRing`; a new session replaces the ring.
pub struct RingTap {
    ring: SharedRing,
    channels: usize,
}

impl RingTap {
    pub fn new(ring: SharedRing) -> Self {
        Self { ring, channels: 2 }
    }
}

impl CaptureTap for RingTap {
    fn begin(&mut self, format: CaptureFormat) {
        self.channels = format.channels;
        let cap = RECORDING_HISTORY_S * format.sample_rate as usize;
        *self.ring.lock().unwrap() = Some(StereoRing::new(format.sample_rate, cap));
    }

    fn chunk(&mut self, interleaved: &[f32]) {
        if let Some(ring) = self.ring.lock().unwrap().as_mut() {
            ring.push_f32(interleaved, self.channels);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(frames: &[(f32, f32)]) -> Vec<f32> {
        frames.iter().flat_map(|&(l, r)| [l, r]).collect()
    }

    #[test]
    fn grows_then_wraps_keeping_the_newest_frames() {
        let mut ring = StereoRing::new(100, 4);
        ring.push_f32(&stereo(&[(0.1, -0.1), (0.2, -0.2), (0.3, -0.3)]), 2);
        assert_eq!((ring.start_frame(), ring.end_frame()), (0, 3));
        ring.push_f32(&stereo(&[(0.4, -0.4), (0.5, -0.5), (0.6, -0.6)]), 2);
        assert_eq!((ring.start_frame(), ring.end_frame()), (2, 6));
        let mut out = [0.0f32; 8];
        assert_eq!(ring.read_f32(2, &mut out), 4);
        for (i, want) in [0.3f32, 0.4, 0.5, 0.6].iter().enumerate() {
            assert!((out[2 * i] - want).abs() < 1e-4, "left {i}: {}", out[2 * i]);
            assert!((out[2 * i + 1] + want).abs() < 1e-4, "right {i}: {}", out[2 * i + 1]);
        }
    }

    #[test]
    fn read_before_start_is_silent_and_past_end_is_short() {
        let mut ring = StereoRing::new(100, 2);
        ring.push_f32(&stereo(&[(0.5, 0.5), (0.6, 0.6), (0.7, 0.7)]), 2);
        // frames 0 was overwritten; the ring holds 1..3
        let mut out = [9.0f32; 8];
        assert_eq!(ring.read_f32(0, &mut out), 3);
        assert_eq!(&out[..2], &[0.0, 0.0]);
        assert!((out[2] - 0.6).abs() < 1e-4);
        assert_eq!(&out[6..], &[9.0, 9.0], "past the end is left untouched");
        assert_eq!(ring.read_f32(3, &mut out), 0);
    }

    #[test]
    fn mono_is_duplicated_and_extra_channels_dropped() {
        let mut ring = StereoRing::new(100, 8);
        ring.push_f32(&[0.5, -0.5], 1);
        ring.push_f32(&[0.25, 0.75, 1.0, 1.0, 1.0, 1.0], 6);
        let mut out = [0.0f32; 6];
        assert_eq!(ring.read_f32(0, &mut out), 3);
        assert!((out[0] - 0.5).abs() < 1e-4 && (out[1] - 0.5).abs() < 1e-4);
        assert!((out[2] + 0.5).abs() < 1e-4 && (out[3] + 0.5).abs() < 1e-4);
        assert!((out[4] - 0.25).abs() < 1e-4 && (out[5] - 0.75).abs() < 1e-4);
    }

    #[test]
    fn conversion_clamps_and_round_trips() {
        let mut ring = StereoRing::new(100, 4);
        ring.push_f32(&stereo(&[(2.0, -2.0), (0.123456, -0.654321)]), 2);
        let mut out = [0.0f32; 4];
        ring.read_f32(0, &mut out);
        assert!((out[0] - 32767.0 / 32768.0).abs() < 1e-6);
        assert!((out[1] + 32767.0 / 32768.0).abs() < 1e-6);
        assert!((out[2] - 0.123456).abs() < 1.0 / 16384.0);
        assert!((out[3] + 0.654321).abs() < 1.0 / 16384.0);
    }

    #[test]
    fn info_reports_seconds() {
        let mut ring = StereoRing::new(100, 200);
        ring.push_f32(&vec![0.0; 2 * 150], 2);
        let info = ring.info();
        assert_eq!(info.sample_rate, 100);
        assert_eq!(info.start_s, 0.0);
        assert_eq!(info.end_s, 1.5);
    }

    #[test]
    fn tap_replaces_the_ring_on_begin() {
        let shared: SharedRing = Arc::new(Mutex::new(None));
        let mut tap = RingTap::new(Arc::clone(&shared));
        tap.begin(CaptureFormat { sample_rate: 100, channels: 2 });
        tap.chunk(&[0.1, 0.1, 0.2, 0.2]);
        assert_eq!(shared.lock().unwrap().as_ref().unwrap().end_frame(), 2);
        tap.begin(CaptureFormat { sample_rate: 100, channels: 2 });
        assert_eq!(shared.lock().unwrap().as_ref().unwrap().end_frame(), 0);
    }
}
