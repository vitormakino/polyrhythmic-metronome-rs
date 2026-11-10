//! What crosses between the UI thread and the audio thread.
//!
//! Everything here moves through lock-free queues or atomics: the audio
//! thread never waits for the UI.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};
use std::time::Instant;

use pm_core::session::SessionPlan;
use pm_core::{Bpm, MusicalPosition, Pattern, PulseId, SampleBank};

/// UI → engine. Applied at the first frame of the next rendered buffer.
pub enum Command {
    /// Start at measure 0 (a "one"), running the selected session if there
    /// is one. No-op while playing.
    Play,
    /// Silence at once (ringing clicks are cut); ends a running session.
    Stop,
    /// While a session runs, it also shifts the session's tempo (see
    /// [`Runner::user_bpm`](pm_core::Runner::user_bpm)).
    SetBpm(Bpm),
    /// While a session runs, a pulse whose `enabled` changes is the user's
    /// until the stage ends (see [`Runner::user_pulse`](pm_core::Runner::user_pulse)).
    SetPattern(Box<PatternUpdate>),
    /// The user turned the pulse at this position (0-based) on or off where
    /// the running stage overrides it: the stage's override goes until the
    /// stage ends and the pattern's `enabled` plays (see
    /// [`Runner::user_pulse`](pm_core::Runner::user_pulse)). Needed when the
    /// click leaves the pattern's `enabled` as it was, so `SetPattern` sees
    /// no change. Ignored without a running session.
    UserPulse(usize),
    /// Selects the session plan the next `Play` runs (`None`: plain
    /// metronome). While a session runs it keeps its plan; this one waits for
    /// the next `Play`. Replaced plans come back through
    /// [`EngineHandle::session_garbage`](crate::EngineHandle) to be dropped
    /// there.
    Session(Option<Box<SessionPlan>>),
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
    /// The session silenced this hit (a muted pulse, a silent bar or a random
    /// draw): nothing sounds. Hits of pulses the session turned off produce
    /// no event at all.
    pub muted: bool,
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
