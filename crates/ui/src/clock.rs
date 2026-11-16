//! A continuous position in the measure, rebuilt on the UI side from the
//! beat events the engine already sends. The topology drawings move with it
//! at constant speed and touch their marks when a beat sounds.
//!
//! The clock extrapolates linearly from a base point and bends gently
//! towards each new anchor (a beat event) instead of jumping to it.

use std::collections::VecDeque;
use std::time::Instant;

/// Seconds a small correction takes to blend in.
const SLEW: f64 = 0.120;
/// An anchor further than this (in measures) from the prediction snaps.
const SNAP: f64 = 0.25;
/// The measure length before the first `set_measure`, in seconds.
const DEFAULT_MEASURE: f64 = 2.0;
/// Anchors kept waiting for their time; the oldest goes first.
const MAX_WAITING: usize = 4096;

/// Seconds from `b` to `a`, negative when `a` is earlier.
fn secs_since(a: Instant, b: Instant) -> f64 {
    if a >= b {
        a.duration_since(b).as_secs_f64()
    } else {
        -b.duration_since(a).as_secs_f64()
    }
}

/// Wraps a phase difference to `[-0.5, 0.5)`.
fn wrap_error(d: f64) -> f64 {
    (d + 0.5).rem_euclid(1.0) - 0.5
}

/// Measure phase derived from beat events. Times are `Instant`s in the
/// shown-time domain (`now - visual_offset`); anchors carry their own `at`.
#[derive(Debug, Clone)]
pub struct MeasureClock {
    running: bool,
    /// The base is a guess (Play or reset), not a beat: the next anchor replaces it.
    provisional: bool,
    /// Measure length in seconds.
    measure: f64,
    /// Linear part: `base_phase` at `base_at`, advancing one per `measure`.
    base_at: Instant,
    base_phase: f64,
    /// The correction being blended in: (start, total error in measures).
    slew: Option<(Instant, f64)>,
    /// Anchors not yet due, sorted by time.
    waiting: VecDeque<(Instant, f64)>,
}

impl Default for MeasureClock {
    fn default() -> Self {
        Self {
            running: false,
            provisional: false,
            measure: DEFAULT_MEASURE,
            base_at: Instant::now(),
            base_phase: 0.0,
            slew: None,
            waiting: VecDeque::new(),
        }
    }
}

impl MeasureClock {
    /// Blended correction applied at `t`.
    fn slew_at(&self, t: Instant) -> f64 {
        self.slew.map_or(0.0, |(start, e)| {
            e * (secs_since(t, start) / SLEW).clamp(0.0, 1.0)
        })
    }

    /// Unwrapped phase at `t`, correction included.
    fn raw_at(&self, t: Instant) -> f64 {
        self.base_phase + secs_since(t, self.base_at) / self.measure + self.slew_at(t)
    }

    /// Applies every anchor whose time is `<= t`.
    fn apply_due(&mut self, t: Instant) {
        while let Some(&(at, phase)) = self.waiting.front() {
            if at > t {
                break;
            }
            self.waiting.pop_front();
            let predicted = self.raw_at(at);
            let e = wrap_error(phase - predicted);
            self.base_at = at;
            if self.provisional || e.abs() > SNAP {
                self.base_phase = phase;
                self.slew = None;
                self.provisional = false;
            } else {
                self.base_phase = predicted;
                self.slew = Some((at, e));
            }
        }
    }

    /// Play pressed: phase 0 at `at`, provisional until a beat anchors it.
    /// Anchors already waiting are kept.
    pub fn start(&mut self, at: Instant) {
        self.running = true;
        self.provisional = true;
        self.base_at = at;
        self.base_phase = 0.0;
        self.slew = None;
    }

    /// Stop pressed: [`Self::phase`] is `None` and waiting anchors go.
    pub fn stop(&mut self) {
        self.running = false;
        self.waiting.clear();
        self.slew = None;
    }

    /// Beat `beat` of a pulse with `beats` beats sounds at `at`: measure
    /// phase `beat / beats` at `at`. An anchor later than the query time
    /// waits and applies once due.
    pub fn anchor(&mut self, at: Instant, beat: u32, beats: u32) {
        if beats == 0 {
            return;
        }
        let phase = f64::from(beat % beats) / f64::from(beats);
        let i = self.waiting.partition_point(|&(t, _)| t <= at);
        self.waiting.insert(i, (at, phase));
        if self.waiting.len() > MAX_WAITING {
            self.waiting.pop_front();
        }
    }

    /// The measure now lasts `secs`. Re-bases at `now` so the phase does not
    /// jump; the motion continues at the new rate.
    pub fn set_measure(&mut self, now: Instant, secs: f64) {
        if !(secs.is_finite() && secs > 0.0) {
            return;
        }
        if self.running {
            self.apply_due(now);
            // The unfinished correction keeps blending in on its own clock.
            self.base_phase += secs_since(now, self.base_at) / self.measure;
            self.base_at = now;
        }
        self.measure = secs;
    }

