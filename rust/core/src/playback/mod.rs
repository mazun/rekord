//! Playback of the recorded loopback audio plus the transcribed notes
//! synthesized as simple waves (see `audio_ring` for the recording).

pub mod engine;
pub mod synth;

pub use engine::{start, PartVoice, PlaybackEvent, PlaybackParams, PlaybackSession};
pub use synth::{SynthNote, Wave};
