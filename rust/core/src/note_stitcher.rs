//! Turns repeated note-extraction passes into a stable stream of
//! "confirmed" note events, holding back notes near the live edge since
//! more audio could still revise them. Mirrors chord_poc/note_stitcher.py.

use std::collections::HashMap;

use crate::note_creation::NoteEvent;

pub(crate) const CONFIRM_MARGIN_S: f32 = 0.5;

// Two independently-windowed extractors (e.g. the main stream vs. the
// file-tail reconciliation pass) can re-detect the same physical note with
// a start_s that jitters by a few tens of ms depending on window
// alignment. Round to this coarser bucket so such near-duplicates collide
// in the dedup key instead of both being emitted.
const START_BUCKET_S: f32 = 0.1;
// end_s can legitimately shift between passes (get_infered_onsets rescales
// using the global max over the whole accumulated buffer, so appending
// audio can retroactively change how earlier frames are classified).
// Only treat a change bigger than this as a real correction worth
// re-emitting, not float noise between two runs of the same computation.
const END_CHANGE_EPSILON_S: f32 = 0.02;

pub struct ConfirmedNote {
    pub start_s: f32,
    pub end_s: f32,
    pub pitch_midi: i32,
    // The model's own per-pitch activation strength, averaged over the
    // note's frames (see note_creation::output_to_notes_polyphonic) --
    // already a natural [0, 1]-ish "how strongly did this specific pitch
    // sound" figure, and (unlike a broadband loudness measure over the
    // note's time span) correctly distinguishes notes within the same
    // chord that differ in strength.
    pub velocity: f32,
}

pub struct NoteStitcher {
    confirm_margin_s: f32,
    // key -> end_s last emitted for that key, so a later pass that
    // recomputes a meaningfully different end_s for the same note can be
    // re-emitted (with the corrected duration) instead of silently
    // dropped.
    emitted: HashMap<(i64, i32), f32>,
}

impl NoteStitcher {
    pub fn new() -> Self {
        Self { confirm_margin_s: CONFIRM_MARGIN_S, emitted: HashMap::new() }
    }

    pub fn update(&mut self, buffer_duration_s: f32, note_events: &[NoteEvent]) -> Vec<ConfirmedNote> {
        let confirm_before = buffer_duration_s - self.confirm_margin_s;
        let mut newly_confirmed = Vec::new();
        for &(start_s, end_s, pitch_midi, amplitude) in note_events {
            if end_s > confirm_before {
                continue;
            }
            let key = ((start_s / START_BUCKET_S).round() as i64, pitch_midi);
            if let Some(&prev_end_s) = self.emitted.get(&key) {
                if (prev_end_s - end_s).abs() < END_CHANGE_EPSILON_S {
                    continue;
                }
            }
            self.emitted.insert(key, end_s);
            newly_confirmed.push(ConfirmedNote { start_s, end_s, pitch_midi, velocity: amplitude });
        }
        newly_confirmed.sort_by(|a, b| a.start_s.partial_cmp(&b.start_s).unwrap());
        newly_confirmed
    }

    /// Emit everything still held back by the confirm margin, regardless
    /// of how close to the live edge it is. Call this once when a stream
    /// ends (e.g. live capture stopping) -- without it, whatever's within
    /// the last `confirm_margin_s` of the session is never confirmed.
    pub fn flush(&mut self, note_events: &[NoteEvent]) -> Vec<ConfirmedNote> {
        self.update(f32::INFINITY, note_events)
    }
}
