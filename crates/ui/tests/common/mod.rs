//! Helpers shared by the UI test files.

use std::time::{Duration, Instant};

use pm_audio::BeatEvent;
use pm_ui::state::UiState;

/// Audio frames per second the played beats are numbered with.
const SAMPLE_RATE: f64 = 48_000.0;

/// Plays `state` from Play the way the engine would and stops `after` past
/// beat `beat` of pulse `pulse`, counted from Play (so a pulse of `n` beats
/// has its next measure's beat 1 at `n`).
///
/// Beat 1 sounds at Play. Every pulse's beat `k` sounds at
/// `k / beats` of the measure (pulse 0's beats at the shown tempo), and each
/// is delivered through `on_beat` and `advance` in time order; beats of one
/// instant share an audio frame, as the engine's do. A snapshot taken after
/// this shows each moving part at, or just past, the mark it last lit.
pub fn play_to(state: &mut UiState, pulse: usize, beat: u32, after: Duration) {
    let measure = f64::from(state.pattern.reference_beats().max(1)) * 60.0
        / f64::from(state.shown_bpm().get());
    let beats: Vec<(pm_core::PulseId, u32)> = state
        .pattern
        .pulses
        .iter()
        .map(|p| (p.id, p.beats.max(1)))
        .collect();
    let target = measure * f64::from(beat) / f64::from(beats[pulse].1);
    let mut events = Vec::new();
    for &(id, n) in &beats {
        for k in 0.. {
            let at = measure * (f64::from(k) / f64::from(n));
            if at > target {
                break;
            }
            events.push((at, id, k % n));
        }
    }
    events.sort_by(|a, b| a.0.total_cmp(&b.0));
    let t0 = Instant::now();
    state.playing = true;
    state.advance(t0, 0.0);
    for (secs, pulse, beat) in events {
        let at = t0 + Duration::from_secs_f64(secs);
        state.on_beat(&BeatEvent {
            pulse,
            beat,
            frame: (secs * SAMPLE_RATE).round() as u64,
            at,
            muted: false,
        });
        state.advance(at, 0.0);
    }
    state.advance(t0 + Duration::from_secs_f64(target) + after, 0.0);
}
