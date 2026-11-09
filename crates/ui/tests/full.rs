use std::time::{Duration, Instant};

use egui::Vec2;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_kittest::{Harness, SnapshotOptions, wgpu::WgpuTestRenderer};
use pm_audio::BeatEvent;
use pm_core::{Intensity, MAX_PULSES, Pattern, SoundType};
use pm_ui::state::{PENDING_BEATS_CAP, UiAction, UiState};
use pm_ui::theme;
use pm_ui::views::full::full_view;

const MIN: Vec2 = Vec2::new(420.0, 640.0);
const MEDIUM: Vec2 = Vec2::new(900.0, 700.0);
const LARGE: Vec2 = Vec2::new(1400.0, 900.0);

struct App {
    ui: UiState,
    actions: Vec<UiAction>,
    now: Instant,
}

fn app(ui: UiState) -> App {
    App {
        ui,
        actions: Vec::new(),
        now: Instant::now(),
    }
}

fn harness_with(
    size: Vec2,
    state: UiState,
    renderer: Option<WgpuTestRenderer>,
) -> Harness<'static, App> {
    let mut builder = Harness::builder()
        .with_size(size)
        .with_context_setup(theme::install);
    if let Some(renderer) = renderer {
        builder = builder.renderer(renderer);
    }
    let mut harness = builder.build_ui_state(
        |ui, app: &mut App| {
            // Like the app's frameless central panel: the view gets the whole window,
            // not the harness's 8 pt margin.
            let window = ui.ctx().content_rect();
            ui.scope_builder(egui::UiBuilder::new().max_rect(window), |ui| {
                let actions = full_view(ui, &mut app.ui, app.now);
                app.actions.extend(actions);
            });
        },
        app(state),
    );
    harness.run();
    harness
}

fn harness(size: Vec2, state: UiState) -> Harness<'static, App> {
    harness_with(size, state, None)
}

fn click(harness: &mut Harness<'static, App>, label: &str) {
    harness.get_by_label(label).click();
    harness.run();
}

#[test]
fn bpm_buttons_and_slider() {
    let mut h = harness(MEDIUM, UiState::default());
    click(&mut h, "Aumentar BPM");
    assert_eq!(h.state().ui.bpm.get(), 121);
    click(&mut h, "Diminuir BPM");
    click(&mut h, "Diminuir BPM");
    assert_eq!(h.state().ui.bpm.get(), 119);
    assert!(
        h.state()
            .actions
            .contains(&UiAction::SetBpm(pm_core::Bpm::new(119)))
    );

    h.get_by_label("BPM").set_value_accesskit(200.0);
    h.run();
    assert_eq!(h.state().ui.bpm.get(), 200);
    assert_eq!(
        h.state().actions.last(),
        Some(&UiAction::SetBpm(pm_core::Bpm::new(200)))
    );
}

#[test]
fn bpm_stays_in_range() {
    let mut state = UiState::default();
    state.bpm = pm_core::Bpm::new(300);
    let mut h = harness(MEDIUM, state);
    click(&mut h, "Aumentar BPM");
    assert_eq!(h.state().ui.bpm.get(), 300);
}

#[test]
fn tap_sets_bpm() {
    let mut h = harness(MEDIUM, UiState::default());
    let t0 = h.state().now;
    click(&mut h, "Tap");
    h.state_mut().now = t0 + Duration::from_millis(600);
    click(&mut h, "Tap");
    assert_eq!(h.state().ui.bpm.get(), 100);
    let taps = h
        .state()
        .actions
        .iter()
        .filter(|a| matches!(a, UiAction::Tap(_)))
        .count();
    assert_eq!(taps, 2);
    assert!(
        h.state()
            .actions
            .contains(&UiAction::SetBpm(pm_core::Bpm::new(100)))
    );
}

#[test]
fn add_and_remove_pulse() {
    let mut h = harness(MEDIUM, UiState::default());
    click(&mut h, "Adicionar pulso");
    assert_eq!(h.state().ui.pattern.pulses.len(), 3);
    assert!(h.state().actions.contains(&UiAction::PatternChanged));
    click(&mut h, "Pulso 3: remover");
    assert_eq!(h.state().ui.pattern.pulses.len(), 2);
    click(&mut h, "Pulso 2: remover");
    assert_eq!(h.state().ui.pattern.pulses.len(), 1);
    assert!(
        h.get_by_label("Pulso 1: remover")
            .accesskit_node()
            .is_disabled()
    );
    click(&mut h, "Pulso 1: remover");
    assert_eq!(h.state().ui.pattern.pulses.len(), 1);
}

