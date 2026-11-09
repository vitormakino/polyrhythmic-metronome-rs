//! Compares the clicks found in a recording with the `Timeline` grid.

use pm_audio::soft_clip;
use pm_core::{Bpm, Pattern, SampleBank, Timeline};

use crate::onset::{OnsetParams, detect_onsets};

/// Level (of full scale) a click must reach to count as sound.
pub const THRESHOLD: f32 = 0.01;

/// Silence that separates two sounds: 10 ms.
pub fn gap_frames(sample_rate: u32) -> usize {
    (sample_rate / 100) as usize
}

/// Largest sample difference still counted as equal. The engine sums the
/// voices of a buffer in slot order and the reference in hit order, which
/// can differ by float rounding (about 1e-8) when three or more clicks
/// overlap; a click moved by one frame differs by far more.
pub const TOLERANCE: f32 = 1e-6;

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Sounds the grid predicts inside the recording.
    pub expected: usize,
    /// Sounds found in the recording.
    pub detected: usize,
    /// Expected sounds with a detected one near them.
    pub matched: usize,
    /// Clicks the grid places in the recording.
    pub hits: usize,
    /// Of those, clicks that start inside another click's sound and so have
    /// no onset of their own. Only the sample comparison checks these.
    pub merged_hits: usize,
    /// Frames between the first detected onset and its expected frame.
    pub shift: i64,
    /// Largest `|detected - expected - shift|` over the matched sounds
    /// (secondary: leading onsets only).
    pub max_deviation: u64,
    /// Samples compared (the whole recording).
    pub samples: usize,
    /// Largest `|recorded - reference|` over the recording.
    pub max_sample_diff: f32,
    /// Samples differing by more than `TOLERANCE`.
    pub differing_samples: usize,
}

impl Report {
    /// Every sample equals the reference and the onsets agree, starting at
    /// frame 0.
    pub fn is_exact(&self) -> bool {
        self.differing_samples == 0
            && self.shift == 0
            && self.expected == self.detected
            && self.matched == self.expected
            && self.max_deviation == 0
    }
}

/// One click the grid places: `(frame, pulse index, accent, gain)`.
pub type Hit = (usize, usize, bool, f32);

/// The clicks of `pattern` at `bpm` from frame 0 that start before `frames`.
pub fn grid_hits(sample_rate: u32, bpm: Bpm, pattern: &Pattern, frames: u64) -> Vec<Hit> {
    let mut timeline = Timeline::new(sample_rate, bpm, pattern, 0);
    let mut scratch = Vec::with_capacity(4096);
    let mut out = Vec::new();
    let mut from = 0;
    while from < frames {
        let to = (from + 48_000).min(frames);
        scratch.clear();
        timeline.events_in(pattern, from, to, &mut scratch);
        for hit in &scratch {
            if let Some(index) = pattern.pulses.iter().position(|p| p.id == hit.pulse) {
                out.push((hit.frame as usize, index, hit.accent, hit.gain));
            }
        }
        from = to;
    }
    out
}

/// What the engine must have output: every click of `hits` (clips from
/// `SampleBank`) summed into `frames` samples and soft clipped, with no
/// audio engine in between.
pub fn reference_render(
    sample_rate: u32,
    pattern: &Pattern,
    hits: &[Hit],
    frames: usize,
) -> Vec<f32> {
    let bank = SampleBank::build(sample_rate, pattern);
    let mut reference = vec![0.0_f32; frames];
    for &(frame, index, accent, gain) in hits {
        for (out, s) in reference[frame..].iter_mut().zip(bank.clip(index, accent)) {
            *out += s * gain;
        }
    }
    for x in &mut reference {
        *x = soft_clip(*x);
    }
    reference
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

/// Compares `samples` (a mono recording of `pattern` at `bpm` from frame 0)
/// with the reference render: sample by sample, and by sound onsets.
pub fn analyse(samples: &[f32], sample_rate: u32, bpm: Bpm, pattern: &Pattern) -> Report {
    let hits = grid_hits(sample_rate, bpm, pattern, samples.len() as u64);
    let reference = reference_render(sample_rate, pattern, &hits, samples.len());
    let detected = onsets_inside(samples, sample_rate);
    let expected = onsets_inside(&reference, sample_rate);
    let mut report = Report {
        expected: expected.len(),
        detected: detected.len(),
        hits: hits.len(),
        merged_hits: hits
            .len()
            .saturating_sub(count_sounds(&reference, sample_rate)),
        samples: samples.len(),
        ..Report::default()
    };
    for (a, b) in samples.iter().zip(&reference) {
        let d = (a - b).abs();
        if d.is_nan() || d > TOLERANCE {
            report.differing_samples += 1;
        }
        if d.is_nan() || d > report.max_sample_diff {
            report.max_sample_diff = if d.is_nan() { f32::INFINITY } else { d };
        }
    }
    let (Some(&d0), Some(&e0)) = (detected.first(), expected.first()) else {
        return report;
    };
    report.shift = d0 as i64 - e0 as i64;
    // Both lists are sorted: pair each expected onset with the nearest
    // detected one, if it is within 50 ms.
    let window = i64::from(sample_rate / 20);
    let mut j = 0;
    for &e in &expected {
        let target = e as i64 + report.shift;
        while j + 1 < detected.len() && detected[j + 1] as i64 <= target {
            j += 1;
        }
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

/// Sounds in `samples`, counting the ones cut by the end too.
fn count_sounds(samples: &[f32], sample_rate: u32) -> usize {
    let params = OnsetParams {
        threshold: THRESHOLD,
        gap: gap_frames(sample_rate),
    };
    detect_onsets(samples, params).len()
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
            assert_eq!(report.differing_samples, 0);
            assert!(report.merged_hits > 0, "{report:?}");
        }
    }

    /// The case the onsets alone cannot see: a click that starts inside
    /// another click's sound is moved 5 frames. Its sound still starts on the
    /// grid, so the onset check passes, and the sample comparison fails.
    #[test]
    fn a_shifted_click_inside_a_merged_sound_fails_the_verdict() {
        let (sr, bpm, pattern) = (48_000, Bpm::new(300), seven_eleven());
        let frames = sr as usize * 10;
        let mut samples = render(sr, bpm, &pattern, frames);
        let hits = grid_hits(sr, bpm, &pattern, frames as u64);
        let bank = SampleBank::build(sr, &pattern);
        // A Medium click 5 to 20 ms after another one, alone on its frame.
        let (_, &(frame, index, accent, gain)) = hits
            .iter()
            .enumerate()
            .skip(10)
            .find(|&(n, h)| {
                let prev = hits[n - 1].0;
                h.3 < 1.0 && h.0 > prev + 240 && h.0 < prev + 960 && hits[n + 1].0 > h.0
            })
            .expect("7:11 has close pairs");
        for (i, s) in bank.clip(index, accent).iter().enumerate() {
            samples[frame + i] -= s * gain;
            samples[frame + 5 + i] += s * gain;
        }
        let report = analyse(&samples, sr, bpm, &pattern);
        assert_eq!(report.max_deviation, 0, "the onsets do not see it");
        assert_eq!(report.shift, 0);
        assert!(report.merged_hits > 0);
        assert!(report.differing_samples > 0, "{report:?}");
        assert!(report.max_sample_diff > 0.01, "{report:?}");
        assert!(!report.is_exact());
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
