//! Port of basic_pitch.note_creation: converts raw model output (note/onset
//! activation matrices) into note events. Faithfully mirrors
//! output_to_notes_polyphonic / get_infered_onsets / model_frames_to_time
//! from the basic-pitch Python package (Apache 2.0) -- min/max frequency
//! constraints and pitch bends are dropped since we never use them.

use ndarray::{s, Array2};

const MIDI_OFFSET: i32 = 21;
const MAX_FREQ_IDX: usize = 87;

/// (start_time_s, end_time_s, midi_pitch, amplitude)
pub type NoteEvent = (f32, f32, i32, f32);

fn get_infered_onsets(onsets: &Array2<f32>, frames: &Array2<f32>, n_diff: usize) -> Array2<f32> {
    let (t, f) = frames.dim();
    let mut diffs: Vec<Array2<f32>> = Vec::with_capacity(n_diff);
    for n in 1..=n_diff {
        let mut appended = Array2::<f32>::zeros((t + n, f));
        appended.slice_mut(s![n.., ..]).assign(frames);
        let a = appended.slice(s![n.., ..]).to_owned();
        let b = appended.slice(s![..t, ..]).to_owned();
        diffs.push(&a - &b);
    }
    let mut frame_diff = diffs[0].clone();
    for d in &diffs[1..] {
        frame_diff.zip_mut_with(d, |x, &y| {
            if y < *x {
                *x = y;
            }
        });
    }
    frame_diff.mapv_inplace(|v| v.max(0.0));
    frame_diff.slice_mut(s![..n_diff.min(t), ..]).fill(0.0);

    let onsets_max = onsets.iter().cloned().fold(f32::MIN, f32::max);
    let diff_max = frame_diff.iter().cloned().fold(f32::MIN, f32::max);
    if diff_max > 0.0 {
        frame_diff.mapv_inplace(|v| onsets_max * v / diff_max);
    }

    let mut result = onsets.clone();
    result.zip_mut_with(&frame_diff, |x, &y| {
        if y > *x {
            *x = y;
        }
    });
    result
}

/// scipy.signal.argrelmax(x, axis=0), default order=1: strict local maxima
/// along the time axis, independently per frequency column. Edge frames
/// can never be a local max (no both-sided neighbor to compare against).
fn argrelmax_axis0(x: &Array2<f32>) -> Vec<(usize, usize)> {
    let (t, f) = x.dim();
    let mut peaks = Vec::new();
    if t < 3 {
        return peaks;
    }
    for col in 0..f {
        for row in 1..t - 1 {
            let v = x[[row, col]];
            if v > x[[row - 1, col]] && v > x[[row + 1, col]] {
                peaks.push((row, col));
            }
        }
    }
    peaks
}

