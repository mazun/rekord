use anyhow::{bail, Context, Result};

use rekord_core::audio_io;
use rekord_core::harmonica_model::{self, Device, HarmonicaModel, ModelSize, Part};
use rekord_core::harmonica_stream::NoteTranscriber;
use rekord_core::key_estimator::KeyEstimator;
use rekord_core::melody_bass::LineTracker;
use rekord_core::pipeline::{
    self, midi_to_name, ChordStabilizer, NoteModelConfig, NoteModelInfo, PipelineEvent, CAPTURE_HOP_S,
    DEFAULT_FUTURE_FRAMES, FUTURE_FRAME_CHOICES,
};

fn device_name(device: Device) -> &'static str {
    match device {
        Device::DirectML => "DirectML",
        Device::Cpu => "CPU",
    }
}

fn print_model_info(note_model: NoteModelInfo) {
    println!(
        "Using {} on {} (look-ahead {} frames)\n",
        note_model.size,
        device_name(note_model.device),
        note_model.future_frames
    );
}

fn print_event(event: PipelineEvent) {
    match event {
        PipelineEvent::Started { total_latency_s, note_model } => {
            println!(
                "(model: {} on {}, look-ahead {} frames; confirm latency: {total_latency_s:.2}s)\n",
                note_model.size,
                device_name(note_model.device),
                note_model.future_frames
            );
        }
        PipelineEvent::NoteConfirmed { start_s, end_s, pitch_midi, velocity, part, melody, bass } => {
            let role = match (part, melody, bass) {
                (Part::Vocal, ..) => " [vocal]",
                (Part::Bass, ..) => " [bass part]",
                (Part::Other, ..) => " [other]",
                (Part::All, true, true) => " [melody+bass]",
                (Part::All, true, false) => " [melody]",
                (Part::All, false, true) => " [bass]",
                (Part::All, false, false) => "",
            };
            println!(
                "[{start_s:6.2}s -> {end_s:6.2}s] note {} (vel {velocity:.2}){role}",
                midi_to_name(pitch_midi)
            );
        }
        PipelineEvent::ChordChange { t, label, roman_numeral } => {
            let suffix = roman_numeral.map(|r| format!("  ({r})")).unwrap_or_default();
            println!("  t={t:6.2}s  chord: {label}{suffix}");
        }
        PipelineEvent::TensionsChanged { tensions } => {
            if !tensions.is_empty() {
                println!("    tensions: {}", tensions.join(", "));
            }
        }
        PipelineEvent::KeyChange { key_name } => {
            println!("  key: {key_name}");
        }
        // the CLI prints a note once, when it ends
        PipelineEvent::NoteStarted { .. } => {}
        // snapshots of a moving window; run_live prints each committed line once instead
        PipelineEvent::BeatLines { .. } => {}
    }
}

fn run_file(audio_path: &str, size: ModelSize, future_frames: usize, allow_directml: bool, hop_seconds: f32) -> Result<()> {
    println!("Loading {audio_path} ...");
    let audio = audio_io::load_audio(audio_path, harmonica_model::SAMPLE_RATE)?;
    let duration = audio.len() as f32 / harmonica_model::SAMPLE_RATE as f32;
    println!("Duration: {duration:.1}s, simulating stream with {hop_seconds:.2}s hops\n");

    let model = HarmonicaModel::load(size, allow_directml).context("loading Harmonica model")?;
    print_model_info(NoteModelInfo { size: model.size(), device: model.device(), future_frames });
    let mut transcriber = NoteTranscriber::new(model, future_frames);
    let mut key_estimator = KeyEstimator::new();
    let mut lines = LineTracker::new();
    let mut chord_display = ChordStabilizer::new();
    let mut last_key_name: Option<String> = None;
    let mut last_covered = 0.0f32;

    let hop_samples = (hop_seconds * harmonica_model::SAMPLE_RATE as f32).round() as usize;
    let mut pos = 0usize;
    while pos < audio.len() {
        let end = (pos + hop_samples).min(audio.len());
        let blocks_before = transcriber.blocks_done();
        let notes = transcriber.push(&audio[pos..end])?;
        pos = end;
        if transcriber.blocks_done() != blocks_before {
            let note_events = transcriber.note_events();
            let covered = transcriber.covered_seconds();
            pipeline::emit_confirmed_notes(notes, &note_events, &mut key_estimator, &mut lines, print_event);
            pipeline::report_new_chords(
                &note_events,
                last_covered,
                covered,
                &mut chord_display,
                &mut key_estimator,
                &pipeline::KeyMode::Auto,
                None, // file mode has no beat tracker to snap chord onsets to
                &mut last_key_name,
                print_event,
            );
            last_covered = covered;
        }
    }

    // `finish()` zero-pads the end of the file and closes every still-open
    // note, exactly as the reference does over a whole file.
    let final_notes = transcriber.finish()?;
    let final_note_events = transcriber.note_events();
    pipeline::emit_confirmed_notes(final_notes, &final_note_events, &mut key_estimator, &mut lines, print_event);
    pipeline::report_new_chords(
        &final_note_events,
        last_covered,
        transcriber.covered_seconds(),
        &mut chord_display,
        &mut key_estimator,
        &pipeline::KeyMode::Auto,
        None,
        &mut last_key_name,
        print_event,
    );

    Ok(())
}

fn run_live(config: NoteModelConfig, hop_seconds: f32, duration_s: Option<f32>) -> Result<()> {
    println!("Listening to default speaker output via loopback capture ...");
    match duration_s {
        Some(d) => println!("Will stop after {d:.1}s (Ctrl+C to stop earlier)\n"),
        None => println!("Press Ctrl+C to stop.\n"),
    }
    let never_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let key_mode = std::sync::Arc::new(std::sync::Mutex::new(pipeline::KeyMode::Auto));
    let mut printed_until = f32::NEG_INFINITY;
    pipeline::run_live_session(config, hop_seconds, duration_s, never_stop, key_mode, move |event| {
        if let PipelineEvent::BeatLines { committed, .. } = &event {
            let from = printed_until;
            for line in committed.iter().filter(|l| l.t > from) {
                println!("  {} {:6.2}s", if line.bar { "|| bar " } else { "|  beat" }, line.t);
                printed_until = line.t;
            }
        }
        print_event(event)
    })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut audio_file: Option<String> = None;
    let mut hop = CAPTURE_HOP_S;
    let mut size = ModelSize::Mix;
    let mut future_frames = DEFAULT_FUTURE_FRAMES;
    let mut allow_directml = true;
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
            "--size" => {
                i += 1;
                let value = args.get(i).context("--size expects small, medium, large or mix")?;
                size = value.parse()?;
            }
            "--future" => {
                i += 1;
                let value = args.get(i).context("--future expects a number")?;
                let parsed: usize = value.parse().context("--future expects a number")?;
                if !FUTURE_FRAME_CHOICES.contains(&parsed) {
                    bail!("--future must be one of {FUTURE_FRAME_CHOICES:?}, got {parsed}");
                }
                future_frames = parsed;
            }
            "--cpu" => allow_directml = false,
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
        let config = NoteModelConfig { size, future_frames, allow_directml };
        run_live(config, hop, duration)
    } else {
        let audio_file = audio_file.context(
            "usage: rekord_engine <audio_file> [--size small|medium|large|mix] [--future 8|16] [--cpu] [--hop 0.064]",
        )?;
        run_file(&audio_file, size, future_frames, allow_directml, hop)
    }
}
