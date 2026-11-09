//! What crosses between the UI thread and the audio thread.
//!
//! Everything here moves through lock-free queues or atomics: the audio
//! thread never waits for the UI.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};
use std::time::Instant;

use pm_core::{Bpm, MusicalPosition, Pattern, PulseId, SampleBank};

/// UI → engine. Applied at the first frame of the next rendered buffer.
pub enum Command {
    /// Start at measure 0 (a "one"). No-op while playing.
    Play,
    /// Silence at once (ringing clicks are cut).
    Stop,
    SetBpm(Bpm),
    SetPattern(Box<PatternUpdate>),
}

/// A pattern with its clips, built off the audio thread. Replaced updates
/// come back through [`EngineHandle::garbage`](crate::EngineHandle) to be
/// dropped there.
pub struct PatternUpdate {
    pub pattern: Pattern,
    /// Clips for `pattern`, by pulse index (`SampleBank::build(sr, &pattern)`
    /// at the engine's sample rate).
    pub bank: SampleBank,
}

impl PatternUpdate {
    /// Builds the clips for `pattern` at `sample_rate`. Allocates.
    pub fn build(sample_rate: u32, pattern: Pattern) -> Box<PatternUpdate> {
        Box::new(PatternUpdate {
            bank: SampleBank::build(sample_rate, &pattern),
            pattern,
        })
    }
}

/// Engine → UI: one click that was scheduled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatEvent {
    pub pulse: PulseId,
    pub beat: u32,
    /// Absolute frame of the engine's stream.
    pub frame: u64,
    /// When the click reaches the DAC.
    pub at: Instant,
}

/// Where the engine is: `position` is the musical position at `frame`, the
/// first frame not rendered yet (so `frame` is the number of frames rendered).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositionSnapshot {
    pub frame: u64,
    pub position: MusicalPosition,
    pub playing: bool,
}

/// Seqlock over atomics: one writer (the audio thread, wait-free), any
/// number of readers that retry while a write is in progress.
#[derive(Default)]
struct PositionCell {
    seq: AtomicU64,
    frame: AtomicU64,
    measure: AtomicU64,
    ticks: AtomicU64,
    ticks_per_measure: AtomicU64,
    playing: AtomicBool,
}

/// Engine side of the position cell.
pub(crate) struct PositionWriter(Arc<PositionCell>);

/// UI side: the latest [`PositionSnapshot`] the engine published.
#[derive(Clone)]
pub struct PositionReader(Arc<PositionCell>);

pub(crate) fn position_cell() -> (PositionWriter, PositionReader) {
    let cell = Arc::new(PositionCell::default());
    (PositionWriter(Arc::clone(&cell)), PositionReader(cell))
}

impl PositionWriter {
    /// Publishes `s`. Never blocks or allocates.
    pub(crate) fn publish(&self, s: PositionSnapshot) {
        let c = &*self.0;
        let seq = c.seq.load(Ordering::Relaxed);
        c.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        c.frame.store(s.frame, Ordering::Relaxed);
        c.measure.store(s.position.measure, Ordering::Relaxed);
        c.ticks.store(s.position.ticks, Ordering::Relaxed);
        c.ticks_per_measure
            .store(s.position.ticks_per_measure, Ordering::Relaxed);
        c.playing.store(s.playing, Ordering::Relaxed);
        c.seq.store(seq.wrapping_add(2), Ordering::Release);
    }
}

impl PositionReader {
    /// The latest consistent snapshot (all fields from the same publish).
    pub fn snapshot(&self) -> PositionSnapshot {
        let c = &*self.0;
        loop {
            let before = c.seq.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let s = PositionSnapshot {
                frame: c.frame.load(Ordering::Relaxed),
                position: MusicalPosition {
                    measure: c.measure.load(Ordering::Relaxed),
                    ticks: c.ticks.load(Ordering::Relaxed),
                    ticks_per_measure: c.ticks_per_measure.load(Ordering::Relaxed),
                },
                playing: c.playing.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            if c.seq.load(Ordering::Relaxed) == before {
                return s;
            }
        }
    }
}
