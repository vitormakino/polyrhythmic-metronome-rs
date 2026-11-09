//! Real-time CoreAudio/cpal engine for the polyrhythmic metronome.

pub mod engine;
pub mod messages;

pub use engine::{Engine, EngineHandle, EngineInit};
pub use messages::{BeatEvent, Command, PatternUpdate, PositionReader, PositionSnapshot};
