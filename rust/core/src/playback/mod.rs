//! Playback of the recorded loopback audio plus the transcribed notes and drum
//! hits synthesized as simple sounds (see `audio_ring` for the recording).

pub mod drum_synth;
pub mod engine;
pub mod synth;

pub use drum_synth::SynthDrumHit;
pub use engine::{start, PartVoice, PlaybackEvent, PlaybackParams, PlaybackSession};
pub use synth::{SynthNote, Wave};
