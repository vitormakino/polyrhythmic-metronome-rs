//! Offline click onset detection: energy threshold after silence.

/// How a click start is recognised.
#[derive(Clone, Copy, Debug)]
pub struct OnsetParams {
    /// A sample whose magnitude reaches this level is sound.
    pub threshold: f32,
    /// Frames below the threshold that make silence: a sound after at least
    /// this many quiet frames (or at the start) is an onset.
    pub gap: usize,
}

/// Frame index of every onset in `samples`.
pub fn detect_onsets(samples: &[f32], params: OnsetParams) -> Vec<usize> {
    let mut onsets = Vec::new();
    // Start as if silence had been going on.
    let mut quiet = params.gap;
    for (i, &x) in samples.iter().enumerate() {
        if x.abs() >= params.threshold {
            if quiet >= params.gap {
                onsets.push(i);
            }
            quiet = 0;
        } else {
            quiet = quiet.saturating_add(1);
        }
    }
    onsets
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decaying 2 kHz burst that starts at once, like a click.
    fn add_click(buf: &mut [f32], at: usize, gain: f32) {
        let sr = 48_000.0_f32;
        for i in 0..144 {
            let t = i as f32 / sr;
            let decay = (-t / 0.001).exp();
            buf[at + i] += gain * decay * (std::f32::consts::TAU * 2_000.0 * t).sin();
        }
    }

    #[test]
    fn onset_detector_finds_synthetic_clicks() {
        let params = OnsetParams {
            threshold: 0.01,
            gap: 480,
        };
        // Different gains, a 20 ms gap, and one click on frame 0.
        let at = [0usize, 9_600, 10_560, 24_001, 24_900, 47_999];
        let gains = [1.0, 0.3, 0.6, 1.0, 0.3, 0.6];
        let mut buf = vec![0.0_f32; 60_000];
        for (&f, &g) in at.iter().zip(&gains) {
            add_click(&mut buf, f, g);
        }
        let found = detect_onsets(&buf, params);
        assert_eq!(found.len(), at.len(), "found {found:?}");
        for (f, want) in found.iter().zip(at) {
            assert!(
                f.abs_diff(want) <= 1,
                "onset {f} for a click placed at {want}"
            );
        }
    }

    #[test]
    fn a_click_inside_the_tail_of_another_is_not_a_new_onset() {
        let params = OnsetParams {
            threshold: 0.01,
            gap: 480,
        };
        let mut buf = vec![0.0_f32; 4_000];
        add_click(&mut buf, 100, 1.0);
        add_click(&mut buf, 160, 1.0);
        assert_eq!(detect_onsets(&buf, params).len(), 1);
    }
}
