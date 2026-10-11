//! MIDI out for a range selected on the roll: the notes are written as a Standard MIDI File (format 1:
//! one track per part that has notes, no tempo event) to a temp folder, and that file is dragged out
//! of the window onto a DAW's track or a folder.

use std::fs;
use std::path::PathBuf;

use rekord_core::okarina_model::Part;
use rekord_core::playback::SynthNote;
use serde::Deserialize;

const TICKS_PER_BEAT: u16 = 480;
const DEFAULT_BPM: f64 = 120.0;
// Files older than this many are removed when a new one is made.
const KEEP_FILES: usize = 20;

/// Which notes go into the file, and onto which tracks/channels.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// Only that part, on one track (channel 1).
    Vocal,
    Bass,
    Other,
    /// Vocal, Bass and Other on their own tracks and channels (1, 2, 3).
    Split,
}

impl Layout {
    /// The tracks to write: name, channel, and the part it takes.
    fn tracks(self) -> Vec<(&'static str, u8, Part)> {
        match self {
            Layout::Vocal => vec![("Vocal", 0, Part::Vocal)],
            Layout::Bass => vec![("Bass", 0, Part::Bass)],
            Layout::Other => vec![("Other", 0, Part::Other)],
            Layout::Split => vec![
                ("Vocal", 0, Part::Vocal),
                ("Bass", 1, Part::Bass),
                ("Other", 2, Part::Other),
            ],
        }
    }
}

#[derive(Deserialize)]
pub struct MidiRange {
    /// Notes already clipped to the selection, times relative to its start.
    notes: Vec<SynthNote>,
    /// The tempo at the selection (the beat tracker's), if it had one.
    bpm: Option<f64>,
    layout: Layout,
}

/// Writes the .mid file for `range` to the temp folder and returns its path.
fn write_file(range: &MidiRange) -> Result<PathBuf, String> {
    let bpm = range
        .bpm
        .filter(|b| b.is_finite() && *b >= 20.0 && *b <= 400.0)
        .unwrap_or(DEFAULT_BPM);
    let bytes = smf_bytes(&range.notes, bpm, range.layout).ok_or("no notes in the selection")?;

    let dir = std::env::temp_dir().join("ReKord midi");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    prune(&dir);
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let path = dir.join(format!("rekord_{unix_ms}.mid"));
    fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Starts a native drag of the .mid file out of the window (to drop onto a DAW's track or a folder).
/// On Windows this returns once the drag has ended.
#[cfg(windows)]
pub fn drag(window: &tauri::WebviewWindow, range: MidiRange) -> Result<(), String> {
    let path = write_file(&range)?;
    drag::start_drag(
        window,
        drag::DragItem::Files(vec![path]),
        // No drag picture of our own (undecodable bytes make the crate skip it): a custom one changed
        // nothing about what is shown while dragging, which is Windows' own file-drag look.
        drag::Image::Raw(Vec::new()),
        |_result, _cursor| {},
        drag::Options::default(),
    )
    .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn drag(_window: &tauri::WebviewWindow, _range: MidiRange) -> Result<(), String> {
    Err("dragging a file out is only supported on Windows".into())
}

fn prune(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "mid"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .collect();
    files.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    for (path, _) in files.into_iter().skip(KEEP_FILES) {
        let _ = fs::remove_file(path);
    }
}

fn push_vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 5];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = (v & 0x7f) as u8 | if i == buf.len() - 1 { 0 } else { 0x80 };
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[i..]);
}

fn track_chunk(events: &[u8]) -> Vec<u8> {
    let mut out = b"MTrk".to_vec();
    out.extend_from_slice(&(events.len() as u32).to_be_bytes());
    out.extend_from_slice(events);
    out
}

fn name_event(name: &str) -> Vec<u8> {
    let mut e = vec![0x00, 0xff, 0x03, name.len() as u8];
    e.extend_from_slice(name.as_bytes());
    e
}

