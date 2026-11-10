//! Offline click synthesis mirroring the web `scheduleNote` timbres.
//!
//! Clips are rendered once, off the real-time thread; the audio callback only copies them.

use crate::model::{Pattern, SoundType};

/// Peak level every clip is normalised to (unit gain; the caller applies `Intensity::gain`).
const PEAK: f32 = 0.8;

/// One normal and one accent clip per pulse, mono, unit gain.
#[derive(Clone, Debug)]
pub struct SampleBank {
    clips: Vec<[Vec<f32>; 2]>,
}

impl SampleBank {
    pub fn build(sample_rate: u32, pattern: &Pattern) -> SampleBank {
        let clips = pattern
            .pulses
            .iter()
            .map(|p| {
                [
                    render(p.sound, p.frequency_hz, false, sample_rate),
                    render(p.sound, p.frequency_hz, true, sample_rate),
                ]
            })
            .collect();
        SampleBank { clips }
    }

    /// Clip for a pulse index; empty when the index is out of range.
    pub fn clip(&self, pulse_index: usize, accent: bool) -> &[f32] {
        self.clips
            .get(pulse_index)
            .map_or(&[], |c| &c[usize::from(accent)])
    }
}

/// Two-note rising chime (A5 then E6, ~400 ms) for the end of a session.
/// Built once, off the real-time thread; mono, peak 0.5.
pub fn end_chime(sample_rate: u32) -> Vec<f32> {
    const NOTES: [(f32, f32); 2] = [(880.0, 0.0), (1318.5, 0.15)];
    const NOTE_LEN: f32 = 0.25;
    const LEVEL: f32 = 0.5;
    let sr = sample_rate as f32;
    let mut clip = vec![0.0_f32; samples(sr, 0.4)];
    for (freq, start) in NOTES {
        let first = samples(sr, start);
        for (n, s) in clip[first..].iter_mut().enumerate() {
            let t = n as f32 / sr;
            if t >= NOTE_LEN {
                break;
            }
            // 5 ms attack, exponential decay, linear fade over the last 20 ms.
            let attack = (t / 0.005).min(1.0);
            let fade = ((NOTE_LEN - t) / 0.02).min(1.0);
            *s += (core::f32::consts::TAU * freq * t).sin() * attack * (-t * 14.0).exp() * fade;
        }
    }
    let peak = clip.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let k = LEVEL / peak;
        clip.iter_mut().for_each(|s| *s *= k);
    }
    clip
}

/// Tiny deterministic xorshift32 PRNG, white noise in [-1, 1).
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn samples(sr: f32, seconds: f32) -> usize {
    (sr * seconds).round() as usize
}

/// Linear 0 -> 1 in 5 ms, exponential to 0.001 at `end`, then held (as WebAudio does).
fn envelope(t: f32, end: f32) -> f32 {
    const ATTACK: f32 = 0.005;
    if t < ATTACK {
        t / ATTACK
    } else if t < end {
        0.001_f32.powf((t - ATTACK) / (end - ATTACK))
    } else {
        0.001
    }
}

