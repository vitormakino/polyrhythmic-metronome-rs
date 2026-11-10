//! The session strip, the mini stage bar and the effective tempo and pulses
//! shown while a session runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Vec2;
use egui::accesskit::{Role, Toggled};
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_kittest::{Harness, SnapshotOptions, wgpu::WgpuTestRenderer};
use pm_audio::BeatEvent;
use pm_core::session::{
    Jump, MuteCycle, OnFinish, PulseState, Ramp, RandomMute, Runner, SessionPlan, SessionProgress,
    Span, Stage, Tempo,
};
use pm_core::{Bpm, MAX_PULSES, Pattern};
use pm_ui::state::{SessionEntry, UiAction, UiState};
use pm_ui::theme::{self, Theme};
use pm_ui::views::full::full_view;
use pm_ui::views::mini::{DEFAULT_SIZE, mini_view};

const MIN: Vec2 = Vec2::new(420.0, 640.0);
const MEDIUM: Vec2 = Vec2::new(900.0, 700.0);
const SAMPLE_RATE: u32 = 48_000;

struct App {
    ui: UiState,
    actions: Vec<UiAction>,
    now: Instant,
    mini: bool,
}

fn harness_with(
    size: Vec2,
    state: UiState,
    mini: bool,
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
            let window = ui.ctx().content_rect();
            ui.scope_builder(egui::UiBuilder::new().max_rect(window), |ui| {
                let actions = if app.mini {
                    mini_view(ui, &mut app.ui, app.now)
                } else {
                    full_view(ui, &mut app.ui, app.now)
                };
                app.actions.extend(actions);
            });
        },
        App {
            ui: state,
            actions: Vec::new(),
            now: Instant::now(),
            mini,
        },
    );
    harness.run();
    harness
}

fn harness(size: Vec2, state: UiState) -> Harness<'static, App> {
    harness_with(size, state, false, None)
}

fn click(h: &mut Harness<'static, App>, label: &str) {
    h.get_by_label(label).click();
    h.run();
}

fn stage(length: Span) -> Stage {
    Stage {
        length,
        tempo: None,
        mute: None,
        random_mute: None,
        pulses: [None; MAX_PULSES],
    }
}

/// Three stages: a ramp, a mute cycle with pulse 2 off, and a timed random mute.
fn plan(name: &str) -> SessionPlan {
    let mut ramp = stage(Span::Bars(8));
    ramp.tempo = Some(Tempo {
        jump: Some(Jump::To(Bpm::new(100))),
        ramp: Some(Ramp {
            change: 2,
            every: Span::Bars(4),
            limit: Some(Bpm::new(140)),
            smooth: false,
        }),
    });
    let mut cycle = stage(Span::Bars(16));
    cycle.mute = Some(MuteCycle {
        play: Span::Bars(4),
        mute: Span::Bars(4),
    });
    cycle.pulses[1] = Some(PulseState::Off);
    let mut random = stage(Span::Seconds(60));
    random.random_mute = Some(RandomMute {
        percent: 20,
        increase: 0,
        every_bars: 1,
        max: 20,
    });
    SessionPlan {
        name: name.to_owned(),
        stages: vec![ramp, cycle, random],
        on_finish: OnFinish::Stop,
        show_muted_visuals: false,
        seed: 1,
    }
}

fn entry(plan: SessionPlan) -> SessionEntry {
    SessionEntry {
        name: plan.name.clone(),
        plan: Arc::new(plan),
    }
}

/// A progress at stage 1, bar 0 of `plan`, from a base of 100 BPM.
fn progress(plan: &SessionPlan) -> SessionProgress {
    let mut runner = Runner::start(plan, Bpm::new(100), SAMPLE_RATE, 0);
    runner.bar(plan, 0);
    runner.progress()
}

/// Playing `plan`, selected, with its progress as `edit` leaves it.
fn running(plan: SessionPlan, edit: impl FnOnce(&mut SessionProgress)) -> UiState {
    let mut p = progress(&plan);
    edit(&mut p);
    let mut state = UiState::default();
    state.bpm = Bpm::new(100);
    state.session = Some(plan.name.clone());
    state.sessions = vec![entry(plan)].into();
    state.playing = true;
    state.session_progress = Some(p);
    state
}

#[test]
fn no_sessions_draws_no_strip() {
    let h = harness(MEDIUM, UiState::default());
    assert!(h.query_by_value("No session").is_none());
    assert!(h.query_by_role(Role::ComboBox).is_none());

    let mut state = UiState::default();
    state.sessions = vec![entry(plan("Warm-up"))].into();
    let h = harness(MEDIUM, state);
    assert!(h.query_by_value("No session").is_some());
}

