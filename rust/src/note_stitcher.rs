//! Turns repeated note-extraction passes into a stable stream of
//! "confirmed" note events, holding back notes near the live edge since
//! more audio could still revise them. Mirrors chord_poc/note_stitcher.py.

use std::collections::HashSet;

use crate::note_creation::NoteEvent;

const CONFIRM_MARGIN_S: f32 = 0.5;

pub struct ConfirmedNote {
    pub start_s: f32,
    pub end_s: f32,
    pub pitch_midi: i32,
}

pub struct NoteStitcher {
    confirm_margin_s: f32,
    emitted: HashSet<(i64, i32)>,
}

impl NoteStitcher {
    pub fn new() -> Self {
        Self { confirm_margin_s: CONFIRM_MARGIN_S, emitted: HashSet::new() }
    }

    pub fn update(&mut self, buffer_duration_s: f32, note_events: &[NoteEvent]) -> Vec<ConfirmedNote> {
        let confirm_before = buffer_duration_s - self.confirm_margin_s;
        let mut newly_confirmed = Vec::new();
        for &(start_s, end_s, pitch_midi, _amplitude) in note_events {
            if end_s > confirm_before {
                continue;
            }
            let key = ((start_s * 100.0).round() as i64, pitch_midi);
            if self.emitted.contains(&key) {
                continue;
            }
            self.emitted.insert(key);
            newly_confirmed.push(ConfirmedNote { start_s, end_s, pitch_midi });
        }
        newly_confirmed.sort_by(|a, b| a.start_s.partial_cmp(&b.start_s).unwrap());
        newly_confirmed
    }
}
