//! Records what the piano roll would receive from the beat tracker for a WAV file, for
//! checking beat-line timing and rendering without playing audio (see CLAUDE.md, "Beat
//! lines: checking timing and rendering").
//!
//! Feeds the audio through the live session's own beat thread (`pipeline::run_beat_tracker`)
//! at real-time pace, in `--hop`-second chunks like the loopback capture, and writes every
//! `BeatLines` event as one JSON line with `wall`: seconds since the start, i.e. since the
//! `Started` event. The timings therefore include this machine's real compute delay.
//!
//!     cargo run --release -p rekord_core --example beat_replay -- <in.wav> <out.jsonl> [--seconds 90] [--hop 0.25]

use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rekord_core::audio_io::load_audio;
use rekord_core::beat_model;
use rekord_core::pipeline::{run_beat_tracker, BeatLine, PipelineEvent};

fn json(v: &[BeatLine]) -> String {
    let items: Vec<String> = v.iter().map(|l| format!("[{:.4},{}]", l.t, l.bar as u8)).collect();
    format!("[{}]", items.join(","))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: beat_replay <in.wav> <out.jsonl> [--seconds S] [--hop H]";
    let (input, output) = (args.get(1).context(usage)?, args.get(2).context(usage)?);
    let mut seconds = f64::INFINITY;
    let mut hop = 0.25f64;
    let mut i = 3;
    while i < args.len() {
        let value: f64 = args.get(i + 1).context(usage)?.parse().context(usage)?;
        match args[i].as_str() {
            "--seconds" => seconds = value,
            "--hop" => hop = value,
            _ => anyhow::bail!(usage),
        }
        i += 2;
    }

    let mut audio = load_audio(input, beat_model::SAMPLE_RATE)?;
    audio.truncate(((seconds * beat_model::SAMPLE_RATE as f64) as usize).min(audio.len()));
    let out = Mutex::new(std::fs::File::create(output).with_context(|| format!("creating {output}"))?);
    let n = (hop * beat_model::SAMPLE_RATE as f64).round() as usize;
    eprintln!("replaying {:.1}s of {input} in real time ...", audio.len() as f64 / beat_model::SAMPLE_RATE as f64);

    let t0 = Instant::now();
    let on_event = Mutex::new(|event: PipelineEvent| {
        if let PipelineEvent::BeatLines { from_s, committed, provisional, predicted } = event {
            let wall = t0.elapsed().as_secs_f64();
            let line = format!(
                "{{\"wall\":{wall:.4},\"from_s\":{from_s:.4},\"committed\":{},\"provisional\":{},\"predicted\":{}}}",
                json(&committed),
                json(&provisional),
                json(&predicted)
            );
            writeln!(out.lock().unwrap(), "{line}").unwrap();
        }
    });
    let stop = AtomicBool::new(false);
    let (tx, rx) = channel();
    std::thread::scope(|scope| {
        scope.spawn(|| run_beat_tracker(rx, &stop, &on_event));
        for (k, chunk) in audio.chunks(n).enumerate() {
            // chunk k covers [k hop, (k+1) hop) and is delivered once it has all been captured
            let due = Duration::from_secs_f64((k + 1) as f64 * hop);
            if let Some(wait) = due.checked_sub(t0.elapsed()) {
                std::thread::sleep(wait);
            }
            tx.send(chunk.to_vec()).unwrap();
        }
        drop(tx); // the beat thread flushes and exits; the scope joins it
    });
    eprintln!("wrote {output}");
    Ok(())
}
