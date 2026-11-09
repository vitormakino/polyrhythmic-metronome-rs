use std::time::{Duration, Instant};

use egui::Vec2;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::{Harness, SnapshotOptions, wgpu::WgpuTestRenderer};
use pm_audio::BeatEvent;
use pm_core::{Pattern, PulseId};
use pm_ui::state::UiState;
use pm_ui::theme;
use pm_ui::views::topology::{ring_layout, topology};

fn pattern_of(beats: &[u32]) -> Pattern {
    let mut pattern = Pattern::default();
    while pattern.pulses.len() < beats.len() {
        pattern.add_pulse();
    }
    while pattern.pulses.len() > beats.len() {
        let id = pattern.pulses.last().unwrap().id;
        pattern.remove(id);
    }
    for (pulse, n) in pattern.pulses.clone().iter().zip(beats) {
        pattern.set_beats(pulse.id, *n);
    }
    pattern
}

fn playing(beats: &[u32]) -> UiState {
    let mut state = UiState::default();
    state.pattern = pattern_of(beats);
    state.playing = true;
    state
}

fn ids(state: &UiState) -> Vec<PulseId> {
    state.pattern.pulses.iter().map(|p| p.id).collect()
}

/// Delivers one beat per `(pulse index, beat, frame)` and lights it.
fn hit(state: &mut UiState, hits: &[(usize, u32, u64)]) {
    let at = Instant::now();
    let ids = ids(state);
    for &(i, beat, frame) in hits {
        state.on_beat(&BeatEvent {
            pulse: ids[i],
            beat,
            frame,
            at,
        });
    }
    state.advance(at + Duration::from_millis(1), 0.001);
}

fn settle(state: &mut UiState) {
    let mut now = Instant::now() + Duration::from_secs(1);
    for _ in 0..120 {
        now += Duration::from_millis(16);
        state.advance(now, 0.016);
    }
}

#[test]
fn all_pulses_get_positive_radius() {
    for size in [120.0_f32, 200.0, 450.0, 800.0] {
        let half = size / 2.0;
        for count in 1..=12 {
            let rings = ring_layout(count, 8, size);
            assert_eq!(rings.len(), count);
            for ring in &rings {
                assert!(ring.radius > 0.0);
                assert!(ring.radius >= 0.3 * half - 1e-3, "{count}: {ring:?}");
                assert!(ring.radius <= 0.95 * half + 1e-3, "{count}: {ring:?}");
                assert!(ring.dot_radius > 0.0);
            }
            // Distinct rings, outermost first, evenly spaced.
            let gaps: Vec<f32> = rings
                .windows(2)
                .map(|w| w[0].radius - w[1].radius)
                .collect();
            for gap in &gaps {
                assert!(*gap > 0.0, "{count} rings must not collapse: {rings:?}");
                assert!((gap - gaps[0]).abs() < 1e-3);
            }
        }
    }
}

#[test]
fn dots_never_overlap() {
    for size in [200.0_f32, 320.0, 450.0, 800.0] {
        for count in [1, 2, 6, 12, 16] {
            for beats in 1..=64u32 {
                let rings = ring_layout(count, beats, size);
                for (i, ring) in rings.iter().enumerate() {
                    if beats >= 2 {
                        let chord = 2.0 * ring.radius * (std::f32::consts::PI / beats as f32).sin();
                        assert!(
                            chord >= 2.0 * ring.dot_radius,
                            "size {size} rings {count} beats {beats}: chord {chord} dot {}",
                            ring.dot_radius
                        );
                    }
                    if let Some(next) = rings.get(i + 1) {
                        let gap = ring.radius - next.radius;
                        assert!(gap >= ring.dot_radius + next.dot_radius);
                    }
                    assert!(ring.dot_radius <= 3.5);
                }
            }
        }
    }
}

#[test]
fn polygon_hidden_when_many_beats() {
    assert!(ring_layout(1, 3, 300.0)[0].show_polygon);
    assert!(ring_layout(1, 24, 300.0)[0].show_polygon);
    assert!(!ring_layout(1, 25, 300.0)[0].show_polygon);
    assert!(!ring_layout(1, 2, 300.0)[0].show_polygon);
}

#[test]
fn coincidence_only_when_all_on_one() {
    let mut state = playing(&[4, 3]);
    assert!(!state.coincidence(), "nothing lit yet");

    hit(&mut state, &[(0, 0, 1000), (1, 0, 1000)]);
    assert!(state.coincidence());

    hit(&mut state, &[(0, 1, 2000)]);
    assert!(!state.coincidence(), "one pulse left beat 0");

    hit(&mut state, &[(0, 0, 3000)]);
    assert!(
        !state.coincidence(),
        "the other still holds an older beat 0"
    );

    hit(&mut state, &[(1, 0, 3000)]);
    assert!(state.coincidence());

    hit(&mut state, &[(0, 0, 4000), (1, 0, 4001)]);
    assert!(!state.coincidence(), "different frames");

    let mut off = playing(&[4, 3]);
    hit(&mut off, &[(0, 0, 10), (1, 0, 10)]);
    off.playing = false;
    assert!(!off.coincidence(), "never while stopped");
}