#[test]
fn selecting_a_session_emits_action() {
    let mut state = UiState::default();
    state.sessions = vec![entry(plan("Alpha")), entry(plan("Beta"))].into();
    let mut h = harness(MEDIUM, state);
    h.get_by_role(Role::ComboBox).click();
    h.run();
    click(&mut h, "Beta");
    assert_eq!(h.state().ui.session.as_deref(), Some("Beta"));
    assert!(h.state().actions.contains(&UiAction::SelectSession));
    assert_eq!(
        h.state().ui.selected_session().map(|s| s.name.as_str()),
        Some("Beta")
    );
    assert!(h.query_by_value("Beta").is_some(), "the selector shows it");
}

#[test]
fn plus_during_session_moves_base_by_delta() {
    let state = running(plan("Ramp"), |p| p.moment.bpm = Bpm::new(110));
    assert_eq!(state.shown_bpm(), Bpm::new(110));
    let mut h = harness(MEDIUM, state);
    click(&mut h, "Increase BPM");
    assert_eq!(
        h.state().actions.last(),
        Some(&UiAction::SetBpm(Bpm::new(111)))
    );
    assert_eq!(h.state().ui.bpm, Bpm::new(101));
    assert_eq!(h.state().ui.shown_bpm(), Bpm::new(111));
}

#[test]
fn tap_during_session_sets_effective() {
    let state = running(plan("Ramp"), |p| p.moment.bpm = Bpm::new(110));
    let mut h = harness(MEDIUM, state);
    let t0 = h.state().now;
    click(&mut h, "Tap");
    // 90 BPM: taps 666.7 ms apart.
    h.state_mut().now = t0 + Duration::from_micros(666_667);
    click(&mut h, "Tap");
    assert_eq!(
        h.state().actions.last(),
        Some(&UiAction::SetBpm(Bpm::new(90)))
    );
    assert_eq!(h.state().ui.bpm, Bpm::new(80));
}

#[test]
fn without_a_running_session_the_base_is_shown_and_set() {
    let mut state = running(plan("Ramp"), |p| p.moment.bpm = Bpm::new(110));
    state.playing = false;
    assert_eq!(state.shown_bpm(), Bpm::new(100));
    let mut actions = Vec::new();
    state.set_shown_bpm(Bpm::new(130), &mut actions);
    assert_eq!(state.bpm, Bpm::new(130));
    assert_eq!(actions, [UiAction::SetBpm(Bpm::new(130))]);

    let mut finished = running(plan("Ramp"), |p| {
        p.moment.bpm = Bpm::new(110);
        p.finished = true;
    });
    // Stopped after finishing: the base again.
    finished.playing = false;
    assert_eq!(finished.shown_bpm(), Bpm::new(100));
    finished.set_shown_bpm(Bpm::new(90), &mut actions);
    assert_eq!(finished.bpm, Bpm::new(90));
}

#[test]
fn continue_after_finish_keeps_effective_tempo() {
    let mut p = plan("Endless");
    p.on_finish = OnFinish::Continue;
    let state = running(p, |p| {
        p.stage = 3;
        p.finished = true;
        p.moment.bpm = Bpm::new(140);
        p.moment.pulses.off = 0b10;
    });
    assert_eq!(state.shown_bpm(), Bpm::new(140));
    assert!(
        !state.pulse_shown_enabled(1),
        "the session mask still applies"
    );
    let mut h = harness(MEDIUM, state);
    assert!(h.query_by_label("Done · continuing").is_some());
    assert!(h.query_by_label_contains("Stage 3/3").is_none());
    click(&mut h, "Increase BPM");
    assert_eq!(
        h.state().actions.last(),
        Some(&UiAction::SetBpm(Bpm::new(141)))
    );
    assert_eq!(h.state().ui.bpm, Bpm::new(101));
    // Stop brings the base back, with the summary.
    click(&mut h, "Stop");
    assert_eq!(h.state().ui.shown_bpm(), Bpm::new(101));
    assert!(h.query_by_label("Done · 2:30 · reached 140 BPM").is_none());
    assert!(h.query_by_label_contains("Done · ").is_some());
}

fn muted_beat_lights(show_muted_visuals: bool) -> bool {
    let mut p = plan("Quiet");
    p.show_muted_visuals = show_muted_visuals;
    let mut state = running(p, |_| {});
    let pulse = state.pattern.pulses[0].id;
    let t0 = Instant::now();
    state.on_beat(&BeatEvent {
        pulse,
        beat: 0,
        frame: 0,
        at: t0,
        muted: true,
    });
    state.advance(t0 + Duration::from_millis(10), 0.016);
    state.active_beat(pulse).is_some()
}

