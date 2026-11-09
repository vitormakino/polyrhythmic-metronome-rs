//! When the open stream must be rebuilt, decided from what
//! [`AudioService::tick`](crate::AudioService::tick) sees about 4 times a
//! second. Pure, so every case is a unit test.
//!
//! No single odd sample rebuilds: a default output that is missing or
//! different must stay so for a while, a wall clock that jumped counts only
//! if the data callback stopped too, and the callback counter is the one
//! stall detector (the data callback's own timestamps are not trusted).

use std::fmt;
use std::time::{Duration, Instant};

use cpal::ErrorKind;

/// The data callback not called for this long is a stall.
pub(crate) const STALL: Duration = Duration::from_secs(1);
/// No default output for this long (every tick in between) is a device
/// change to "none". A real loss is also reported by cpal as
/// `DeviceNotAvailable`, which rebuilds at once.
pub(crate) const NO_DEVICE_GRACE: Duration = Duration::from_secs(1);

/// Why the service replaced (or dropped) its stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebuildReason {
    /// The wall clock jumped past the monotonic one (the machine slept) and
    /// the data callback was not called since the previous tick.
    Slept,
    /// The data callback was not called for over a second.
    Stalled,
    /// The stream's error callback reported this (first error kept).
    StreamError(ErrorKind),
    /// The default output changed: to another device, to none (`to:
    /// None`), or from none to one (`from: None`).
    DeviceChanged {
        from: Option<String>,
        to: Option<String>,
    },
}

impl fmt::Display for RebuildReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |uid: &Option<String>| uid.clone().unwrap_or_else(|| "none".to_owned());
        match self {
            RebuildReason::Slept => f.write_str("slept (wall clock jumped, no callbacks)"),
            RebuildReason::Stalled => write!(f, "stalled (no callback for {STALL:?})"),
            RebuildReason::StreamError(kind) => write!(f, "stream error {kind:?}"),
            RebuildReason::DeviceChanged { from, to } => {
                write!(f, "device changed {} -> {}", name(from), name(to))
            }
        }
    }
}

/// Rebuilds so far, by reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RebuildCounts {
    pub slept: u64,
    pub stalled: u64,
    pub stream_error: u64,
    pub device_changed: u64,
}

impl RebuildCounts {
    pub(crate) fn count(&mut self, reason: &RebuildReason) {
        let n = match reason {
            RebuildReason::Slept => &mut self.slept,
            RebuildReason::Stalled => &mut self.stalled,
            RebuildReason::StreamError(_) => &mut self.stream_error,
            RebuildReason::DeviceChanged { .. } => &mut self.device_changed,
        };
        *n += 1;
    }

    pub fn total(&self) -> u64 {
        self.slept + self.stalled + self.stream_error + self.device_changed
    }
}

/// What the watcher remembers about one open stream between ticks.
#[derive(Debug)]
pub(crate) struct WatchState {
    /// Data callbacks at the last tick, and when the count last moved.
    seen: (u64, Instant),
    /// First tick of the current run of ticks with no default output.
    missing_since: Option<Instant>,
    /// A different default output seen on the previous tick only.
    candidate: Option<String>,
}

impl WatchState {
    /// For a stream opened at `now`.
    pub(crate) fn new(now: Instant) -> WatchState {
        WatchState {
            seen: (0, now),
            missing_since: None,
            candidate: None,
        }
    }
}

/// One tick's view of an open stream.
#[derive(Debug)]
pub(crate) struct Observation<'a> {
    pub now: Instant,
    /// The wall clock ran more than [`STALL`] ahead of the monotonic one
    /// since the previous tick.
    pub wall_jumped: bool,
    /// Data callbacks so far.
    pub callbacks: u64,
    /// First error the stream's error callback reported, if any.
    pub error: Option<ErrorKind>,
    /// UID of the device the stream plays on.
    pub uid: &'a str,
    /// UID of the default output now (`None`: none, or the query failed).
    pub default_uid: Option<String>,
}

