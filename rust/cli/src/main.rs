use anyhow::{bail, Context, Result};

use chord_core::note_creation::NoteEvent;
use chord_core::note_stitcher::NoteStitcher;
use chord_core::onnx_model::OnnxNoteModel;
use chord_core::pipeline::{self, midi_to_name, PipelineEvent};
use chord_core::streaming_extractor::{
    StreamingNoteExtractor, AUDIO_SAMPLE_RATE, MAX_FRESH_EXTRACTOR_REAL_SAMPLES,
};
use chord_core::audio_io;

fn print_event(event: PipelineEvent) {
    match event {
        PipelineEvent::Started => {}
        PipelineEvent::NoteConfirmed { start_s, end_s, pitch_midi } => {
            println!("[{start_s:6.2}s -> {end_s:6.2}s] note {}", midi_to_name(pitch_midi));
        }
        PipelineEvent::ChordChange { t, label } => {
            println!("  t={t:6.2}s  chord: {label}");
        }
    }
}

fn print_confirmed_notes(stitcher: &mut NoteStitcher, covered: f32, note_events: &[NoteEvent]) {
    pipeline::emit_confirmed_notes(stitcher, covered, note_events, print_event);
}

fn report_new_chords(
    note_events: &[NoteEvent],
    prev_covered: f32,
    covered: f32,
    last_label: &mut Option<String>,
) {
    pipeline::report_new_chords(note_events, prev_covered, covered, last_label, print_event);
}

fn run_file(audio_path: &str, hop_seconds: f32) -> Result<()> {
    println!("Loading {audio_path} ...");
    let audio = audio_io::load_audio(audio_path)?;
    let duration = audio.len() as f32 / AUDIO_SAMPLE_RATE as f32;
    println!("Duration: {duration:.1}s, simulating stream with {hop_seconds:.2}s hops\n");

    let mut model = OnnxNoteModel::load().context("loading ONNX model")?;
    let mut stitcher = NoteStitcher::new();
    let mut last_chord_label: Option<String> = None;
    let mut last_covered = 0.0f32;

    let hop_samples = (hop_seconds * AUDIO_SAMPLE_RATE as f32).round() as usize;
    let main_final_notes = {
        let mut extractor = StreamingNoteExtractor::new(&mut model);
        let mut pos = 0usize;
        while pos < audio.len() {
            let end = (pos + hop_samples).min(audio.len());
            let completed = extractor.push(&audio[pos..end])?;
            pos = end;
            if completed > 0 {
                let note_events = extractor.note_events();
                let covered = extractor.covered_seconds();
                print_confirmed_notes(&mut stitcher, covered, &note_events);
                report_new_chords(&note_events, last_covered, covered, &mut last_chord_label);
                last_covered = covered;
            }
        }
        // Kept for the tail-reconciliation chord check below: the tail
        // pass re-analyzes its own short clip from scratch and otherwise
        // has no memory of notes the main loop already established.
        extractor.note_events()
    };

    // The tail shorter than one window (~1.6s) never completes on its own.
    // Re-run extraction on the real last bit of audio (no padding needed)
    // instead of zero-padding, which showed the model a mostly-synthetic-
    // silence window and suppressed real notes there (verified empirically
    // in the Python prototype). Cap what's fed in at
    // MAX_FRESH_EXTRACTOR_REAL_SAMPLES: a fresh extractor's buffer already
    // starts with some front-padding, so pushing a full window's worth of
    // real audio on top of that makes `push` complete a window on its own
    // before `finish` ever runs, leaving `finish` to pad a second, mostly-
    // synthetic window instead of the small bounded pad it's meant for.
    if last_covered < duration && !audio.is_empty() {
        let clip_start = audio.len().saturating_sub(MAX_FRESH_EXTRACTOR_REAL_SAMPLES);
        let clip_start_s = clip_start as f32 / AUDIO_SAMPLE_RATE as f32;
        let mut tail_extractor = StreamingNoteExtractor::new(&mut model);
        tail_extractor.push(&audio[clip_start..])?;
        tail_extractor.finish()?;
        let tail_notes: Vec<NoteEvent> = tail_extractor
            .note_events()
            .into_iter()
            .map(|(s, e, p, a)| (s + clip_start_s, e + clip_start_s, p, a))
            .collect();

        // Combine with what the main loop already knew, so a note the
        // tail's independent re-analysis fails to re-surface doesn't
        // silently vanish from the final chord/notes report.
        let mut combined_notes = main_final_notes;
        combined_notes.extend(tail_notes);

        print_confirmed_notes(&mut stitcher, duration, &combined_notes);
        report_new_chords(&combined_notes, last_covered, duration, &mut last_chord_label);
    }

    Ok(())
}

fn run_live(hop_seconds: f32, duration_s: Option<f32>) -> Result<()> {
    println!("Listening to default speaker output via loopback capture ...");
    match duration_s {
        Some(d) => println!("Will stop after {d:.1}s (Ctrl+C to stop earlier)\n"),
        None => println!("Press Ctrl+C to stop.\n"),
    }
    let never_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    pipeline::run_live_session(hop_seconds, duration_s, never_stop, print_event)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut audio_file: Option<String> = None;
    let mut hop = 0.5f32;
    let mut live = false;
    let mut duration: Option<f32> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--hop" => {
                i += 1;
                let value = args.get(i).context("--hop expects a number")?;
                hop = value.parse().context("--hop expects a number")?;
                if !(hop > 0.0) {
                    bail!("--hop must be greater than 0, got {hop}");
                }
            }
            "--live" => live = true,
            "--duration" => {
                i += 1;
                let value = args.get(i).context("--duration expects a number")?;
                let parsed: f32 = value.parse().context("--duration expects a number")?;
                if !(parsed > 0.0) {
                    bail!("--duration must be greater than 0, got {parsed}");
                }
                duration = Some(parsed);
            }
            other => audio_file = Some(other.to_string()),
        }
        i += 1;
    }

    if live {
        run_live(hop, duration)
    } else {
        let audio_file = audio_file.context("usage: chord_engine <audio_file> [--hop 0.25]")?;
        run_file(&audio_file, hop)
    }
}