#[test]
fn muted_event_lights_only_with_show_muted_visuals() {
    assert!(!muted_beat_lights(false));
    assert!(muted_beat_lights(true));
}

#[test]
fn off_mask_shows_pulse_card_off() {
    let state = running(plan("Off"), |p| p.moment.pulses.off = 0b10);
    assert!(state.pulse_shown_enabled(0));
    assert!(!state.pulse_shown_enabled(1));
    assert!(state.pattern.pulses[1].enabled, "the setup is not edited");
    let h = harness(MEDIUM, state);
    let toggled = |label: &str| h.get_by_label(label).accesskit_node().toggled();
    assert_eq!(toggled("Pulse 1: mute"), Some(Toggled::False));
    assert_eq!(toggled("Pulse 2: mute"), Some(Toggled::True));
}

#[test]
fn power_on_a_stage_off_pulse_takes_it_over() {
    let state = running(plan("Off"), |p| p.moment.pulses.off = 0b10);
    let mut h = harness(MEDIUM, state);
    click(&mut h, "Pulse 2: mute");
    let ui = &h.state().ui;
    assert_eq!(
        h.state().actions,
        [UiAction::TakeOverPulse(1), UiAction::PatternChanged]
    );
    assert!(ui.pattern.pulses[1].enabled, "as clicked: on");
    assert!(ui.pulse_shown_enabled(1), "shown on at once");
    let toggled = h.get_by_label("Pulse 2: mute").accesskit_node().toggled();
    assert_eq!(toggled, Some(Toggled::False));
}

#[test]
fn power_off_a_forced_on_pulse_takes_it_over() {
    let mut state = running(plan("On"), |p| p.moment.pulses.forced_on = 0b1);
    let id = state.pattern.pulses[0].id;
    state.pattern.set_enabled(id, false);
    let mut h = harness(MEDIUM, state);
    click(&mut h, "Pulse 1: mute");
    let ui = &h.state().ui;
    assert_eq!(
        h.state().actions,
        [UiAction::TakeOverPulse(0), UiAction::PatternChanged]
    );
    assert!(!ui.pattern.pulses[0].enabled, "as clicked: off");
    assert!(!ui.pulse_shown_enabled(0));
}

#[test]
fn power_on_a_pulse_the_stage_leaves_alone_is_a_plain_edit() {
    let state = running(plan("Off"), |p| p.moment.pulses.off = 0b10);
    let mut h = harness(MEDIUM, state);
    click(&mut h, "Pulse 1: mute");
    assert_eq!(h.state().actions, [UiAction::PatternChanged]);
    assert!(!h.state().ui.pattern.pulses[0].enabled);
}

#[test]
fn forced_on_mask_shows_a_disabled_pulse() {
    let mut state = running(plan("On"), |p| p.moment.pulses.forced_on = 0b1);
    let id = state.pattern.pulses[0].id;
    state.pattern.set_enabled(id, false);
    assert!(state.pulse_shown_enabled(0));
    state.playing = false;
    assert!(!state.pulse_shown_enabled(0), "only while running");
}

#[test]
fn missing_pulse_warns_in_strip() {
    let mut p = plan("Five");
    p.stages[0].pulses[4] = Some(PulseState::Muted);
    p.stages[0].pulses[0] = Some(PulseState::Muted);
    let h = harness(MEDIUM, running(p, |_| {}));
    assert!(
        h.query_by_label_contains("pulse 5 is not on screen")
            .is_some()
    );
    assert!(h.query_by_label_contains("pulse 1 muted").is_some());
    assert!(
        h.query_by_label_contains("pulse 1 is not on screen")
            .is_none()
    );
}

#[test]
fn strip_describes_the_running_stage() {
    let h = harness(
        MEDIUM,
        running(plan("Ramp"), |p| {
            p.bars_in_stage = 5;
            p.moment.bpm = Bpm::new(102);
        }),
    );
    assert!(h.query_by_label_contains("Stage 1/3").is_some());
    assert!(h.query_by_label_contains("bar 6/8").is_some());
    assert!(
        h.query_by_label_contains("100 BPM · +2 BPM every 4 bars · up to 140")
            .is_some()
    );
    assert!(h.query_by_label("MUTED").is_none());
    let combo = h.get_by_role(Role::ComboBox);
    assert!(combo.accesskit_node().is_disabled(), "locked while running");
}

