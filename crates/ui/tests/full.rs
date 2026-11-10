use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Vec2;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_kittest::{Harness, SnapshotOptions, wgpu::WgpuTestRenderer};
use pm_audio::BeatEvent;
use pm_core::{Intensity, MAX_PULSES, Pattern, SoundType};
use pm_ui::i18n::Messages;
use pm_ui::palette::{Palette, parse};
use pm_ui::state::{PENDING_BEATS_CAP, UiAction, UiState};
use pm_ui::theme::{self, Theme};
use pm_ui::views::full::full_view;

const MIN: Vec2 = Vec2::new(420.0, 640.0);
const MEDIUM: Vec2 = Vec2::new(900.0, 700.0);
const LARGE: Vec2 = Vec2::new(1400.0, 900.0);

/// The v1 snapshots are Brazilian Portuguese; they must not change.
fn pt_br(mut state: UiState) -> UiState {
    state.messages = Arc::new(Messages::builtin("pt-BR").expect("pt-BR is embedded"));
    state
}

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
    let active = state.theme.clone();
    let mut builder = Harness::builder()
        .with_size(size)
        .with_context_setup(move |ctx| {
            theme::install(ctx);
            theme::apply(ctx, &active);
        });
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
    click(&mut h, "Increase BPM");
    assert_eq!(h.state().ui.bpm.get(), 121);
    click(&mut h, "Decrease BPM");
    click(&mut h, "Decrease BPM");
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
    click(&mut h, "Increase BPM");
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
    click(&mut h, "Add pulse");
    assert_eq!(h.state().ui.pattern.pulses.len(), 3);
    assert!(h.state().actions.contains(&UiAction::PatternChanged));
    click(&mut h, "Pulse 3: remove");
    assert_eq!(h.state().ui.pattern.pulses.len(), 2);
    click(&mut h, "Pulse 2: remove");
    assert_eq!(h.state().ui.pattern.pulses.len(), 1);
    assert!(
        h.get_by_label("Pulse 1: remove")
            .accesskit_node()
            .is_disabled()
    );
    click(&mut h, "Pulse 1: remove");
    assert_eq!(h.state().ui.pattern.pulses.len(), 1);
}

#[test]
fn add_pulse_disabled_at_cap() {
    let mut state = UiState::default();
    while state.pattern.add_pulse().is_some() {}
    assert_eq!(state.pattern.pulses.len(), MAX_PULSES);
    let mut h = harness(MEDIUM, state);
    assert!(h.get_by_label("Add pulse").accesskit_node().is_disabled());
    click(&mut h, "Add pulse");
    assert_eq!(h.state().ui.pattern.pulses.len(), MAX_PULSES);
}

#[test]
fn beats_mute_and_sound() {
    let mut h = harness(MEDIUM, UiState::default());
    click(&mut h, "Pulse 1: more beats");
    assert_eq!(h.state().ui.pattern.pulses[0].beats, 5);
    click(&mut h, "Pulse 2: fewer beats");
    assert_eq!(h.state().ui.pattern.pulses[1].beats, 2);
    click(&mut h, "Pulse 2: mute");
    assert!(!h.state().ui.pattern.pulses[1].enabled);
    click(&mut h, "Pulse 2: mute");
    assert!(h.state().ui.pattern.pulses[1].enabled);
    click(&mut h, "Pulse 1: settings");
    click(&mut h, "Pulse 1: sound Bell");
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
    assert!(h.query_by_label("Pulse 1: beat 2").is_none());
    click(&mut h, "Pulse 1: settings");
    assert_eq!(
        h.state().ui.pattern.pulses[0].intensities[1],
        Intensity::Medium
    );
    click(&mut h, "Pulse 1: beat 2");
    assert_eq!(
        h.state().ui.pattern.pulses[0].intensities[1],
        Intensity::Hard
    );
    click(&mut h, "Pulse 1: beat 2");
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
    click(&mut h, "Pin mini");
    assert!(h.state().actions.contains(&UiAction::ToggleMini));
}

