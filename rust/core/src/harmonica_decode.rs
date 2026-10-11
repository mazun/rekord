//! Streaming port of harmonica-repro's `harmonica/decode.py::decode` --
//! turns per-frame onset/sustain probabilities into notes. The offset head
//! is unused, exactly as in the reference.
//!
//! `decode()` there is a whole-array, look-both-ways function: per pitch, a
//! frame `t` is an onset candidate iff `on[t] > on[t-1]`, `on[t] >= on[t+1]`
//! and `on[t] > th_on` (a local max, ties broken toward the earlier frame),
//! and a candidate's note ends at the earlier of the next candidate's own
//! frame or the first later frame where `sustain < th_sus`. Streaming this
//! only needs one frame of lookahead to decide an onset candidate (frame
//! `u-1` is decided once frame `u` arrives, using `on[u-2]`, `on[u-1]`,
//! `on[u]`), and the sustain-close condition is checked against the newest
//! frame every time it arrives -- so each note is emitted exactly once, the
//! moment it's known to have ended, and never revised. See
//! `StreamingDecoder::push`/`finish` for the exact per-frame algorithm.

use ndarray::ArrayView2;

use crate::harmonica_model::{Part, MIN_MIDI, N_PITCH};

/// (start_s, end_s, midi pitch, velocity, part) -- the flat tuple form used
/// by `NoteTranscriber::note_events` for the live in-progress note list.
pub type NoteEvent = (f32, f32, i32, f32, Part);

/// A finished, never-to-be-revised note, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfirmedNote {
    pub start_s: f32,
    pub end_s: f32,
    pub pitch_midi: i32,
    /// Which output part of the model this note came from.
    pub part: Part,
    /// The onset probability at the note's onset frame (the local maximum's
    /// own value) -- not mean sustain, which saturates near 1 for almost
    /// every real note and gives no usable contrast (decided by the user).
    pub velocity: f32,
}

/// Same as `ConfirmedNote` but in frame units, as produced by
/// `StreamingDecoder` before `harmonica_stream` converts to seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodedNote {
    pub start_frame: usize,
    pub end_frame: usize,
    pub pitch_midi: i32,
    pub velocity: f32,
}

/// Per-pitch onset/sustain state carried across `push` calls.
#[derive(Clone, Copy)]
struct PitchState {
    /// on[u-2] relative to the next frame to be decided.
    prev_on: f32,
    /// on[u-1] relative to the next frame to be decided -- the pending
    /// onset value, not yet known to be a candidate or not.
    pending_on: f32,
    /// The currently open note for this pitch, if any: (start_frame, velocity).
    active: Option<(usize, f32)>,
}

impl Default for PitchState {
    fn default() -> Self {
        Self { prev_on: f32::NEG_INFINITY, pending_on: f32::NEG_INFINITY, active: None }
    }
}

/// Streaming per-pitch onset/sustain decoder. Frames must be pushed in
/// order (in any grouping -- see the module doc); `push`/`finish` return
/// each note exactly once, at the moment its end frame becomes known.
pub struct StreamingDecoder {
    th_on: f32,
    th_sus: f32,
    frames_seen: usize,
    finished: bool,
    pitches: [PitchState; N_PITCH],
}

impl StreamingDecoder {
    pub fn new(th_on: f32, th_sus: f32) -> Self {
        Self { th_on, th_sus, frames_seen: 0, finished: false, pitches: [PitchState::default(); N_PITCH] }
    }

    /// Frames whose note state is final: `frames_seen - 1` while streaming
    /// (the newest frame's own onset candidacy isn't decided until the next
    /// frame arrives), clamped at 0; `frames_seen` once `finish` has run.
    pub fn decided_frames(&self) -> usize {
        if self.finished {
            self.frames_seen
        } else {
            self.frames_seen.saturating_sub(1)
        }
    }

    /// Notes currently open (start_frame, midi pitch, velocity), one per
    /// sounding pitch.
    pub fn open_notes(&self) -> Vec<(usize, i32, f32)> {
        self.pitches.iter().enumerate().filter_map(|(p, st)| st.active.map(|(start, vel)| (start, MIN_MIDI + p as i32, vel))).collect()
    }

    /// Decides whether the pending frame (`u - 1`, values in `prev_on`/
    /// `pending_on`) is an onset candidate for pitch `p`, given `next` =
    /// on[u] (or -inf when called from `finish`, where there is no next
    /// frame). If it is, closes any note already active for this pitch at
    /// end frame `u - 1` and opens a new one there. No-op at `u == 0`: frame
    /// -1 doesn't exist, so there is nothing pending yet.
    fn decide_pending(&mut self, p: usize, u: usize, next: f32, out: &mut Vec<DecodedNote>) {
        if u == 0 {
            return;
        }
        let st = &mut self.pitches[p];
        let (prev, v) = (st.prev_on, st.pending_on);
        let is_candidate = v > prev && v >= next && v > self.th_on;
        if is_candidate {
            if let Some((start, vel)) = st.active.take() {
                out.push(DecodedNote { start_frame: start, end_frame: u - 1, pitch_midi: MIN_MIDI + p as i32, velocity: vel });
            }
            st.active = Some((u - 1, v));
        }
    }