/// The file for `layout`, or `None` when none of the notes belongs on any of its tracks.
fn smf_bytes(notes: &[SynthNote], bpm: f64, layout: Layout) -> Option<Vec<u8>> {
    let ticks_per_s = f64::from(TICKS_PER_BEAT) * bpm / 60.0;
    let to_ticks = |s: f64| (s.max(0.0) * ticks_per_s).round() as u32;

    let mut tracks: Vec<Vec<u8>> = Vec::new();

    // No tempo (or time signature) event is written: a DAW would ask whether to import it. The
    // times are still laid out in beats of `bpm`, so the notes sit on the grid once the project
    // runs at the song's tempo.
    for (name, channel, part) in layout.tracks() {
        let mut mine: Vec<&SynthNote> = notes.iter().filter(|n| n.part == part).collect();
        if mine.is_empty() {
            continue;
        }
        mine.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
        // (tick, is_on, pitch, velocity). A note ends where the next one of its pitch starts,
        // else the later note-on would be cut by the earlier note-off.
        let mut ev: Vec<(u32, bool, u8, u8)> = Vec::new();
        for (i, n) in mine.iter().enumerate() {
            let pitch = n.pitch.min(127);
            let on = to_ticks(n.start_s);
            let next_on = mine[i + 1..]
                .iter()
                .find(|m| m.pitch.min(127) == pitch)
                .map(|m| to_ticks(m.start_s));
            let off = to_ticks(n.end_s)
                .min(next_on.unwrap_or(u32::MAX))
                .max(on + 1);
            let vel = (n.velocity * 127.0).round().clamp(1.0, 127.0) as u8;
            ev.push((on, true, pitch, vel));
            ev.push((off, false, pitch, 0));
        }
        // Note-offs before note-ons at the same tick.
        ev.sort_by_key(|&(tick, is_on, pitch, _)| (tick, is_on, pitch));
        let mut body = name_event(name);
        let mut last = 0;
        for (tick, is_on, pitch, vel) in ev {
            push_vlq(&mut body, tick - last);
            last = tick;
            body.extend_from_slice(&[(if is_on { 0x90 } else { 0x80 }) | channel, pitch, vel]);
        }
        body.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
        tracks.push(track_chunk(&body));
    }
    if tracks.is_empty() {
        return None;
    }

    let mut out = b"MThd".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    out.extend_from_slice(&TICKS_PER_BEAT.to_be_bytes());
    for t in tracks {
        out.extend(t);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start_s: f64, end_s: f64, pitch: u8, part: Part) -> SynthNote {
        SynthNote {
            start_s,
            end_s,
            pitch,
            velocity: 0.5,
            part,
        }
    }

    #[test]
    fn vlq() {
        let mut v = Vec::new();
        push_vlq(&mut v, 0);
        push_vlq(&mut v, 0x7f);
        push_vlq(&mut v, 0x80);
        push_vlq(&mut v, 480);
        assert_eq!(v, [0x00, 0x7f, 0x81, 0x00, 0x83, 0x60]);
    }

    #[test]
    fn one_note_at_120_bpm() {
        // 0.5 s at 120 bpm is one beat = 480 ticks.
        let b = smf_bytes(&[note(0.0, 0.5, 60, Part::Other)], 120.0, Layout::Other).unwrap();
        assert_eq!(&b[..4], b"MThd");
        assert_eq!(u16::from_be_bytes([b[10], b[11]]), 1);
        assert!(
            !b.windows(3).any(|w| w == [0xff, 0x51, 0x03]),
            "no tempo event"
        );
        // ends with: note-off after 480 ticks (vlq 83 60), then end of track
        assert_eq!(
            &b[b.len() - 9..],
            [0x83, 0x60, 0x80, 60, 0, 0x00, 0xff, 0x2f, 0x00]
        );
    }

    #[test]
    fn layouts() {
        let notes = [
            note(0.0, 0.5, 72, Part::Vocal),
            note(0.0, 0.5, 36, Part::Bass),
            note(0.0, 0.5, 60, Part::Other),
        ];
        let tracks =
            |layout| smf_bytes(&notes, 120.0, layout).map(|b| u16::from_be_bytes([b[10], b[11]]));
        assert_eq!(tracks(Layout::Vocal), Some(1));
        assert_eq!(tracks(Layout::Split), Some(3));
        assert_eq!(smf_bytes(&notes[..1], 120.0, Layout::Bass), None);
        // Split puts Bass on channel 2 (note-on status 0x91).
        let b = smf_bytes(&notes, 120.0, Layout::Split).unwrap();
        assert!(b.windows(3).any(|w| w == [0x91, 36, 64]));
    }

    #[test]
    fn same_pitch_overlap_is_cut_at_the_next_start() {
        // Two overlapping C4s in one part: the first must end when the second starts.
        let b = smf_bytes(
            &[
                note(0.0, 1.0, 60, Part::Other),
                note(0.5, 1.0, 60, Part::Other),
            ],
            120.0,
            Layout::Other,
        )
        .unwrap();
        let off_then_on = [0x83, 0x60, 0x80, 60, 0, 0x00, 0x90, 60, 64];
        assert!(b.windows(off_then_on.len()).any(|w| w == off_then_on));
    }
}