#[allow(clippy::too_many_arguments)]
fn output_to_notes_polyphonic(
    frames: &Array2<f32>,
    onsets: &Array2<f32>,
    onset_thresh: f32,
    frame_thresh: f32,
    min_note_len: i64,
    infer_onsets: bool,
    melodia_trick: bool,
    energy_tol: i64,
) -> Vec<(i64, i64, i32, f32)> {
    let (n_frames, n_freqs) = frames.dim();
    let onsets = if infer_onsets { get_infered_onsets(onsets, frames, 2) } else { onsets.clone() };

    let mut peak_thresh_mat = Array2::<f32>::zeros((n_frames, n_freqs));
    for (row, col) in argrelmax_axis0(&onsets) {
        peak_thresh_mat[[row, col]] = onsets[[row, col]];
    }

    let mut onset_pairs: Vec<(usize, usize)> = Vec::new();
    for row in 0..n_frames {
        for col in 0..n_freqs {
            if peak_thresh_mat[[row, col]] >= onset_thresh {
                onset_pairs.push((row, col));
            }
        }
    }
    onset_pairs.reverse(); // process backwards in time, matching Python's [::-1]

    let mut remaining_energy = frames.clone();
    let mut note_events: Vec<(i64, i64, i32, f32)> = Vec::new();

    for (note_start_idx, freq_idx) in onset_pairs {
        if note_start_idx >= n_frames - 1 {
            continue;
        }
        let mut i = note_start_idx as i64 + 1;
        let mut k = 0i64;
        while i < n_frames as i64 - 1 && k < energy_tol {
            if remaining_energy[[i as usize, freq_idx]] < frame_thresh {
                k += 1;
            } else {
                k = 0;
            }
            i += 1;
        }
        i -= k;

        if i - note_start_idx as i64 <= min_note_len {
            continue;
        }

        let (start, end) = (note_start_idx as i64, i);
        for row in start..end {
            remaining_energy[[row as usize, freq_idx]] = 0.0;
            if freq_idx < MAX_FREQ_IDX {
                remaining_energy[[row as usize, freq_idx + 1]] = 0.0;
            }
            if freq_idx > 0 {
                remaining_energy[[row as usize, freq_idx - 1]] = 0.0;
            }
        }
        let amplitude: f32 =
            (start..end).map(|row| frames[[row as usize, freq_idx]]).sum::<f32>() / (end - start) as f32;
        note_events.push((start, end, freq_idx as i32 + MIDI_OFFSET, amplitude));
    }

    if melodia_trick {
        loop {
            let mut max_val = f32::MIN;
            let mut max_pos = (0usize, 0usize);
            for row in 0..n_frames {
                for col in 0..n_freqs {
                    let v = remaining_energy[[row, col]];
                    if v > max_val {
                        max_val = v;
                        max_pos = (row, col);
                    }
                }
            }
            if max_val <= frame_thresh {
                break;
            }
            let (i_mid, freq_idx) = max_pos;
            remaining_energy[[i_mid, freq_idx]] = 0.0;

            let mut i = i_mid as i64 + 1;
            let mut k = 0i64;
            while i < n_frames as i64 - 1 && k < energy_tol {
                if remaining_energy[[i as usize, freq_idx]] < frame_thresh {
                    k += 1;
                } else {
                    k = 0;
                }
                remaining_energy[[i as usize, freq_idx]] = 0.0;
                if freq_idx < MAX_FREQ_IDX {
                    remaining_energy[[i as usize, freq_idx + 1]] = 0.0;
                }
                if freq_idx > 0 {
                    remaining_energy[[i as usize, freq_idx - 1]] = 0.0;
                }
                i += 1;
            }
            let i_end = i - 1 - k;

            let mut i2 = i_mid as i64 - 1;
            let mut k2 = 0i64;
            while i2 > 0 && k2 < energy_tol {
                if remaining_energy[[i2 as usize, freq_idx]] < frame_thresh {
                    k2 += 1;
                } else {
                    k2 = 0;
                }
                remaining_energy[[i2 as usize, freq_idx]] = 0.0;
                if freq_idx < MAX_FREQ_IDX {
                    remaining_energy[[i2 as usize, freq_idx + 1]] = 0.0;
                }
                if freq_idx > 0 {
                    remaining_energy[[i2 as usize, freq_idx - 1]] = 0.0;
                }
                i2 -= 1;
            }
            let i_start = i2 + 1 + k2;

            if i_end - i_start <= min_note_len {
                continue;
            }
            let amplitude: f32 = (i_start..i_end)
                .map(|row| frames[[row as usize, freq_idx]])
                .sum::<f32>()
                / (i_end - i_start) as f32;
            note_events.push((i_start, i_end, freq_idx as i32 + MIDI_OFFSET, amplitude));
        }
    }

    note_events
}

/// `times_s[i]` must give the absolute time in seconds of `frames`/`onsets`
/// row `i` -- the caller (the streaming extractor, which knows each row's
/// true originating window and sample offset) is responsible for this
/// mapping; this module only works in frame-index space.
pub fn model_output_to_notes(
    frames: &Array2<f32>,
    onsets: &Array2<f32>,
    onset_thresh: f32,
    frame_thresh: f32,
    min_note_len_frames: i64,
    times_s: &[f32],
) -> Vec<NoteEvent> {
    let raw = output_to_notes_polyphonic(
        frames,
        onsets,
        onset_thresh,
        frame_thresh,
        min_note_len_frames,
        true,  // infer_onsets
        true,  // melodia_trick
        11,    // energy_tol
    );
    raw.into_iter()
        .map(|(start, end, pitch, amp)| (times_s[start as usize], times_s[end as usize], pitch, amp))
        .collect()
}
