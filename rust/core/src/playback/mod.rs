//! Playback of the recorded loopback audio plus the transcribed notes as sine
//! waves (see `audio_ring` for the recording).

pub mod engine;
pub mod synth;

pub use engine::{start, PlaybackEvent, PlaybackParams, PlaybackSession};
pub use synth::SynthNote;
