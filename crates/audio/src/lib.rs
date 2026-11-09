//! Real-time CoreAudio/cpal engine for the polyrhythmic metronome.

pub mod engine;
pub mod messages;
pub mod stream;

pub use engine::{Engine, EngineHandle, EngineInit};
pub use messages::{BeatEvent, Command, PatternUpdate, PositionReader, PositionSnapshot};
pub use stream::{AudioError, AudioService, DeviceInfo, Transport, query_output_latency};
