use anyhow::{bail, Context, Result};

use rekord_core::audio_io;
use rekord_core::chord_layers::{ChordEngine, ChordLayerId};
use rekord_core::chord_templates::{extra_tensions, ChordMatch};
use rekord_core::harmonica_model::{self, Device, HarmonicaModel, ModelChoice, Part};
use rekord_core::harmonica_stream::NoteTranscriber;
use rekord_core::key_estimator::KeyEstimator;
use rekord_core::pipeline::{
    self, midi_to_name, NoteModelConfig, NoteModelInfo, PipelineEvent, CAPTURE_HOP_S,
    DEFAULT_FUTURE_FRAMES, FUTURE_FRAME_CHOICES,
};
use rekord_core::spelling::Accidental;

fn device_name(device: Device) -> &'static str {
    match device {
        Device::DirectML => "DirectML",
        Device::Cpu => "CPU",
    }
}

fn over_budget_note(note_model: NoteModelInfo) -> &'static str {
    if note_model.over_budget {
        " -- WARNING: this size may not keep up with real time on this hardware"
    } else {
        ""
    }
}

fn print_model_info(note_model: NoteModelInfo) {
    println!(
        "Using {} on {} (look-ahead {} frames){}\n",
        note_model.size.label(),
        device_name(note_model.device),
        note_model.future_frames,
        over_budget_note(note_model),
    );
}

/// Prints the pipeline's events. Of the chord layers only `layers` are printed (the tensions
/// are those of the first of them); the default, the settled layer, prints exactly what the
/// single chord logic printed before the layers.
struct Printer {
    accidental: Accidental,
    layers: Vec<ChordLayerId>,
    chord: Option<ChordMatch>,
    pcs: u16,
    tensions: Vec<&'static str>,
}

impl Printer {
    fn new(accidental: Accidental, layers: Vec<ChordLayerId>) -> Self {
        Self {
            accidental,
            layers,
            chord: None,
            pcs: 0,
            tensions: Vec::new(),
        }
    }

    fn update_tensions(&mut self) {
        let now = match self.chord {
            Some(chord) => {
                let pcs: Vec<i32> = (0..12).filter(|pc| self.pcs & (1 << pc) != 0).collect();
                extra_tensions(&pcs, &chord)
            }
            None => Vec::new(),
        };
        if now != self.tensions {
            if !now.is_empty() {
                println!("    tensions: {}", now.join(", "));
            }
            self.tensions = now;
        }
    }

    fn print(&mut self, event: PipelineEvent) {
        let accidental = self.accidental;
        match event {
            PipelineEvent::ChordLayer { layer, changes, .. } => {
                if !self.layers.contains(&layer) {
                    return;
                }
                let tag = if self.layers.len() > 1 {
                    format!("  [{layer:?}]")
                } else {
                    String::new()
                };
                for c in changes {
                    let suffix = c
                        .roman_numeral
                        .map(|r| format!("  ({r})"))
                        .unwrap_or_default();
                    println!("  t={:6.2}s  chord: {}{suffix}{tag}", c.t, c.label);
                    if layer == self.layers[0] {
                        self.chord =
                            c.root_pc
                                .zip(c.quality)
                                .map(|(root_pc, quality)| ChordMatch {
                                    root_pc,
                                    quality,
                                    bass_pc: None,
                                });
                    }
                }
                self.update_tensions();
            }
            PipelineEvent::ActiveNotes { changes } => {
                if let Some(last) = changes.last() {
                    self.pcs = last.pcs;
                    self.update_tensions();
                }
            }
            other => print_event(other, accidental),
        }
    }
}

fn print_event(event: PipelineEvent, accidental: Accidental) {
    match event {
        PipelineEvent::Started {
            total_latency_s,
            note_model,
            ..
        } => {
            println!(
                "(model: {} on {}, look-ahead {} frames; confirm latency: {total_latency_s:.2}s){}\n",
                note_model.size.label(),
                device_name(note_model.device),
                note_model.future_frames,
                over_budget_note(note_model),
            );
        }
        PipelineEvent::NoteConfirmed {
            start_s,
            end_s,
            pitch_midi,
            velocity,
            part,
        } => {
            let role = match part {
                Part::Vocal => " [vocal]",
                Part::Bass => " [bass part]",
                Part::Other => " [other]",
            };
            println!(
                "[{start_s:6.2}s -> {end_s:6.2}s] note {} (vel {velocity:.2}){role}",
                midi_to_name(pitch_midi, accidental)
            );
        }
        PipelineEvent::KeyChange { key_name, .. } => {
            println!("  key: {key_name}");
        }
        // the CLI prints a note once, when it ends
        PipelineEvent::NoteStarted { .. } => {}
        // snapshots of a moving window; run_live prints each committed line once instead
        PipelineEvent::BeatLines { .. } => {}
        // the chord events are the Printer's
        PipelineEvent::ChordLayer { .. } | PipelineEvent::ActiveNotes { .. } => {}
    }
}