#[test]
fn add_pulse_disabled_at_cap() {
    let mut state = UiState::default();
    while state.pattern.add_pulse().is_some() {}
    assert_eq!(state.pattern.pulses.len(), MAX_PULSES);
    let mut h = harness(MEDIUM, state);
    assert!(
        h.get_by_label("Adicionar pulso")
            .accesskit_node()
            .is_disabled()
    );
    click(&mut h, "Adicionar pulso");
    assert_eq!(h.state().ui.pattern.pulses.len(), MAX_PULSES);
}

#[test]
fn beats_mute_and_sound() {
    let mut h = harness(MEDIUM, UiState::default());
    click(&mut h, "Pulso 1: aumentar batidas");
    assert_eq!(h.state().ui.pattern.pulses[0].beats, 5);
    click(&mut h, "Pulso 2: diminuir batidas");
    assert_eq!(h.state().ui.pattern.pulses[1].beats, 2);
    click(&mut h, "Pulso 2: silenciar");
    assert!(!h.state().ui.pattern.pulses[1].enabled);
    click(&mut h, "Pulso 2: silenciar");
    assert!(h.state().ui.pattern.pulses[1].enabled);
    click(&mut h, "Pulso 1: ajustes");
    click(&mut h, "Pulso 1: som Bell");
    assert_eq!(h.state().ui.pattern.pulses[0].sound, SoundType::Bell);
    let changes = h
        .state()
        .actions
        .iter()
        .filter(|a| **a == UiAction::PatternChanged)
        .count();
    assert_eq!(changes, 5);
}

#[test]
fn intensity_cycles() {
    let mut h = harness(MEDIUM, UiState::default());
    assert!(h.query_by_label("Pulso 1: batida 2").is_none());
    click(&mut h, "Pulso 1: ajustes");
    assert_eq!(
        h.state().ui.pattern.pulses[0].intensities[1],
        Intensity::Medium
    );
    click(&mut h, "Pulso 1: batida 2");
    assert_eq!(
        h.state().ui.pattern.pulses[0].intensities[1],
        Intensity::Hard
    );
    click(&mut h, "Pulso 1: batida 2");
    assert_eq!(
        h.state().ui.pattern.pulses[0].intensities[1],
        Intensity::Mute
    );
    assert!(h.state().actions.contains(&UiAction::PatternChanged));
}

#[test]
fn play_emits_action() {
    let mut h = harness(MEDIUM, UiState::default());
    click(&mut h, "Play");
    assert!(h.state().actions.contains(&UiAction::TogglePlay));
    assert!(h.state().ui.playing);
    assert!(h.query_by_label("Stop").is_some());
}

#[test]
fn pin_mini_emits_action() {
    let mut h = harness(MEDIUM, UiState::default());
    click(&mut h, "Fixar mini");
    assert!(h.state().actions.contains(&UiAction::ToggleMini));
}

#[test]
fn drag_reorders_pulses() {
    let mut h = harness(MEDIUM, UiState::default());
    let first = h.state().ui.pattern.pulses[0].id;
    let from = h.get_by_label("Pulso 1: mover").rect().center();
    let to = h.get_by_label("Pulso 2").rect().center();
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    for i in 1..=4 {
        h.hover_at(from + (to - from) * (i as f32 / 4.0));
        h.run();
    }
    h.drop_at(to);
    h.run();
    assert_eq!(h.state().ui.pattern.pulses[1].id, first);
    assert!(h.state().actions.contains(&UiAction::PatternChanged));
}

#[test]
fn visual_waits_for_offset() {
    let mut state = UiState::default();
    state.playing = true;
    state.visual_offset = Duration::from_millis(200);
    let pulse = state.pattern.pulses[0].id;
    let t0 = Instant::now();
    state.on_beat(&BeatEvent {
        pulse,
        beat: 0,
        frame: 0,
        at: t0,
    });
    state.advance(t0 + Duration::from_millis(150), 0.016);
    assert_eq!(state.active_beat(pulse), None);
    state.advance(t0 + Duration::from_millis(210), 0.016);
    assert_eq!(state.active_beat(pulse), Some(0));
}

