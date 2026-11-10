//! Real-time CoreAudio/cpal engine for the polyrhythmic metronome.

pub mod engine;
pub mod messages;
mod session;
pub mod stream;
mod watch;

pub use engine::{Engine, EngineHandle, EngineInit, soft_clip};
pub use messages::{BeatEvent, Command, PatternUpdate, PositionReader, PositionSnapshot};
pub use session::SessionReader;
pub use stream::{
    AudioError, AudioService, DeviceInfo, OutputLatency, TapRx, Transport, query_output_latency,
};
pub use watch::{RebuildCounts, RebuildReason};