    /// Feeds consecutive frames (any number of rows, `[n, N_PITCH]`).
    /// Returns notes that ended while processing them, sorted by
    /// (start_frame, pitch).
    pub fn push(&mut self, onset: ArrayView2<f32>, sustain: ArrayView2<f32>) -> Vec<DecodedNote> {
        assert!(!self.finished, "StreamingDecoder::push called after finish");
        assert_eq!(onset.dim(), sustain.dim(), "onset/sustain shape mismatch");
        assert_eq!(onset.ncols(), N_PITCH, "expected N_PITCH columns");

        let mut out = Vec::new();
        for r in 0..onset.nrows() {
            let u = self.frames_seen;
            for p in 0..N_PITCH {
                let on_u = onset[[r, p]];
                let sus_u = sustain[[r, p]];

                // (a) decide the pending frame u-1 using next = on[u].
                self.decide_pending(p, u, on_u, &mut out);

                // (b) on_u becomes the new pending value.
                let st = &mut self.pitches[p];
                st.prev_on = st.pending_on;
                st.pending_on = on_u;

                // (c) close an active note (start < u, always true here --
                // a note can only be active starting at some frame <= u-1)
                // the first time sustain drops below threshold.
                if let Some((start, vel)) = st.active {
                    if start < u && sus_u < self.th_sus {
                        st.active = None;
                        out.push(DecodedNote { start_frame: start, end_frame: u, pitch_midi: MIN_MIDI + p as i32, velocity: vel });
                    }
                }
            }
            self.frames_seen = u + 1;
        }
        out.sort_by_key(|n| (n.start_frame, n.pitch_midi));
        out
    }

    /// Ends the stream: decides the last pending frame (with no future
    /// frame to look at) and closes every still-open note at
    /// `T = frames_seen`. No `push` may follow.
    pub fn finish(&mut self) -> Vec<DecodedNote> {
        assert!(!self.finished, "StreamingDecoder::finish called twice");
        let mut out = Vec::new();
        let u = self.frames_seen;
        for p in 0..N_PITCH {
            self.decide_pending(p, u, f32::NEG_INFINITY, &mut out);
        }
        let t = self.frames_seen;
        for (p, st) in self.pitches.iter_mut().enumerate() {
            if let Some((start, vel)) = st.active.take() {
                out.push(DecodedNote { start_frame: start, end_frame: t, pitch_midi: MIN_MIDI + p as i32, velocity: vel });
            }
        }
        self.finished = true;
        out.sort_by_key(|n| (n.start_frame, n.pitch_midi));
        out
    }
}

#[cfg(test)]
mod tests {
    use ndarray::Array2;

    use super::*;

    const TH_ON: f32 = 0.5;
    const TH_SUS: f32 = 0.5;

    /// Builds ([n, N_PITCH]) onset/sustain arrays with pitch 0 set to the
    /// given per-frame values and every other pitch at 0.0 (always below
    /// any sane threshold, so it never produces spurious candidates).
    fn col0(onset_vals: &[f32], sustain_vals: &[f32]) -> (Array2<f32>, Array2<f32>) {
        assert_eq!(onset_vals.len(), sustain_vals.len());
        let n = onset_vals.len();
        let mut onset = Array2::zeros((n, N_PITCH));
        let mut sustain = Array2::zeros((n, N_PITCH));
        for i in 0..n {
            onset[[i, 0]] = onset_vals[i];
            sustain[[i, 0]] = sustain_vals[i];
        }
        (onset, sustain)
    }

    #[test]
    fn harmonica_decode_candidate_decided_on_next_push() {
        // Frame 1's onset (0.9) is the last row of push 1; it can't be
        // decided as a candidate until push 2 supplies frame 2.
        let (on, sus) = col0(&[0.1, 0.9], &[1.0, 1.0]);
        let mut d = StreamingDecoder::new(TH_ON, TH_SUS);
        assert_eq!(d.push(on.view(), sus.view()), vec![]);

        let (on2, sus2) = col0(&[0.1], &[1.0]);
        assert_eq!(d.push(on2.view(), sus2.view()), vec![]); // opened, not yet closed

        let notes = d.finish();
        assert_eq!(notes, vec![DecodedNote { start_frame: 1, end_frame: 3, pitch_midi: MIN_MIDI, velocity: 0.9 }]);
    }