fn render(sound: SoundType, freq: f32, accent: bool, sample_rate: u32) -> Vec<f32> {
    let sr = sample_rate as f32;
    let tau = std::f32::consts::TAU;
    let mut out = match sound {
        SoundType::Sine => {
            let f = if accent { freq * 1.5 } else { freq };
            (0..samples(sr, 0.1))
                .map(|i| {
                    let t = i as f32 / sr;
                    (tau * f * t).sin() * envelope(t, 0.1)
                })
                .collect::<Vec<_>>()
        }
        SoundType::Wood => {
            let f = if accent { 800.0 } else { 600.0 };
            let mut rng = Noise(0x9E37_79B9);
            let noise_len = samples(sr, 0.02);
            (0..samples(sr, 0.05))
                .map(|i| {
                    let t = i as f32 / sr;
                    let tri = 2.0 / std::f32::consts::PI * (tau * f * t).sin().asin();
                    let noise = if i < noise_len {
                        // 0.3 decaying exponentially to 0.01 over 10 ms, then held.
                        let g = 0.3 * (0.01_f32 / 0.3).powf((t / 0.01).min(1.0));
                        rng.next() * g
                    } else {
                        0.0
                    };
                    (tri + noise) * envelope(t, 0.1)
                })
                .collect()
        }
        SoundType::Bell => {
            let f = if accent { freq * 1.5 } else { freq };
            (0..samples(sr, 0.5))
                .map(|i| {
                    let t = i as f32 / sr;
                    let s: f32 = [1.0_f32, 2.5, 3.2]
                        .iter()
                        .map(|h| (tau * f * h * t).sin() / h)
                        .sum();
                    s * envelope(t, 0.4)
                })
                .collect()
        }
        SoundType::Electronic => {
            let f = if accent { 150.0 } else { 100.0 };
            let mut lp = 0.0_f32;
            (0..samples(sr, 0.1))
                .map(|i| {
                    let t = i as f32 / sr;
                    let square = if (tau * f * t).sin() >= 0.0 {
                        1.0
                    } else {
                        -1.0
                    };
                    // Cutoff sweeps 2000 -> 100 Hz exponentially over 50 ms.
                    let fc = 2000.0 * (100.0_f32 / 2000.0).powf((t / 0.05).min(1.0));
                    let a = 1.0 - (-tau * fc / sr).exp();
                    lp += a * (square - lp);
                    lp * envelope(t, 0.1)
                })
                .collect()
        }
    };
    normalize(&mut out);
    out
}

fn normalize(clip: &mut [f32]) {
    let peak = clip.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let k = PEAK / peak;
        clip.iter_mut().for_each(|s| *s *= k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bank_for(sound: SoundType) -> SampleBank {
        let mut pat = Pattern::default();
        pat.pulses[0].sound = sound;
        SampleBank::build(48_000, &pat)
    }

    #[test]
    fn each_clip_nonsilent_and_bounded() {
        let cases = [
            (SoundType::Sine, 4800),
            (SoundType::Wood, 2400),
            (SoundType::Bell, 24_000),
            (SoundType::Electronic, 4800),
        ];
        for (sound, len) in cases {
            let bank = bank_for(sound);
            for accent in [false, true] {
                let clip = bank.clip(0, accent);
                assert_eq!(clip.len(), len, "{sound:?} accent={accent}");
                let rms = (clip.iter().map(|s| s * s).sum::<f32>() / clip.len() as f32).sqrt();
                let peak = clip.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
                assert!(rms > 0.01, "{sound:?} accent={accent} rms={rms}");
                assert!(peak <= 0.8 + 1e-6, "{sound:?} accent={accent} peak={peak}");
            }
        }
    }

    #[test]
    fn end_chime_shape() {
        let sr = 48_000;
        let clip = end_chime(sr);
        let secs = clip.len() as f32 / sr as f32;
        assert!((0.3..=0.5).contains(&secs), "len {secs}s");
        let peak = clip.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1 && peak <= 0.6, "peak {peak}");
        let tail = &clip[clip.len() - (sr as usize / 100)..];
        assert!(tail.iter().all(|s| s.abs() < 0.01), "tail not silent");
    }

    #[test]
    fn accent_differs() {
        for sound in [
            SoundType::Sine,
            SoundType::Wood,
            SoundType::Bell,
            SoundType::Electronic,
        ] {
            let bank = bank_for(sound);
            assert_ne!(bank.clip(0, false), bank.clip(0, true), "{sound:?}");
        }
    }

    #[test]
    fn deterministic() {
        let pat = Pattern::default();
        let a = SampleBank::build(48_000, &pat);
        let b = SampleBank::build(48_000, &pat);
        for i in 0..pat.pulses.len() {
            for accent in [false, true] {
                assert_eq!(a.clip(i, accent), b.clip(i, accent));
            }
        }
    }

    #[test]
    fn sine_pitch() {
        let pat = Pattern::default();
        assert_eq!(pat.pulses[0].sound, SoundType::Sine);
        assert_eq!(pat.pulses[0].frequency_hz, 800.0);
        let bank = SampleBank::build(48_000, &pat);
        let clip = bank.clip(0, false);
        let crossings = clip
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((156..=164).contains(&crossings), "crossings={crossings}");
    }

    #[test]
    fn out_of_range_index_is_empty() {
        let bank = bank_for(SoundType::Sine);
        assert!(bank.clip(99, false).is_empty());
    }
}