/// Whether the stream `obs` describes must be rebuilt, and why.
pub(crate) fn rebuild_reason(
    state: &mut WatchState,
    obs: Observation<'_>,
) -> Option<RebuildReason> {
    let advanced = obs.callbacks != state.seen.0;
    if advanced {
        state.seen = (obs.callbacks, obs.now);
    }
    if let Some(kind) = obs.error {
        return Some(RebuildReason::StreamError(kind));
    }
    // A wall-clock step alone (NTP, the user setting the time) is not sleep.
    if obs.wall_jumped && !advanced {
        return Some(RebuildReason::Slept);
    }
    // Silent only counts once the stream has run (AirPlay starts slowly).
    if obs.callbacks > 0 && obs.now.duration_since(state.seen.1) > STALL {
        return Some(RebuildReason::Stalled);
    }
    let changed = |to| RebuildReason::DeviceChanged {
        from: Some(obs.uid.to_owned()),
        to,
    };
    match obs.default_uid {
        // Unknown for now: the query can fail once, or the default flap.
        None => {
            state.candidate = None;
            let since = *state.missing_since.get_or_insert(obs.now);
            (obs.now.duration_since(since) >= NO_DEVICE_GRACE).then(|| changed(None))
        }
        Some(uid) if uid == obs.uid => {
            state.missing_since = None;
            state.candidate = None;
            None
        }
        // The same other device on two ticks in a row.
        Some(uid) => {
            state.missing_since = None;
            if state.candidate.as_deref() == Some(uid.as_str()) {
                Some(changed(Some(uid)))
            } else {
                state.candidate = Some(uid);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(250);

    /// A stream on device "A" whose callback runs, ticked every 250 ms.
    struct Sim {
        state: WatchState,
        start: Instant,
        tick: u32,
        callbacks: u64,
    }

    impl Sim {
        fn new() -> Sim {
            let start = Instant::now();
            Sim {
                state: WatchState::new(start),
                start,
                tick: 0,
                callbacks: 0,
            }
        }

        /// One tick; `advance` callbacks ran since the previous one.
        fn step(
            &mut self,
            advance: u64,
            default_uid: Option<&str>,
            wall_jumped: bool,
            error: Option<ErrorKind>,
        ) -> Option<RebuildReason> {
            self.tick += 1;
            self.callbacks += advance;
            rebuild_reason(
                &mut self.state,
                Observation {
                    now: self.start + TICK * self.tick,
                    wall_jumped,
                    callbacks: self.callbacks,
                    error,
                    uid: "A",
                    default_uid: default_uid.map(str::to_owned),
                },
            )
        }

        /// A healthy tick: callbacks ran, the default output is `uid`.
        fn on(&mut self, uid: Option<&str>) -> Option<RebuildReason> {
            self.step(20, uid, false, None)
        }
    }

    #[test]
    fn healthy_stream_is_kept() {
        let mut s = Sim::new();
        for _ in 0..40 {
            assert_eq!(s.on(Some("A")), None);
        }
    }

    #[test]
    fn default_output_missing_once_does_not_rebuild() {
        let mut s = Sim::new();
        s.on(Some("A"));
        assert_eq!(s.on(None), None);
        for _ in 0..10 {
            assert_eq!(s.on(Some("A")), None);
        }
        // A later short gap starts its own count.
        for _ in 0..3 {
            assert_eq!(s.on(None), None);
        }
        assert_eq!(s.on(Some("A")), None);
        for _ in 0..3 {
            assert_eq!(s.on(None), None);
        }
    }

    #[test]
    fn default_output_missing_for_a_second_rebuilds() {
        let mut s = Sim::new();
        s.on(Some("A"));
        // Missing at 0, 250, 500, 750 ms of the gap: not yet.
        for _ in 0..4 {
            assert_eq!(s.on(None), None);
        }
        assert_eq!(
            s.on(None),
            Some(RebuildReason::DeviceChanged {
                from: Some("A".to_owned()),
                to: None
            })
        );
    }

    #[test]
    fn default_output_flapping_back_does_not_rebuild() {
        let mut s = Sim::new();
        s.on(Some("A"));
        assert_eq!(s.on(Some("B")), None);
        assert_eq!(s.on(Some("A")), None);
        // B again, once: still only a candidate, not B held twice.
        assert_eq!(s.on(Some("B")), None);
        assert_eq!(s.on(Some("C")), None);
        assert_eq!(s.on(Some("A")), None);
    }

    #[test]
    fn default_output_changed_for_two_ticks_rebuilds() {
        let mut s = Sim::new();
        s.on(Some("A"));
        assert_eq!(s.on(Some("B")), None);
        assert_eq!(
            s.on(Some("B")),
            Some(RebuildReason::DeviceChanged {
                from: Some("A".to_owned()),
                to: Some("B".to_owned())
            })
        );
    }

    #[test]
    fn wall_clock_step_while_callbacks_run_does_not_rebuild() {
        let mut s = Sim::new();
        s.on(Some("A"));
        assert_eq!(s.step(20, Some("A"), true, None), None);
        assert_eq!(s.on(Some("A")), None);
    }

    #[test]
    fn wall_clock_jump_without_callbacks_is_sleep() {
        let mut s = Sim::new();
        s.on(Some("A"));
        assert_eq!(s.step(0, Some("A"), true, None), Some(RebuildReason::Slept));
    }

    #[test]
    fn callbacks_stopped_for_over_a_second_is_a_stall() {
        let mut s = Sim::new();
        s.on(Some("A"));
        // Last moved at 250 ms; still at 1250 ms is exactly 1 s: not over.
        for _ in 0..4 {
            assert_eq!(s.step(0, Some("A"), false, None), None);
        }
        assert_eq!(
            s.step(0, Some("A"), false, None),
            Some(RebuildReason::Stalled)
        );
    }

    #[test]
    fn a_stream_that_never_ran_is_not_stalled() {
        let mut s = Sim::new();
        for _ in 0..20 {
            assert_eq!(s.step(0, Some("A"), false, None), None);
        }
    }

    #[test]
    fn stream_error_rebuilds_at_once() {
        let mut s = Sim::new();
        s.on(Some("A"));
        assert_eq!(
            s.step(20, Some("A"), false, Some(ErrorKind::DeviceNotAvailable)),
            Some(RebuildReason::StreamError(ErrorKind::DeviceNotAvailable))
        );
    }

    #[test]
    fn reasons_are_counted_and_printable() {
        let mut counts = RebuildCounts::default();
        let reasons = [
            RebuildReason::Slept,
            RebuildReason::Stalled,
            RebuildReason::StreamError(ErrorKind::StreamInvalidated),
            RebuildReason::DeviceChanged {
                from: Some("A".to_owned()),
                to: None,
            },
            RebuildReason::DeviceChanged {
                from: None,
                to: Some("B".to_owned()),
            },
        ];
        for r in &reasons {
            counts.count(r);
        }
        assert_eq!(
            counts,
            RebuildCounts {
                slept: 1,
                stalled: 1,
                stream_error: 1,
                device_changed: 2
            }
        );
        assert_eq!(counts.total(), 5);
        assert_eq!(reasons[3].to_string(), "device changed A -> none");
        assert_eq!(reasons[4].to_string(), "device changed none -> B");
        assert_eq!(reasons[2].to_string(), "stream error StreamInvalidated");
    }
}