#[test]
fn drag_reorders_pulses() {
    let mut h = harness(MEDIUM, UiState::default());
    let first = h.state().ui.pattern.pulses[0].id;
    let from = h.get_by_label("Pulse 1: move").rect().center();
    let to = h.get_by_label("Pulse 2").rect().center();
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
    let one = h.get_by_label("Pulse 1").rect();
    let two = h.get_by_label("Pulse 2").rect();
    let three = h.get_by_label("Pulse 3").rect();
    assert!(
        (one.top() - two.top()).abs() < 0.5 && (two.top() - three.top()).abs() < 0.5,
        "3 pulses must share a row: {one:?} {two:?} {three:?}"
    );
    click(&mut h, "Pulse 1: settings");
    let one_open = h.get_by_label("Pulse 1").rect();
    let two_after = h.get_by_label("Pulse 2").rect();
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
    click(&mut h, "Pulse 1: settings");
    let labels = [
        "BPM",
        "Tap",
        "Add pulse",
        "Play",
        "Pin mini",
        "Rhythmic topology",
        "Settings",
        "no audio output",
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

#[test]
fn footer_shows_output_state_and_underruns() {
    let mut h = harness(MIN, UiState::default());
    assert!(h.query_by_label("no audio output").is_some());
    assert!(
        h.query_by_label_contains("Audio dropouts").is_none(),
        "hidden while there are none"
    );

    let mut state = UiState::default();
    state.device_name = Some("USB Audio Interface".to_owned());
    state.underruns = 3;
    h = harness(MIN, state);
    assert!(h.query_by_label("no audio output").is_none());
    assert!(h.query_by_label("Audio dropouts: 3").is_some());
}

#[test]
fn no_output_shows_the_open_error_on_hover() {
    let mut state = UiState::default();
    state.audio_error = Some("audio output: device busy".to_owned());
    let mut h = harness(MIN, state);
    assert!(h.query_by_label_contains("device busy").is_none());
    h.get_by_label("no audio output").hover();
    for _ in 0..30 {
        h.step();
    }
    assert!(h.query_by_label_contains("device busy").is_some());
}

#[test]
fn settings_popover_sets_visual_offset() {
    let mut h = harness(MEDIUM, UiState::default());
    assert!(
        h.query_by_label("Visual offset (ms)").is_none(),
        "closed at first"
    );
    click(&mut h, "Settings");
    h.get_by_label("Visual offset (ms)")
        .set_value_accesskit(37.0);
    h.run();
    assert_eq!(h.state().ui.visual_offset, Duration::from_millis(37));
    assert_eq!(
        h.state().actions.last(),
        Some(&UiAction::SetVisualOffset(Duration::from_millis(37)))
    );
    // Clamped to the slider's range.
    h.get_by_label("Visual offset (ms)")
        .set_value_accesskit(9000.0);
    h.run();
    assert_eq!(h.state().ui.visual_offset, Duration::from_millis(500));
    // The gear closes it again.
    click(&mut h, "Settings");
    assert!(h.query_by_label("Visual offset (ms)").is_none());
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
    snapshot_in(name, size, many_pulses, true);
}

fn snapshot_in(name: &str, size: Vec2, many_pulses: bool, portuguese: bool) {
    snapshot_themed(
        name,
        size,
        many_pulses,
        portuguese,
        Theme::builtin("default"),
    );
}

fn snapshot_themed(
    name: &str,
    size: Vec2,
    many_pulses: bool,
    portuguese: bool,
    theme: Option<Theme>,
) {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED {name}: no wgpu adapter available");
        return;
    };
    let state = if many_pulses {
        many()
    } else {
        UiState::default()
    };
    let mut state = if portuguese { pt_br(state) } else { state };
    state.theme = Arc::new(theme.expect("theme exists"));
    let mut h = harness_with(size, state, Some(renderer));
    if many_pulses {
        let settings = if portuguese {
            "Pulso 1: ajustes"
        } else {
            "Pulse 1: settings"
        };
        click(&mut h, settings);
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

#[test]
fn switching_the_language_relabels_the_view() {
    let mut h = harness(MEDIUM, UiState::default());
    h.get_by_label("Add pulse");
    h.state_mut().actions.clear();
    h.state_mut().ui.messages = Arc::new(Messages::builtin("pt-BR").unwrap());
    h.run();
    assert!(
        h.state().actions.is_empty(),
        "a language switch emits no UiAction"
    );
    assert!(h.query_by_label("Add pulse").is_none());
    h.get_by_label("Adicionar pulso");
    h.get_by_label("Pulso 1: remover");
}

#[test]
fn english_snapshot_full() {
    snapshot_in("full_medium_default_en", MEDIUM, false, false);
}

#[test]
fn switching_the_theme_repaints_without_actions() {
    let mut h = harness(MEDIUM, UiState::default());
    h.state_mut().actions.clear();
    h.state_mut().ui.theme = Arc::new(Theme::builtin("light").unwrap());
    h.run();
    assert!(
        h.state().actions.is_empty(),
        "a theme switch emits no UiAction"
    );
}

#[test]
fn pulse_colors_keep_their_slot_after_reordering() {
    let light = Theme::builtin("light").unwrap();
    let mut state = UiState::default();
    state.pattern.add_pulse();
    assert_eq!(state.pattern.pulses.len(), 3);
    let third = state.pattern.pulses[2].id;
    state.theme = Arc::new(light.clone());
    let mut h = harness(MEDIUM, state);
    let from = h.get_by_label("Pulse 3: move").rect().center();
    let to = h.get_by_label("Pulse 1").rect().center();
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
    let moved = &h.state().ui.pattern.pulses[0];
    assert_eq!(moved.id, third, "pulse 3 is now on top");
    // The view paints `theme.pulse_color(pulse.color)`: the slot, not the stored v1 rgb.
    assert_eq!(light.pulse_color(moved.color), light.pulses[2]);
    assert_ne!(light.pulses[2], theme::DARK.pulses[2]);
}

#[test]
fn full_medium_light() {
    snapshot_themed(
        "full_medium_light",
        MEDIUM,
        true,
        false,
        Theme::builtin("light"),
    );
}

#[test]
fn full_medium_tokyo() {
    let text = include_str!("fixtures/tokyo-night.toml");
    let (palette, _) = parse(text, &Palette::DEFAULT).expect("fixture parses");
    snapshot_themed(
        "full_medium_tokyo",
        MEDIUM,
        true,
        false,
        Some(Theme::from_palette(&palette)),
    );
}
