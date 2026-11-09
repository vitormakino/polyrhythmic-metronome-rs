//! Compares the clicks found in a recording with the `Timeline` grid.

use pm_core::{Bpm, Pattern, SampleBank, Timeline};

use crate::onset::{OnsetParams, detect_onsets};

/// Level (of full scale) a click must reach to count as sound.
pub const THRESHOLD: f32 = 0.01;

/// Silence that separates two sounds: 10 ms.
pub fn gap_frames(sample_rate: u32) -> usize {
    (sample_rate / 100) as usize
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Sounds the grid predicts inside the recording.
    pub expected: usize,
    /// Sounds found in the recording.
    pub detected: usize,
    /// Expected sounds with a detected one near them.
    pub matched: usize,
    /// Frames between the first detected onset and its expected frame (the
    /// constant offset removed from every deviation below).
    pub shift: i64,
    /// Largest `|detected - expected - shift|` over the matched sounds.
    pub max_deviation: u64,
}

impl Report {
    pub fn is_exact(&self) -> bool {
        self.expected == self.detected && self.matched == self.expected && self.max_deviation == 0
    }
}

/// Onsets of the signal the grid predicts: every click of `pattern` at `bpm`
/// from frame 0 (frames from `Timeline`, clips from `SampleBank`, gains from
/// the hits) summed into a buffer of `frames`, with no audio engine in
/// between, then run through the same detector as the recording.
pub fn expected_onsets(sample_rate: u32, bpm: Bpm, pattern: &Pattern, frames: u64) -> Vec<usize> {
    let bank = SampleBank::build(sample_rate, pattern);
    let mut timeline = Timeline::new(sample_rate, bpm, pattern, 0);
    let mut reference = vec![0.0_f32; frames as usize];
    let mut hits = Vec::with_capacity(4096);
    let mut from = 0;
    while from < frames {
        let to = (from + 48_000).min(frames);
        hits.clear();
        timeline.events_in(pattern, from, to, &mut hits);
        for hit in &hits {
            let Some(index) = pattern.pulses.iter().position(|p| p.id == hit.pulse) else {
                continue;
            };
            let start = hit.frame as usize;
            for (out, s) in reference[start..]
                .iter_mut()
                .zip(bank.clip(index, hit.accent))
            {
                *out += s * hit.gain;
            }
        }
        from = to;
    }
    onsets_inside(&reference, sample_rate)
}

/// Onsets of `samples`, except sounds too close to the end to be complete.
fn onsets_inside(samples: &[f32], sample_rate: u32) -> Vec<usize> {
    let params = OnsetParams {
        threshold: THRESHOLD,
        gap: gap_frames(sample_rate),
    };
    detect_onsets(samples, params)
        .into_iter()
        .filter(|&o| o as u64 + 2 * params.gap as u64 <= samples.len() as u64)
        .collect()
}

/// Detects the sounds in `samples` (a mono recording of `pattern` at `bpm`
/// from frame 0) and compares them with the grid.
pub fn analyse(samples: &[f32], sample_rate: u32, bpm: Bpm, pattern: &Pattern) -> Report {
    let detected = onsets_inside(samples, sample_rate);
    let expected = expected_onsets(sample_rate, bpm, pattern, samples.len() as u64);
    let mut report = Report {
        expected: expected.len(),
        detected: detected.len(),
        ..Report::default()
    };
    let (Some(&d0), Some(&e0)) = (detected.first(), expected.first()) else {
        return report;
    };
    report.shift = d0 as i64 - e0 as i64;
    // Both lists are sorted: pair each expected onset with the nearest
    // detected one, if it is within half the shortest distance between hits.
    let window = (sample_rate / 20) as i64;
    let mut j = 0;
    for &e in &expected {
        while j + 1 < detected.len() && detected[j + 1] as i64 <= e as i64 + report.shift {
            j += 1;
        }
        let target = e as i64 + report.shift;
        let best = [j, j + 1]
            .into_iter()
            .filter_map(|k| detected.get(k))
            .map(|&d| (d as i64 - target).abs())
            .min()
            .unwrap_or(i64::MAX);
        if best <= window {
            report.matched += 1;
            report.max_deviation = report.max_deviation.max(best as u64);
        }
    }
    report
}

/// 7 against 11, both wood blocks (short, so clicks separate), accent on
/// each "one".
pub fn seven_eleven() -> Pattern {
    use pm_core::{Intensity, SoundType};
    let mut p = Pattern::default();
    for (pulse, beats) in p.pulses.iter_mut().zip([7, 11]) {
        pulse.beats = beats;
        pulse.sound = SoundType::Wood;
        pulse.intensities = vec![Intensity::Medium; beats as usize];
        pulse.intensities[0] = Intensity::Hard;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use pm_audio::{Engine, EngineInit};
    use pm_core::MusicalPosition;
    use std::time::Duration;

    fn render(sample_rate: u32, bpm: Bpm, pattern: &Pattern, frames: usize) -> Vec<f32> {
        let init = EngineInit {
            pattern: pattern.clone(),
            bpm,
            resume: Some(MusicalPosition::start_of(0)),
        };
        let (mut engine, _handle) = Engine::new(sample_rate, 1, init);
        let mut out = vec![0.0; frames];
        for block in out.chunks_mut(512) {
            engine.render(block, Duration::ZERO);
        }
        out
    }

    #[test]
    fn engine_output_matches_the_grid_exactly() {
        for sr in [44_100, 48_000] {
            let pattern = seven_eleven();
            let bpm = Bpm::new(300);
            let samples = render(sr, bpm, &pattern, sr as usize * 20);
            let report = analyse(&samples, sr, bpm, &pattern);
            assert!(report.expected > 50, "{report:?}");
            assert!(report.is_exact(), "sr {sr}: {report:?}");
            assert_eq!(report.shift, 0);
        }
    }

    #[test]
    fn a_click_off_the_grid_is_measured() {
        let pattern = seven_eleven();
        let bpm = Bpm::new(300);
        let mut samples = render(48_000, bpm, &pattern, 48_000 * 10);
        // Delay everything after 5 s by 3 frames.
        let cut = 48_000 * 5;
        samples.splice(cut..cut, [0.0; 3]);
        let report = analyse(&samples, 48_000, bpm, &pattern);
        assert!(!report.is_exact());
        assert_eq!(report.max_deviation, 3, "{report:?}");
    }
}