    #[test]
    fn harmonica_decode_candidate_at_frame_zero() {
        // prev = on[-1] = -inf must not block frame 0 from being a candidate.
        let (on, sus) = col0(&[0.9, 0.1], &[1.0, 1.0]);
        let mut d = StreamingDecoder::new(TH_ON, TH_SUS);
        assert_eq!(d.push(on.view(), sus.view()), vec![]);
        let notes = d.finish();
        assert_eq!(notes, vec![DecodedNote { start_frame: 0, end_frame: 2, pitch_midi: MIN_MIDI, velocity: 0.9 }]);
    }

    #[test]
    fn harmonica_decode_plateau_prefers_earlier_frame() {
        // Equal adjacent onset values: `>` on the left, `>=` on the right,
        // so only the earlier of the tied pair is a candidate.
        let (on, sus) = col0(&[0.1, 0.9, 0.9, 0.1], &[1.0, 1.0, 1.0, 1.0]);
        let mut d = StreamingDecoder::new(TH_ON, TH_SUS);
        let mut notes = d.push(on.view(), sus.view());
        notes.extend(d.finish());
        assert_eq!(notes, vec![DecodedNote { start_frame: 1, end_frame: 4, pitch_midi: MIN_MIDI, velocity: 0.9 }]);
    }

    #[test]
    fn harmonica_decode_sustain_drop_right_after_onset() {
        // Sustain falls below threshold on the very next frame: a valid
        // 1-frame note (the algorithm's stated minimum length).
        let (on, sus) = col0(&[0.1, 0.9, 0.1], &[1.0, 1.0, 0.1]);
        let mut d = StreamingDecoder::new(TH_ON, TH_SUS);
        let mut notes = d.push(on.view(), sus.view());
        notes.extend(d.finish());
        assert_eq!(notes, vec![DecodedNote { start_frame: 1, end_frame: 2, pitch_midi: MIN_MIDI, velocity: 0.9 }]);
    }

    #[test]
    fn harmonica_decode_reonset_ends_previous_note() {
        // Sustain stays high throughout, so the first note is only ended by
        // the second onset candidate, at the new candidate's own frame.
        let (on, sus) = col0(&[0.1, 0.9, 0.1, 0.9, 0.1], &[1.0; 5]);
        let mut d = StreamingDecoder::new(TH_ON, TH_SUS);
        let mut notes = d.push(on.view(), sus.view());
        notes.extend(d.finish());
        assert_eq!(
            notes,
            vec![
                DecodedNote { start_frame: 1, end_frame: 3, pitch_midi: MIN_MIDI, velocity: 0.9 },
                DecodedNote { start_frame: 3, end_frame: 5, pitch_midi: MIN_MIDI, velocity: 0.9 },
            ]
        );
    }

    #[test]
    fn harmonica_decode_open_note_ends_at_finish() {
        let (on, sus) = col0(&[0.1, 0.9, 0.1, 0.1], &[1.0; 4]);
        let mut d = StreamingDecoder::new(TH_ON, TH_SUS);
        assert_eq!(d.push(on.view(), sus.view()), vec![]);
        let notes = d.finish();
        assert_eq!(notes, vec![DecodedNote { start_frame: 1, end_frame: 4, pitch_midi: MIN_MIDI, velocity: 0.9 }]);
    }

    #[test]
    fn harmonica_decode_splitting_invariance() {
        // A single pitch's onset trace with three candidates (including a
        // 2-long plateau) and no sustain-based closes, so all closes come
        // from re-onset or finish. Splitting this across push() calls in
        // different ways -- including splitting the plateau itself across a
        // push boundary -- must not change the result.
        let onset = vec![0.1, 0.6, 0.6, 0.2, 0.05, 0.9, 0.9, 0.9, 0.1, 0.7, 0.1];
        let sustain = vec![1.0f32; onset.len()];
        let expected = vec![
            DecodedNote { start_frame: 1, end_frame: 5, pitch_midi: MIN_MIDI, velocity: 0.6 },
            DecodedNote { start_frame: 5, end_frame: 9, pitch_midi: MIN_MIDI, velocity: 0.9 },
            DecodedNote { start_frame: 9, end_frame: 11, pitch_midi: MIN_MIDI, velocity: 0.7 },
        ];

        let run = |splits: &[usize]| {
            assert_eq!(splits.iter().sum::<usize>(), onset.len());
            let mut d = StreamingDecoder::new(TH_ON, TH_SUS);
            let mut notes = Vec::new();
            let mut i = 0;
            for &n in splits {
                let (on, sus) = col0(&onset[i..i + n], &sustain[i..i + n]);
                notes.extend(d.push(on.view(), sus.view()));
                i += n;
            }
            notes.extend(d.finish());
            notes
        };

        assert_eq!(run(&[11]), expected);
        assert_eq!(run(&[1; 11]), expected);
        // Splits the plateau (frames 5,6) itself across a push boundary.
        assert_eq!(run(&[6, 5]), expected);
        assert_eq!(run(&[3, 2, 6]), expected);
    }
}
