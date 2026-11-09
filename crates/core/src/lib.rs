//! Pure domain model, exact timeline and offline synthesis for the polyrhythmic metronome.

pub mod model;
pub mod synth;
pub mod timeline;

pub use model::{Bpm, Intensity, Pattern, Pulse, PulseId, SoundType};
pub use synth::SampleBank;
pub use timeline::{ApplyMode, BeatHit, MusicalPosition, TempoSource, Timeline};