fn run_file(
    audio_path: &str,
    choice: ModelChoice,
    future_frames: usize,
    allow_directml: bool,
    hop_seconds: f32,
    accidental: Accidental,
    chord_layers: Vec<ChordLayerId>,
) -> Result<()> {
    println!("Loading {audio_path} ...");
    let audio = audio_io::load_audio(audio_path, harmonica_model::SAMPLE_RATE)?;
    let duration = audio.len() as f32 / harmonica_model::SAMPLE_RATE as f32;
    println!("Duration: {duration:.1}s, simulating stream with {hop_seconds:.2}s hops\n");

    let model = HarmonicaModel::load(choice, allow_directml).context("loading Okarina model")?;
    print_model_info(NoteModelInfo::of(&model, future_frames));
    let mut transcriber = NoteTranscriber::new(model, future_frames);
    let mut key_estimator = KeyEstimator::new();
    let mut chords = ChordEngine::standard();
    let mut printer = Printer::new(accidental, chord_layers);

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
            pipeline::emit_confirmed_notes(notes, &mut key_estimator, |e| printer.print(e));
            chords.advance(
                &note_events,
                covered,
                &mut key_estimator,
                &pipeline::KeyMode::Auto,
                accidental,
                None, // file mode has no beat tracker to snap chord onsets to
                |e| printer.print(e),
            );
        }
    }

    // `finish()` zero-pads the end of the file and closes every still-open
    // note, exactly as the reference does over a whole file.
    let final_notes = transcriber.finish()?;
    let final_note_events = transcriber.note_events();
    pipeline::emit_confirmed_notes(final_notes, &mut key_estimator, |e| printer.print(e));
    chords.advance(
        &final_note_events,
        transcriber.covered_seconds(),
        &mut key_estimator,
        &pipeline::KeyMode::Auto,
        accidental,
        None,
        |e| printer.print(e),
    );
    chords.finish(transcriber.covered_seconds(), accidental, |e| {
        printer.print(e)
    });

    Ok(())
}

fn run_live(
    config: NoteModelConfig,
    hop_seconds: f32,
    duration_s: Option<f32>,
    accidental: Accidental,
    chord_layers: Vec<ChordLayerId>,
) -> Result<()> {
    println!("Listening to default speaker output via loopback capture ...");
    match duration_s {
        Some(d) => println!("Will stop after {d:.1}s (Ctrl+C to stop earlier)\n"),
        None => println!("Press Ctrl+C to stop.\n"),
    }
    let never_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let key_mode = std::sync::Arc::new(std::sync::Mutex::new(pipeline::KeyMode::Auto));
    let accidental_setting = std::sync::Arc::new(std::sync::Mutex::new(accidental));
    let mut printed_until = f32::NEG_INFINITY;
    let mut printer = Printer::new(accidental, chord_layers);
    pipeline::run_live_session(
        config,
        hop_seconds,
        duration_s,
        never_stop,
        key_mode,
        accidental_setting,
        None,
        move |event| {
            if let PipelineEvent::BeatLines { committed, .. } = &event {
                let from = printed_until;
                for line in committed.iter().filter(|l| l.t > from) {
                    println!(
                        "  {} {:6.2}s",
                        if line.bar { "|| bar " } else { "|  beat" },
                        line.t
                    );
                    printed_until = line.t;
                }
            }
            printer.print(event)
        },
    )
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut audio_file: Option<String> = None;
    let mut hop = CAPTURE_HOP_S;
    let mut choice = ModelChoice::Auto;
    let mut future_frames = DEFAULT_FUTURE_FRAMES;
    let mut allow_directml = true;
    let mut accidental = Accidental::Auto;
    let mut live = false;
    let mut duration: Option<f32> = None;
    let mut chord_layers = vec![ChordLayerId::Settled];
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
                let value = args.get(i).context("--size expects auto, s, m or l")?;
                choice = value.parse()?;
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
            "--accidental" => {
                i += 1;
                let value = args
                    .get(i)
                    .context("--accidental expects auto, sharp or flat")?;
                accidental = value.parse()?;
            }
            "--chord-layer" => {
                i += 1;
                let value = args
                    .get(i)
                    .context("--chord-layer expects instant, settled, confirmed or all")?;
                chord_layers = match value.as_str() {
                    "instant" => vec![ChordLayerId::Instant],
                    "settled" => vec![ChordLayerId::Settled],
                    "confirmed" => vec![ChordLayerId::Confirmed],
                    "all" => vec![
                        ChordLayerId::Settled,
                        ChordLayerId::Instant,
                        ChordLayerId::Confirmed,
                    ],
                    other => bail!(
                        "--chord-layer expects instant, settled, confirmed or all, got {other}"
                    ),
                };
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
        let config = NoteModelConfig {
            choice,
            future_frames,
            allow_directml,
        };
        run_live(config, hop, duration, accidental, chord_layers)
    } else {
        let audio_file = audio_file.context(
            "usage: rekord_engine <audio_file> [--size auto|s|m|l] [--future 8|16] [--accidental auto|sharp|flat] [--chord-layer instant|settled|confirmed|all] [--cpu] [--hop 0.064]",
        )?;
        run_file(
            &audio_file,
            choice,
            future_frames,
            allow_directml,
            hop,
            accidental,
            chord_layers,
        )
    }
}
