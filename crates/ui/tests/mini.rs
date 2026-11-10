use std::sync::Arc;
use std::time::Instant;

use egui::{Event, Modifiers, MouseWheelUnit, TouchPhase, Vec2};
use egui_kittest::kittest::Queryable as _;
use egui_kittest::{Harness, SnapshotOptions, wgpu::WgpuTestRenderer};
use pm_core::{Bpm, Pattern};
use pm_ui::i18n::Messages;
use pm_ui::state::{UiAction, UiState, WindowMode};
use pm_ui::theme;
use pm_ui::views::mini::{DEFAULT_SIZE, MIN_SIZE, mini_view};

/// The v1 snapshots are Brazilian Portuguese; they must not change.
fn pt_br(mut state: UiState) -> UiState {
    state.messages = Arc::new(Messages::builtin("pt-BR").expect("pt-BR is embedded"));
    state
}

struct App {
    ui: UiState,
    actions: Vec<UiAction>,
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
            let window = ui.ctx().content_rect();
            ui.scope_builder(egui::UiBuilder::new().max_rect(window), |ui| {
                let actions = mini_view(ui, &mut app.ui, Instant::now());
                app.actions.extend(actions);
            });
        },
        App {
            ui: state,
            actions: Vec::new(),
        },
    );
    harness.run();
    harness
}

fn harness(state: UiState) -> Harness<'static, App> {
    harness_with(DEFAULT_SIZE, state, None)
}

fn click(h: &mut Harness<'static, App>, label: &str) {
    h.get_by_label(label).click();
    h.run();
}

fn wheel(h: &mut Harness<'static, App>, notches: f32) {
    let center = h.get_by_label("BPM").rect().center();
    h.hover_at(center);
    h.run();
    h.event(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: Vec2::new(0.0, notches),
        phase: TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    });
    h.run();
}

#[test]
fn mini_bpm_and_play() {
    let mut h = harness(UiState::default());
    click(&mut h, "Increase BPM");
    assert_eq!(h.state().ui.bpm.get(), 121);
    click(&mut h, "Decrease BPM");
    click(&mut h, "Decrease BPM");
    assert_eq!(h.state().ui.bpm.get(), 119);
    assert!(h.state().actions.contains(&UiAction::SetBpm(Bpm::new(119))));

    // Three notches up: +3 in one go.
    wheel(&mut h, 3.0);
    assert_eq!(h.state().ui.bpm.get(), 122);
    wheel(&mut h, -2.0);
    assert_eq!(h.state().ui.bpm.get(), 120);
    assert_eq!(
        h.state().actions.last(),
        Some(&UiAction::SetBpm(Bpm::new(120)))
    );

    click(&mut h, "Play");
    assert!(h.state().ui.playing);
    assert_eq!(h.state().actions.last(), Some(&UiAction::TogglePlay));
    assert!(h.query_by_label("Stop").is_some());
    click(&mut h, "Stop");
    assert!(!h.state().ui.playing);
}

#[test]
fn mini_drag_changes_bpm_per_4px() {
    let mut h = harness(UiState::default());
    let from = h.get_by_label("BPM").rect().center();
    h.hover_at(from);
    h.run();
    h.event(Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run();
    // egui only starts a drag after ~6 px; step past that first, then
    // 20 px up in 5 px moves must be exactly +5.
    h.event(Event::PointerMoved(from - Vec2::new(0.0, 10.0)));
    h.run();
    let start = h.state().ui.bpm.get();
    for i in 1..=4 {
        h.event(Event::PointerMoved(
            from - Vec2::new(0.0, 10.0 + 5.0 * i as f32),
        ));
        h.run();
    }
    h.event(Event::PointerButton {
        pos: from - Vec2::new(0.0, 30.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    assert_eq!(h.state().ui.bpm.get(), start + 5);
    assert!(!h.state().actions.contains(&UiAction::TogglePlay));
}

#[test]
fn mini_has_no_pulse_controls() {
    let h = harness(UiState::default());
    assert!(h.query_by_label("Pulse 1: remove").is_none());
    assert!(h.query_by_label("ADD PULSE").is_none());
    assert!(h.query_by_label("Pulse 1: settings").is_none());
}

#[test]
fn expand_emits_toggle() {
    let mut h = harness(UiState::default());
    click(&mut h, "Expand");
    assert_eq!(h.state().actions, vec![UiAction::ToggleMini]);
    assert_eq!(h.state().ui.mode, WindowMode::Full);
}

#[test]
fn min_size_fits_every_control() {
    let h = harness_with(MIN_SIZE, UiState::default(), None);
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, MIN_SIZE);
    for label in ["Decrease BPM", "Increase BPM", "Play", "Expand", "BPM"] {
        let r = h.get_by_label(label).rect();
        assert!(window.contains_rect(r), "{label} {r:?} outside {window:?}");
    }
}

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

fn snapshot(name: &str, state: UiState) {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED {name}: no wgpu adapter available");
        return;
    };
    let mut h = harness_with(DEFAULT_SIZE, state, Some(renderer));
    h.remove_cursor();
    h.run();
    h.snapshot_options(
        name,
        &SnapshotOptions::new().threshold(2.0).max_failed_pixels(100),
    );
}

#[test]
fn mini_default() {
    snapshot("mini_default", pt_br(UiState::default()));
}

#[test]
fn mini_many() {
    snapshot("mini_many", pt_br(many()));
}

#[test]
fn mini_default_en() {
    snapshot("mini_default_en", UiState::default());
}
