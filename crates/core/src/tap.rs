//! Tap tempo: BPM from the mean interval of the last four taps.

use std::time::Duration;

use crate::model::Bpm;

/// Number of most recent taps used for the average.
const WINDOW: usize = 4;

/// Tap-tempo estimator. Timestamps are injected by the caller; no clock is read here.
#[derive(Clone, Debug, Default)]
pub struct TapTempo {
    taps: Vec<Duration>,
}

impl TapTempo {
    /// Registers a tap at `at` and returns the estimated tempo, if one can be derived
    /// and its rounded value lies within `Bpm::MIN..=Bpm::MAX`.
    pub fn tap(&mut self, at: Duration) -> Option<Bpm> {
        if self.taps.len() == WINDOW {
            self.taps.remove(0);
        }
        self.taps.push(at);

        let (first, last) = (*self.taps.first()?, *self.taps.last()?);
        let intervals = self.taps.len() - 1;
        if intervals == 0 {
            return None;
        }
        let mean_secs = last.saturating_sub(first).as_secs_f64() / intervals as f64;
        if mean_secs <= 0.0 {
            return None;
        }
        let bpm = (60.0 / mean_secs).round();
        if (f64::from(Bpm::MIN)..=f64::from(Bpm::MAX)).contains(&bpm) {
            Some(Bpm::new(bpm as i32))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn single_tap_none() {
        let mut t = TapTempo::default();
        assert_eq!(t.tap(ms(0)), None);
    }

    #[test]
    fn two_taps_500ms_gives_120() {
        let mut t = TapTempo::default();
        t.tap(ms(0));
        assert_eq!(t.tap(ms(500)), Some(Bpm::new(120)));
    }

    #[test]
    fn uses_last_four_taps() {
        let mut t = TapTempo::default();
        let mut last = None;
        for at in [0, 1000, 1500, 2000, 2500] {
            last = t.tap(ms(at));
        }
        assert_eq!(last, Some(Bpm::new(120)));
    }

    #[test]
    fn out_of_range_ignored() {
        let mut t = TapTempo::default();
        t.tap(ms(0));
        assert_eq!(t.tap(ms(2000)), None); // 30 BPM
        let mut fast = TapTempo::default();
        fast.tap(ms(0));
        assert_eq!(fast.tap(ms(100)), None); // 600 BPM
    }
}