#[test]
fn queued_beats_are_bounded_and_keep_the_newest() {
    let mut state = UiState::default();
    state.playing = true;
    let pulse = state.pattern.pulses[0].id;
    let t0 = Instant::now();
    let total = PENDING_BEATS_CAP as u64 * 4;
    // A hidden window: the app keeps forwarding beats but never advances.
    for i in 0..total {
        state.on_beat(&BeatEvent {
            pulse,
            beat: i as u32,
            frame: i,
            at: t0 + Duration::from_millis(i),
        });
    }
    assert_eq!(state.pending_beats(), PENDING_BEATS_CAP);
    state.advance(t0 + Duration::from_secs(3600), 0.016);
    assert_eq!(state.pending_beats(), 0);
    assert_eq!(state.active_beat(pulse), Some(total as u32 - 1));
}

#[test]
fn beats_are_ignored_while_stopped() {
    let mut state = UiState::default();
    let pulse = state.pattern.pulses[0].id;
    let t0 = Instant::now();
    state.on_beat(&BeatEvent {
        pulse,
        beat: 1,
        frame: 0,
        at: t0,
    });
    state.advance(t0 + Duration::from_millis(10), 0.016);
    assert_eq!(state.active_beat(pulse), None);
    assert_eq!(state.pending_beats(), 0);
}

#[test]
fn settings_does_not_stretch_neighbors() {
    let mut state = UiState::default();
    state.pattern.add_pulse();
    let mut h = harness(MEDIUM, state);
    let one = h.get_by_label("Pulso 1").rect();
    let two = h.get_by_label("Pulso 2").rect();
    let three = h.get_by_label("Pulso 3").rect();
    assert!(
        (one.top() - two.top()).abs() < 0.5 && (two.top() - three.top()).abs() < 0.5,
        "3 pulses must share a row: {one:?} {two:?} {three:?}"
    );
    click(&mut h, "Pulso 1: ajustes");
    let one_open = h.get_by_label("Pulso 1").rect();
    let two_after = h.get_by_label("Pulso 2").rect();
    assert!(
        one_open.height() > one.height() + 20.0,
        "settings did not open"
    );
    assert_eq!(two_after.height(), two.height());
    assert_eq!(two_after.top(), two.top());
}

#[test]
fn no_overlap_at_minimum_size() {
    let mut h = harness(MIN, many());
    click(&mut h, "Pulso 1: ajustes");
    let labels = [
        "BPM",
        "Tap",
        "Adicionar pulso",
        "Play",
        "Fixar mini",
        "Topologia rítmica",
    ];
    let rects: Vec<_> = labels
        .iter()
        .map(|l| (l, h.get_by_label(l).rect()))
        .collect();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, MIN);
    for (i, (a, ra)) in rects.iter().enumerate() {
        assert!(screen.contains_rect(*ra), "{a} off screen: {ra:?}");
        for (b, rb) in &rects[i + 1..] {
            assert!(!ra.intersects(*rb), "{a} {ra:?} overlaps {b} {rb:?}");
        }
    }
}

/// 6 pulses of 17, 23, 4, 5, 6, 32 beats.
fn many() -> UiState {
    let mut state = UiState::default();
    let mut pattern = Pattern::default();
    for _ in 0..4 {
        pattern.add_pulse();
    }
    for (pulse, beats) in pattern.pulses.clone().iter().zip([17, 23, 4, 5, 6, 32]) {
        pattern.set_beats(pulse.id, beats);
    }
    state.pattern = pattern;
    state
}

/// Per ruling R3: without a wgpu adapter the snapshot is skipped, not failed.
fn renderer() -> Option<WgpuTestRenderer> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(WgpuTestRenderer::new);
    std::panic::set_hook(previous);
    result.ok()
}

fn snapshot(name: &str, size: Vec2, many_pulses: bool) {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED {name}: no wgpu adapter available");
        return;
    };
    let state = if many_pulses {
        many()
    } else {
        UiState::default()
    };
    let mut h = harness_with(size, state, Some(renderer));
    if many_pulses {
        click(&mut h, "Pulso 1: ajustes");
    }
    h.remove_cursor();
    h.run();
    h.snapshot_options(
        name,
        &SnapshotOptions::new().threshold(2.0).max_failed_pixels(100),
    );
}

#[test]
fn full_min_default() {
    snapshot("full_min_default", MIN, false);
}

#[test]
fn full_min_many() {
    snapshot("full_min_many", MIN, true);
}

#[test]
fn full_medium_default() {
    snapshot("full_medium_default", MEDIUM, false);
}

#[test]
fn full_medium_many() {
    snapshot("full_medium_many", MEDIUM, true);
}

#[test]
fn full_large_default() {
    snapshot("full_large_default", LARGE, false);
}

#[test]
fn full_large_many() {
    snapshot("full_large_many", LARGE, true);
}
