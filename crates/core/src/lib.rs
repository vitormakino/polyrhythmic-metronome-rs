//! Pure domain model, exact timeline and offline synthesis for the polyrhythmic metronome.

pub mod model;
pub mod synth;
pub mod tap;
pub mod timeline;

pub use model::{Bpm, Intensity, MAX_LOADED_BEATS, MAX_PULSES, Pattern, Pulse, PulseId, SoundType};
pub use synth::SampleBank;
pub use tap::TapTempo;
pub use timeline::{ApplyMode, BeatHit, MusicalPosition, TempoSource, Timeline};
