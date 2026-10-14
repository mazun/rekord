//! MIDI out for a range selected on the roll: the notes are written as a Standard MIDI File (format 1:
//! one track per part that has notes, no tempo event) to a temp folder, and that file is dragged out
//! of the window onto a DAW's track or a folder.

use std::fs;
use std::path::PathBuf;

use rekord_core::drum_stream::Drum;
use rekord_core::okarina_model::Part;
use rekord_core::playback::{SynthDrumHit, SynthNote};
use serde::Deserialize;

const TICKS_PER_BEAT: u16 = 480;
/// General MIDI percussion channel (channel 10).
const DRUM_CHANNEL: u8 = 9;
/// How long a drum hit's note lasts (a 16th note); drums ignore it, but a DAW draws it.
const DRUM_NOTE_TICKS: u32 = (TICKS_PER_BEAT / 4) as u32;
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
    /// Vocal, Bass and Other on their own tracks and channels (1, 2, 3), and the drums on channel
    /// 10 in the General MIDI drum map.
    Split,
}

impl Layout {
    /// Whether the drums go in too (not when a single part was asked for).
    fn has_drums(self) -> bool {
        matches!(self, Layout::Split)
    }

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
    /// The drum hits in the selection, times relative to its start (written with the split layout
    /// only).
    drums: Vec<SynthDrumHit>,
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
    let bytes = smf_bytes(&range.notes, &range.drums, bpm, range.layout)
        .ok_or("no notes in the selection")?;

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
pub fn drag(range: MidiRange) -> Result<(), String> {
    let path = write_file(&range)?;
    let drums = if range.layout.has_drums() {
        &range.drums[..]
    } else {
        &[]
    };
    crate::os_drag::drag(&path, &thumbnail(&range.notes, drums))
}

#[cfg(not(windows))]
pub fn drag(_range: MidiRange) -> Result<(), String> {
    Err("dragging a file out is only supported on Windows".into())
}

#[cfg(windows)]
const THUMB_W: usize = 144;
#[cfg(windows)]
const THUMB_H: usize = 72;
/// Height of one drum row in the drag picture.
#[cfg(windows)]
const DRUM_ROW_PX: usize = 3;

/// The drag picture: the notes as bars on a small dark card (time across, pitch up), coloured by
/// part, with the drum hits as ticks in rows along the bottom (coloured as on the roll).
/// Premultiplied BGRA.
#[cfg(windows)]
fn thumbnail(notes: &[SynthNote], drums: &[SynthDrumHit]) -> crate::os_drag::DragPicture {
    const CARD: [u8; 3] = [0x26, 0x2b, 0x36]; // the roll's guide colour
    const BORDER: [u8; 3] = [96, 104, 124];
    const CARD_ALPHA: u32 = 230;
    let mut px = vec![0u8; THUMB_W * THUMB_H * 4];
    // Writes an opaque-ish RGB pixel with `alpha` (0-255), premultiplied.
    let mut put = |x: usize, y: usize, rgb: [u8; 3], alpha: u32| {
        let i = (y * THUMB_W + x) * 4;
        for (c, v) in [rgb[2], rgb[1], rgb[0]].into_iter().enumerate() {
            px[i + c] = (u32::from(v) * alpha / 255) as u8;
        }
        px[i + 3] = alpha as u8;
    };
    for y in 0..THUMB_H {
        for x in 0..THUMB_W {
            // Corner cut: clip a 3 px triangle off each corner.
            let dx = x.min(THUMB_W - 1 - x);
            let dy = y.min(THUMB_H - 1 - y);
            if dx + dy < 3 {
                continue;
            }
            let edge = dx == 0 || dy == 0;
            put(x, y, if edge { BORDER } else { CARD }, CARD_ALPHA);
        }
    }
    let end = notes
        .iter()
        .map(|n| n.end_s)
        .chain(drums.iter().map(|h| h.t))
        .fold(0.0, f64::max);
    let lo = notes.iter().map(|n| n.pitch).min().unwrap_or(0);
    let hi = notes.iter().map(|n| n.pitch).max().unwrap_or(0);
    if end > 0.0 {
        let (pad, bar_h) = (8.0, 3usize);
        let inner_w = THUMB_W as f64 - 2.0 * pad;
        // The drum rows (cymbal on top, as on the roll) take the bottom of the card.
        let drum_rows = if drums.is_empty() { 0 } else { 5 };
        let drum_h = drum_rows * DRUM_ROW_PX;
        let inner_h = THUMB_H as f64 - 2.0 * pad - bar_h as f64 - drum_h as f64;
        let drum_top = THUMB_H as f64 - pad - drum_h as f64;
        for h in drums {
            let (row, rgb): (usize, [u8; 3]) = match h.drum {
                Drum::Cymbal => (0, [0xc9, 0xa7, 0xff]),
                Drum::HiHat => (1, [0x7f, 0xe0, 0xe8]),
                Drum::Tom => (2, [0x8b, 0xe0, 0x8b]),
                Drum::Snare => (3, [0xff, 0xe2, 0x7a]),
                Drum::Kick => (4, [0xff, 0x7d, 0x6e]),
            };
            let x = (pad + h.t.max(0.0) / end * inner_w) as usize;
            let y = drum_top as usize + row * DRUM_ROW_PX;
            for yy in y..(y + DRUM_ROW_PX - 1).min(THUMB_H - 1) {
                for xx in x..(x + 2).min(THUMB_W - 1) {
                    put(xx, yy, rgb, 255);
                }
            }
        }
        let span = f64::from(hi - lo).max(1.0);
        for n in notes {
            // The roll's palettes (pianoroll.ts noteColors), shaded by velocity.
            let (min, max): ([u8; 3], [u8; 3]) = match n.part {
                Part::Vocal => ([0x1f, 0x3d, 0x2c], [0x6b, 0xe3, 0xa0]),
                Part::Bass => ([0x3d, 0x2c, 0x1f], [0xff, 0x9f, 0x5b]),
                Part::Other => ([0x2c, 0x35, 0x50], [0xae, 0xcb, 0xff]),
            };
            let v = f64::from(n.velocity.clamp(0.0, 1.0));
            let rgb = [0, 1, 2].map(|c| {
                (f64::from(min[c]) + (f64::from(max[c]) - f64::from(min[c])) * v).round() as u8
            });
            let x0 = (pad + n.start_s.max(0.0) / end * inner_w) as usize;
            let x1 = ((pad + n.end_s / end * inner_w) as usize).max(x0 + 2);
            let y0 = (pad + (1.0 - f64::from(n.pitch - lo) / span) * inner_h) as usize;
            for y in y0..(y0 + bar_h).min(THUMB_H - 1) {
                for x in x0..x1.min(THUMB_W - 1) {
                    put(x, y, rgb, 255);
                }
            }
        }
    }
    crate::os_drag::DragPicture {
        width: THUMB_W as u32,
        height: THUMB_H as u32,
        bgra: px,
    }
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

/// A note to write: on tick, wanted off tick, pitch, velocity.
type TrackNote = (u32, u32, u8, u8);

/// One track of `notes` (sorted by their on tick) on `channel`. A note ends where the next one of
/// its pitch starts, else the later note-on would be cut by the earlier note-off.
fn note_track(name: &str, channel: u8, notes: &[TrackNote]) -> Vec<u8> {
    // (tick, is_on, pitch, velocity)
    let mut ev: Vec<(u32, bool, u8, u8)> = Vec::new();
    for (i, &(on, wanted_off, pitch, vel)) in notes.iter().enumerate() {
        let next_on = notes[i + 1..].iter().find(|m| m.2 == pitch).map(|m| m.0);
        let off = wanted_off.min(next_on.unwrap_or(u32::MAX)).max(on + 1);
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
    track_chunk(&body)
}

/// The General MIDI percussion key a drum is written as.
fn gm_key(drum: Drum) -> u8 {
    match drum {
        Drum::Kick => 36,   // Bass Drum 1
        Drum::Snare => 38,  // Acoustic Snare
        Drum::Tom => 47,    // Low-Mid Tom
        Drum::HiHat => 42,  // Closed Hi-Hat
        Drum::Cymbal => 49, // Crash Cymbal 1
    }
}

/// The file for `layout`, or `None` when none of the notes (or, for the split layout, drum hits)
/// belongs on any of its tracks.
fn smf_bytes(
    notes: &[SynthNote],
    drums: &[SynthDrumHit],
    bpm: f64,
    layout: Layout,
) -> Option<Vec<u8>> {
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
        let items: Vec<TrackNote> = mine
            .iter()
            .map(|n| {
                let vel = (n.velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                (
                    to_ticks(n.start_s),
                    to_ticks(n.end_s),
                    n.pitch.min(127),
                    vel,
                )
            })
            .collect();
        tracks.push(note_track(name, channel, &items));
    }
    if layout.has_drums() && !drums.is_empty() {
        let mut sorted = drums.to_vec();
        sorted.sort_by(|a, b| a.t.total_cmp(&b.t));
        let items: Vec<TrackNote> = sorted
            .iter()
            .map(|h| {
                let on = to_ticks(h.t);
                let vel = ((0.4 + 0.6 * f64::from(h.strength.clamp(0.0, 1.0))) * 127.0).round();
                (on, on + DRUM_NOTE_TICKS, gm_key(h.drum), vel as u8)
            })
            .collect();
        tracks.push(note_track("Drums", DRUM_CHANNEL, &items));
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
        let b = smf_bytes(
            &[note(0.0, 0.5, 60, Part::Other)],
            &[],
            120.0,
            Layout::Other,
        )
        .unwrap();
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
        let tracks = |layout| {
            smf_bytes(&notes, &[], 120.0, layout).map(|b| u16::from_be_bytes([b[10], b[11]]))
        };
        assert_eq!(tracks(Layout::Vocal), Some(1));
        assert_eq!(tracks(Layout::Split), Some(3));
        assert_eq!(smf_bytes(&notes[..1], &[], 120.0, Layout::Bass), None);
        // Split puts Bass on channel 2 (note-on status 0x91).
        let b = smf_bytes(&notes, &[], 120.0, Layout::Split).unwrap();
        assert!(b.windows(3).any(|w| w == [0x91, 36, 64]));
    }

    fn hit(t: f64, drum: Drum) -> SynthDrumHit {
        SynthDrumHit {
            t,
            drum,
            strength: 1.0,
        }
    }

    #[test]
    fn drums_go_on_channel_10_in_the_gm_map_with_the_split_layout_only() {
        let notes = [note(0.0, 0.5, 60, Part::Other)];
        let drums = [
            hit(0.0, Drum::Kick),
            hit(0.5, Drum::Snare),
            hit(0.5, Drum::HiHat),
            hit(1.0, Drum::Tom),
            hit(1.0, Drum::Cymbal),
        ];
        let b = smf_bytes(&notes, &drums, 120.0, Layout::Split).unwrap();
        assert_eq!(u16::from_be_bytes([b[10], b[11]]), 2, "Other + Drums");
        for key in [36, 38, 42, 47, 49] {
            assert!(b.windows(3).any(|w| w == [0x99, key, 127]), "key {key}");
        }
        // a single part's file has no drums
        let part = smf_bytes(&notes, &drums, 120.0, Layout::Other).unwrap();
        assert_eq!(u16::from_be_bytes([part[10], part[11]]), 1);
        // drums alone are a file of their own
        let only = smf_bytes(&[], &drums, 120.0, Layout::Split).unwrap();
        assert_eq!(u16::from_be_bytes([only[10], only[11]]), 1);
        assert_eq!(smf_bytes(&[], &drums, 120.0, Layout::Other), None);
    }

    #[test]
    fn same_pitch_overlap_is_cut_at_the_next_start() {
        // Two overlapping C4s in one part: the first must end when the second starts.
        let b = smf_bytes(
            &[
                note(0.0, 1.0, 60, Part::Other),
                note(0.5, 1.0, 60, Part::Other),
            ],
            &[],
            120.0,
            Layout::Other,
        )
        .unwrap();
        let off_then_on = [0x83, 0x60, 0x80, 60, 0, 0x00, 0x90, 60, 64];
        assert!(b.windows(off_then_on.len()).any(|w| w == off_then_on));
    }
}