#[test]
fn finished_session_shows_the_summary() {
    let mut state = running(plan("Ramp"), |p| {
        p.finished = true;
        p.total_frames = u64::from(SAMPLE_RATE) * 150;
        p.max_bpm = Bpm::new(140);
    });
    state.playing = false;
    let mut h = harness(MEDIUM, state);
    assert!(h.query_by_label("Done · 2:30 · reached 140 BPM").is_some());
    assert!(h.state().ui.session_summary.is_some());
    // The app drops the progress once stopped: the summary stays.
    h.state_mut().ui.session_progress = None;
    h.run();
    assert!(h.query_by_label("Done · 2:30 · reached 140 BPM").is_some());
    // Until the next Play.
    click(&mut h, "Play");
    assert!(h.state().ui.session_summary.is_none());
}

#[test]
fn no_overlap_at_minimum_size_with_session() {
    let mut state = running(plan("Ramp"), |_| {});
    let mut pattern = Pattern::default();
    for _ in 0..4 {
        pattern.add_pulse();
    }
    for (pulse, beats) in pattern.pulses.clone().iter().zip([17, 23, 4, 5, 6, 32]) {
        pattern.set_beats(pulse.id, beats);
    }
    state.pattern = pattern;
    let mut h = harness(MIN, state);
    click(&mut h, "Pulse 1: settings");
    let labels = [
        "BPM",
        "Tap",
        "Add pulse",
        "Stop",
        "Pin mini",
        "Rhythmic topology",
        "Settings",
        "no audio output",
    ];
    let mut rects: Vec<_> = labels
        .iter()
        .map(|l| (l.to_string(), h.get_by_label(l).rect()))
        .collect();
    rects.push(("selector".into(), h.get_by_role(Role::ComboBox).rect()));
    rects.push(("stage".into(), h.get_by_label_contains("Stage 1/3").rect()));
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, MIN);
    for (i, (a, ra)) in rects.iter().enumerate() {
        assert!(screen.contains_rect(*ra), "{a} off screen: {ra:?}");
        for (b, rb) in &rects[i + 1..] {
            assert!(!ra.intersects(*rb), "{a} {ra:?} overlaps {b} {rb:?}");
        }
    }
}

#[test]
fn mini_view_without_session_is_unchanged_and_with_one_still_plays() {
    let mut state = running(plan("Ramp"), |_| {});
    state.playing = false;
    let mut h = harness_with(DEFAULT_SIZE, state, true, None);
    click(&mut h, "Play");
    assert!(h.state().actions.contains(&UiAction::TogglePlay));
}

/// Per ruling R3: without a wgpu adapter the snapshot is skipped, not failed.
fn renderer() -> Option<WgpuTestRenderer> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(WgpuTestRenderer::new);
    std::panic::set_hook(previous);
    result.ok()
}

fn snapshot(name: &str, size: Vec2, mini: bool, theme: &str, state: UiState) {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED {name}: no wgpu adapter available");
        return;
    };
    let mut state = state;
    state.theme = Arc::new(Theme::builtin(theme).expect("theme exists"));
    let mut h = harness_with(size, state, mini, Some(renderer));
    h.remove_cursor();
    h.run();
    h.snapshot_options(
        name,
        &SnapshotOptions::new().threshold(2.0).max_failed_pixels(100),
    );
}

/// Stage 1, bar 6 of 8, ramped to 102.
fn ramping() -> UiState {
    running(plan("Speed builder"), |p| {
        p.bars_in_stage = 5;
        p.moment.bpm = Bpm::new(102);
    })
}

#[test]
fn session_running_dark() {
    snapshot("session_running_dark", MEDIUM, false, "default", ramping());
}

#[test]
fn session_running_light() {
    snapshot("session_running_light", MEDIUM, false, "light", ramping());
}

#[test]
fn session_muted_dark() {
    let p = plan("Speed builder");
    let mut runner = Runner::start(&p, Bpm::new(100), SAMPLE_RATE, 0);
    // Stage 2, bar 5 of 16: the second half of the 4 + 4 cycle.
    for bar in 0..13u64 {
        runner.bar(&p, bar * u64::from(SAMPLE_RATE) * 2);
    }
    let progress = runner.progress();
    assert_eq!((progress.stage, progress.bars_in_stage), (2, 4));
    assert!(progress.moment.silent);
    snapshot(
        "session_muted_dark",
        MEDIUM,
        false,
        "default",
        running(p, |p| *p = progress),
    );
}

#[test]
fn session_done_dark() {
    let mut state = running(plan("Speed builder"), |p| {
        p.stage = 3;
        p.finished = true;
        p.total_frames = u64::from(SAMPLE_RATE) * 150;
        p.max_bpm = Bpm::new(140);
    });
    state.playing = false;
    snapshot("session_done_dark", MEDIUM, false, "default", state);
}

#[test]
fn mini_session() {
    let state = running(plan("Speed builder"), |p| {
        p.stage = 2;
        p.bars_in_stage = 8;
    });
    snapshot("mini_session", DEFAULT_SIZE, true, "default", state);
}