#[test]
fn coincidence_ignores_disabled_pulses() {
    let mut state = playing(&[4, 3]);
    let second = ids(&state)[1];
    state.pattern.set_enabled(second, false);
    hit(&mut state, &[(0, 0, 500)]);
    assert!(state.coincidence());

    let mut all_off = playing(&[4]);
    let only = ids(&all_off)[0];
    hit(&mut all_off, &[(0, 0, 1)]);
    all_off.pattern.set_enabled(only, false);
    assert!(!all_off.coincidence(), "no enabled pulse, no coincidence");
}

#[test]
fn dot_springs_up_and_back() {
    let mut state = playing(&[4]);
    let id = ids(&state)[0];
    assert_eq!(state.dot(id, 0), 0.0);

    hit(&mut state, &[(0, 2, 7)]);
    // Springs towards 1 within a few frames, and the neighbours stay idle.
    let mut now = Instant::now() + Duration::from_secs(1);
    for _ in 0..30 {
        now += Duration::from_millis(16);
        state.advance(now, 0.016);
    }
    assert!(
        (state.dot(id, 2) - 1.0).abs() < 0.05,
        "{}",
        state.dot(id, 2)
    );
    assert_eq!(state.dot(id, 1), 0.0);
    assert_eq!(state.dot(id, 3), 0.0);

    hit(&mut state, &[(0, 3, 8)]);
    settle(&mut state);
    assert!(state.dot(id, 2).abs() < 0.05);
    assert!((state.dot(id, 3) - 1.0).abs() < 0.05);
    assert!(!state.animating());
}

#[test]
fn disabled_pulse_dots_stay_idle() {
    let mut state = playing(&[4]);
    let id = ids(&state)[0];
    hit(&mut state, &[(0, 1, 7)]);
    state.pattern.set_enabled(id, false);
    settle(&mut state);
    assert!(state.dot(id, 1).abs() < 0.05);
}

#[test]
fn dots_follow_beat_count_edits() {
    let mut state = playing(&[4]);
    let id = ids(&state)[0];
    hit(&mut state, &[(0, 3, 7)]);
    state.pattern.set_beats(id, 2);
    settle(&mut state);
    assert_eq!(state.dot(id, 3), 0.0, "beat 3 no longer exists");
    state.pattern.set_beats(id, 6);
    settle(&mut state);
    assert_eq!(state.dot(id, 5), 0.0);
}

fn harness(state: UiState, size: f32) -> Harness<'static, UiState> {
    let mut h = Harness::builder()
        .with_size(Vec2::splat(size + 16.0))
        .with_context_setup(theme::install)
        .build_ui_state(
            move |ui, state: &mut UiState| topology(ui, state, size),
            state,
        );
    h.run();
    h
}

#[test]
fn topology_node_is_the_square() {
    let h = harness(UiState::default(), 300.0);
    let rect = h.get_by_label("Topologia rítmica").rect();
    assert!((rect.width() - 300.0).abs() < 0.5);
    assert!((rect.height() - 300.0).abs() < 0.5);
}

/// Per ruling R3: without a wgpu adapter the snapshot is skipped, not failed.
fn renderer() -> Option<WgpuTestRenderer> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(WgpuTestRenderer::new);
    std::panic::set_hook(previous);
    result.ok()
}

fn snapshot(name: &str, mut state: UiState, size: f32, hits: &[(usize, u32, u64)]) {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED {name}: no wgpu adapter available");
        return;
    };
    hit(&mut state, hits);
    settle(&mut state);
    // Keep the hits lit but let the flash settle, like a steady frame.
    let mut h = Harness::builder()
        .with_size(Vec2::splat(size + 16.0))
        .with_context_setup(theme::install)
        .renderer(renderer)
        .build_ui_state(
            move |ui, state: &mut UiState| {
                ui.painter().rect_filled(ui.max_rect(), 0.0, theme::CARD);
                topology(ui, state, size);
            },
            state,
        );
    h.run();
    h.snapshot_options(
        name,
        &SnapshotOptions::new().threshold(2.0).max_failed_pixels(100),
    );
}

#[test]
fn topology_2() {
    snapshot(
        "topology_2",
        playing(&[4, 3]),
        360.0,
        &[(0, 0, 100), (1, 0, 100)],
    );
}

#[test]
fn topology_6() {
    snapshot(
        "topology_6",
        playing(&[4, 3, 5, 7, 6, 8]),
        360.0,
        &[
            (0, 1, 100),
            (1, 2, 100),
            (2, 3, 100),
            (3, 5, 100),
            (4, 0, 100),
            (5, 7, 100),
        ],
    );
}

#[test]
fn topology_many() {
    snapshot(
        "topology_many",
        playing(&[17, 23, 4, 5, 6, 32, 64, 12, 9, 3, 2, 1]),
        360.0,
        &[
            (0, 5, 100),
            (1, 0, 100),
            (2, 1, 100),
            (5, 31, 100),
            (6, 40, 100),
        ],
    );
}