    /// Measure phase in `[0, 1)` at `shown`, the shown time. `None` while stopped.
    pub fn phase(&mut self, shown: Instant) -> Option<f64> {
        if !self.running {
            return None;
        }
        self.apply_due(shown);
        let p = self.raw_at(shown).rem_euclid(1.0);
        // rem_euclid can return exactly 1.0 for tiny negative inputs.
        Some(if p >= 1.0 { 0.0 } else { p })
    }

    /// Drops the waiting anchors and the correction in progress. The clock
    /// keeps running on its last base until the next anchor or start.
    pub fn reset(&mut self) {
        self.waiting.clear();
        self.slew = None;
        self.provisional = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn near(a: Option<f64>, b: f64) {
        let a = a.expect("phase");
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    /// A running clock with a 2.0 s measure anchored at `t0` on beat 0 of 4.
    fn anchored() -> (MeasureClock, Instant) {
        let t0 = Instant::now();
        let mut c = MeasureClock::default();
        c.start(t0);
        c.set_measure(t0, 2.0);
        c.anchor(t0, 0, 4);
        (c, t0)
    }

    #[test]
    fn phase_interpolates_between_anchors() {
        let (mut c, t0) = anchored();
        near(c.phase(t0 + ms(500)), 0.25);
    }

    #[test]
    fn phase_wraps() {
        let (mut c, t0) = anchored();
        near(c.phase(t0 + ms(2500)), 0.25);
    }

    #[test]
    fn future_anchor_waits() {
        let (mut c, t0) = anchored();
        c.anchor(t0 + ms(1000), 2, 4);
        near(c.phase(t0 + ms(500)), 0.25);
    }

    #[test]
    fn small_anchor_error_slews() {
        let (mut c, t0) = anchored();
        // At t0+1 s the prediction is 0.5; the anchor says 0.48.
        let at = t0 + ms(1000);
        c.anchor(at, 48, 100);
        let right_after = c.phase(at + ms(1)).unwrap();
        assert!((right_after - 0.5).abs() < 0.02, "{right_after}");
        near(c.phase(at + ms(120)), 0.48 + 0.12 / 2.0);
        near(c.phase(at + ms(500)), 0.48 + 0.5 / 2.0);
    }

    #[test]
    fn large_anchor_error_snaps() {
        let (mut c, t0) = anchored();
        // Prediction at t0+1 s is 0.5; the anchor says 0.1 (error 0.4).
        let at = t0 + ms(1000);
        c.anchor(at, 1, 10);
        near(c.phase(at), 0.1);
    }

    #[test]
    fn tempo_change_keeps_phase_continuous() {
        let (mut c, t0) = anchored();
        let t = t0 + ms(500);
        let before = c.phase(t).unwrap();
        c.set_measure(t, 1.0);
        near(c.phase(t), before);
        near(c.phase(t + ms(250)), before + 0.25);
    }

    #[test]
    fn reference_change_rebases() {
        let (mut c, t0) = anchored();
        let t = t0 + ms(700);
        c.set_measure(t, 1.5);
        // Pulse 0 now has 3 beats; its beat 0 sounds a bit later.
        let at = t + ms(300);
        c.anchor(at, 0, 3);
        let p = c.phase(at).unwrap();
        assert!((0.0..1.0).contains(&p), "{p}");
        let later = at + ms(120);
        near(c.phase(later), 0.12 / 1.5);
    }

    #[test]
    fn extrapolates_without_anchors_from_play_start() {
        let t0 = Instant::now();
        let mut c = MeasureClock::default();
        c.start(t0);
        c.set_measure(t0, 2.0);
        near(c.phase(t0 + ms(1000)), 0.5);
    }

    #[test]
    fn first_anchor_after_start_replaces_the_guess() {
        let t0 = Instant::now();
        let mut c = MeasureClock::default();
        c.start(t0);
        c.set_measure(t0, 2.0);
        // The first beat arrives 200 ms after Play: small error, still snaps.
        let at = t0 + ms(200);
        c.anchor(at, 0, 4);
        near(c.phase(at), 0.0);
        let mut last = c.phase(at).unwrap();
        for i in 1..=12 {
            let p = c.phase(at + ms(10 * i)).unwrap();
            assert!(p >= last, "went backwards: {last} -> {p}");
            last = p;
        }
    }

    #[test]
    fn anchor_after_reset_snaps() {
        let (mut c, t0) = anchored();
        c.reset();
        // Prediction at +1 s is 0.5; a small error that would otherwise slew.
        let at = t0 + ms(1000);
        c.anchor(at, 48, 100);
        near(c.phase(at), 0.48);
    }

    #[test]
    fn stopped_is_none() {
        let t0 = Instant::now();
        let mut c = MeasureClock::default();
        assert_eq!(c.phase(t0), None);
        c.start(t0);
        c.stop();
        assert_eq!(c.phase(t0), None);
    }
}
