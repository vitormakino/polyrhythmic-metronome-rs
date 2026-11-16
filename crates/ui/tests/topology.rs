use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Vec2;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::{Harness, SnapshotOptions, wgpu::WgpuTestRenderer};
use pm_audio::BeatEvent;
use pm_core::{Pattern, PulseId};
use pm_ui::i18n::Messages;
use pm_ui::state::UiState;
use pm_ui::theme::{self, Theme};
use pm_ui::views::topology::{
    self as topo, TopologyKind, arcs_ball, arcs_radius, cursor_x, displacement, ring_layout,
    topology,
};

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

/// The v1 snapshots are Brazilian Portuguese; they must not change.
fn pt_br(mut state: UiState) -> UiState {
    state.messages = Arc::new(Messages::builtin("pt-BR").expect("pt-BR is embedded"));
    state
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
            muted: false,
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
    let rect = h.get_by_label("Rhythmic topology").rect();
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
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, theme::DARK.card);
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
        pt_br(playing(&[4, 3])),
        360.0,
        &[(0, 0, 100), (1, 0, 100)],
    );
}

#[test]
fn topology_2_en() {
    snapshot(
        "topology_2_en",
        playing(&[4, 3]),
        360.0,
        &[(0, 0, 100), (1, 0, 100)],
    );
}

#[test]
fn topology_6() {
    snapshot(
        "topology_6",
        pt_br(playing(&[4, 3, 5, 7, 6, 8])),
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
        pt_br(playing(&[17, 23, 4, 5, 6, 32, 64, 12, 9, 3, 2, 1])),
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

/// One event for pulse `index` at `at`.
fn event(state: &UiState, index: usize, beat: u32, at: Instant, muted: bool) -> BeatEvent {
    BeatEvent {
        pulse: ids(state)[index],
        beat,
        frame: 0,
        at,
        muted,
    }
}

#[test]
fn phase_uses_visual_offset() {
    let mut state = playing(&[4]);
    state.visual_offset = Duration::from_millis(100);
    let at = Instant::now();
    // Play starts the clock at the shown time, `at`.
    state.advance(at + Duration::from_millis(100), 0.0);
    state.on_beat(&event(&state, 0, 0, at, false));
    state.advance(at + Duration::from_millis(100), 0.0);
    let phase = state.measure_phase().expect("playing");
    assert!(phase.abs() < 1e-6, "{phase}");
    assert_eq!(state.pulse_phase(0).map(|p| p.floor()), Some(0.0));
}

#[test]
fn muted_events_still_anchor() {
    let mut state = playing(&[4]);
    let at = Instant::now();
    state.advance(at, 0.0);
    // 120 BPM, 4 beats: a 2 s measure. Provisionally the clock reads 0.5
    // at +1 s; the muted beat 0 sounding there is far off and snaps it.
    let beat_at = at + Duration::from_secs(1);
    state.on_beat(&event(&state, 0, 0, beat_at, true));
    state.advance(beat_at, 0.0);
    let phase = state.measure_phase().expect("playing");
    assert!(phase.abs() < 1e-6, "{phase}");
    assert_eq!(state.active_beat(ids(&state)[0]), None);
    assert_eq!(state.pending_beats(), 0);
}

#[test]
fn hit_age_counts_from_shown_time() {
    let mut state = playing(&[4]);
    state.visual_offset = Duration::from_millis(50);
    let at = Instant::now();
    state.advance(at, 0.0);
    state.on_beat(&event(&state, 0, 1, at, false));
    state.advance(at + Duration::from_millis(80), 0.0);
    let hit = state.hit(ids(&state)[0]).expect("lit");
    assert_eq!(hit.beat, 1);
    assert!((hit.age - 0.03).abs() < 1e-3, "{}", hit.age);
}

#[test]
fn seed_is_stable() {
    let id = ids(&playing(&[4]))[0];
    assert_eq!(UiState::seed(id, 9), UiState::seed(id, 9));
    assert_ne!(UiState::seed(id, 9), UiState::seed(id, 10));
}

#[test]
fn clear_beats_resets_clock() {
    let mut state = playing(&[4]);
    let at = Instant::now();
    state.advance(at, 0.0);
    // Anchor beat 0 at +1 s (2 s measure), then hide and show again.
    let first = at + Duration::from_secs(1);
    state.on_beat(&event(&state, 0, 0, first, true));
    state.advance(first + Duration::from_millis(100), 0.0);
    state.clear_beats();
    // Beat 1 is phase 0.25; without the reset the clock would only slew
    // towards it, with it the first anchor snaps.
    let next = first + Duration::from_millis(300);
    state.on_beat(&event(&state, 0, 1, next, true));
    state.advance(next, 0.0);
    let phase = state.measure_phase().expect("playing");
    assert!((phase - 0.25).abs() < 1e-6, "{phase}");
}

#[test]
fn kind_round_trips_snake_case() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Doc {
        topology: TopologyKind,
    }
    let names = [
        "circle", "strings", "arcs", "orbits", "sweep", "polygons", "hidden",
    ];
    for (kind, name) in TopologyKind::ALL.into_iter().zip(names) {
        let text = toml::to_string(&Doc { topology: kind }).unwrap();
        assert_eq!(text.trim(), format!("topology = \"{name}\""));
        assert_eq!(
            toml::from_str::<Doc>(&text).unwrap(),
            Doc { topology: kind }
        );
    }
    assert_eq!(TopologyKind::default(), TopologyKind::Circle);
    assert_eq!(UiState::default().topology, TopologyKind::Circle);
}

#[test]
fn all_lists_seven_in_order() {
    use TopologyKind::*;
    assert_eq!(
        TopologyKind::ALL,
        [Circle, Strings, Arcs, Orbits, Sweep, Polygons, Hidden]
    );
}

#[test]
fn shapes_per_kind() {
    use topo::Shape;
    let state = UiState::default();
    for kind in [
        TopologyKind::Circle,
        TopologyKind::Orbits,
        TopologyKind::Sweep,
        TopologyKind::Polygons,
    ] {
        assert_eq!(topo::shape(kind, &state), Shape::Square);
    }
    assert_eq!(topo::shape(TopologyKind::Arcs, &state), Shape::Aspect(2.0));
    assert_eq!(topo::shape(TopologyKind::Hidden, &state), Shape::Hidden);
}

#[test]
fn disabled_pulse_moves_without_hit() {
    let mut state = playing(&[4, 3]);
    let second = ids(&state)[1];
    state.pattern.set_enabled(second, false);
    hit(&mut state, &[(0, 1, 100), (1, 1, 100)]);
    let frame = topo::frame(&state);
    assert!(frame.playing);
    assert_eq!(frame.pulses.len(), 2);
    assert!(frame.pulses[0].hit.is_some());
    assert!(frame.pulses[0].enabled);
    let off = &frame.pulses[1];
    assert!(!off.enabled);
    assert!(off.phase.is_some(), "a disabled pulse keeps moving");
    assert_eq!(off.hit, None, "a disabled pulse fires no effects");
    assert_eq!(frame.pulses[0].seed, UiState::seed(ids(&state)[0], 100));
}

/// One pass of `paint` over `state` in a `size` square; the shapes it made.
fn paint_once(ctx: &egui::Context, state: &UiState, kind: TopologyKind, size: f32) -> usize {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            Vec2::splat(size + 16.0),
        )),
        ..Default::default()
    };
    let output = ctx.run_ui(input, |ui| {
        let rect = egui::Rect::from_min_size(ui.max_rect().min, Vec2::splat(size));
        topo::paint(kind, ui, state, rect);
    });
    let count = output.shapes.len();
    output.drop_without_applying_deltas();
    count
}

/// Hits every pulse at `at` on a non-zero beat and shows them `age` later.
fn hit_all_at(state: &mut UiState, beat: u32, at: Instant, age: Duration) {
    state.advance(at, 0.0);
    for (i, id) in ids(state).into_iter().enumerate() {
        state.on_beat(&BeatEvent {
            pulse: id,
            beat,
            frame: 1000 + i as u64,
            at,
            muted: false,
        });
    }
    state.advance(at + age, 0.0);
}

#[test]
fn paint_survives_pattern_edits_mid_flash() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let mut state = playing(&[4, 3, 5]);
    hit_all_at(&mut state, 2, Instant::now(), Duration::from_millis(30));
    paint_once(&ctx, &state, TopologyKind::Circle, 300.0);
    let first = ids(&state)[0];
    state.pattern.remove(first);
    paint_once(&ctx, &state, TopologyKind::Circle, 300.0);
    // The new reference pulse has fewer beats than the lit one.
    let next = ids(&state)[0];
    state.pattern.set_beats(next, 1);
    paint_once(&ctx, &state, TopologyKind::Circle, 300.0);
    state.advance(state.now() + Duration::from_millis(16), 0.016);
    paint_once(&ctx, &state, TopologyKind::Circle, 300.0);
}

#[test]
fn circle_shape_budget() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let mut state = playing(&[pm_core::MAX_LOADED_BEATS; pm_core::MAX_PULSES]);
    hit_all_at(&mut state, 5, Instant::now(), Duration::from_millis(40));
    assert!(topo::frame(&state).pulses.iter().all(|p| p.hit.is_some()));
    let shapes = paint_once(&ctx, &state, TopologyKind::Circle, 360.0);
    assert!(shapes < 20_000, "{shapes} shapes");
}

#[test]
fn hidden_paints_nothing() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let state = playing(&[4, 3]);
    assert_eq!(paint_once(&ctx, &state, TopologyKind::Hidden, 300.0), 0);
}

/// `state` lit `age` after its hits at a fixed instant, without settling:
/// the young effects show.
fn young_snapshot(name: &str, mut state: UiState, hits: &[(usize, u32, u64)], age: Duration) {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED {name}: no wgpu adapter available");
        return;
    };
    let at = Instant::now();
    state.advance(at, 0.0);
    let ids = ids(&state);
    for &(i, beat, frame) in hits {
        state.on_beat(&BeatEvent {
            pulse: ids[i],
            beat,
            frame,
            at,
            muted: false,
        });
    }
    state.advance(at + age, 0.0);
    let size = 360.0;
    let mut h = Harness::builder()
        .with_size(Vec2::splat(size + 16.0))
        .with_context_setup(theme::install)
        .renderer(renderer)
        .build_ui_state(
            move |ui, state: &mut UiState| {
                let card = state.theme.card;
                ui.painter().rect_filled(ui.max_rect(), 0.0, card);
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
fn topology_circle_hit_dark() {
    young_snapshot(
        "topology_circle_hit_dark",
        pt_br(playing(&[3, 4, 5])),
        &[(0, 1, 100), (1, 1, 100), (2, 2, 100)],
        Duration::from_millis(50),
    );
}

#[test]
fn topology_circle_coincidence_light() {
    let mut state = pt_br(playing(&[3, 4, 5]));
    state.theme = Arc::new(Theme::builtin("light").unwrap());
    young_snapshot(
        "topology_circle_coincidence_light",
        state,
        &[(0, 0, 100), (1, 0, 100), (2, 0, 100)],
        Duration::from_millis(50),
    );
}

#[test]
fn first_hit_after_play_has_no_prev() {
    let mut state = playing(&[4, 3]);
    hit(&mut state, &[(0, 0, 100)]);
    let frame = topo::frame(&state);
    assert_eq!(frame.pulses[0].prev, None, "nothing lit before Play");
    assert_eq!(frame.pulses[1].prev, None, "no hit, no prev");
    // A muted stretch skips beats: prev is the beat really lit, not beat - 1.
    hit(&mut state, &[(0, 1, 200)]);
    hit(&mut state, &[(0, 3, 400)]);
    assert_eq!(topo::frame(&state).pulses[0].prev, Some(1));
    // A prev the pulse no longer has is dropped.
    let id = ids(&state)[0];
    hit(&mut state, &[(0, 0, 500)]);
    state.pattern.set_beats(id, 2);
    assert_eq!(topo::frame(&state).pulses[0].prev, None);
}

#[test]
fn strings_cursor_at_phase() {
    let track = egui::Rect::from_min_max(egui::pos2(10.0, 0.0), egui::pos2(110.0, 50.0));
    assert_eq!(cursor_x(track, 0.0), 10.0);
    assert!((cursor_x(track, 0.25) - 35.0).abs() < 1e-4);
    assert!((cursor_x(track, 1.0) - 110.0).abs() < 1e-4);
}

#[test]
fn strings_pluck_is_zero_at_ends() {
    for pluck in [0.0, 0.25, 0.5, 1.0] {
        for age in [0.0, 0.01, 0.05, 0.3] {
            assert_eq!(displacement(0.0, pluck, age), 0.0, "{pluck} {age}");
            assert_eq!(displacement(1.0, pluck, age), 0.0, "{pluck} {age}");
        }
    }
    // It does move in the middle, and dies away.
    assert!(displacement(0.5, 0.5, 0.03).abs() > 1.0);
    assert!(displacement(0.5, 0.5, 5.0).abs() < 1e-3);
}

#[test]
fn strings_at_rest_when_stopped() {
    let mut state = playing(&[4, 3, 5]);
    hit(&mut state, &[(0, 2, 10), (1, 1, 10)]);
    state.playing = false;
    let frame = topo::frame(&state);
    assert!(!frame.playing);
    for pulse in &frame.pulses {
        assert_eq!(pulse.hit, None);
        let age = pulse.hit.map_or(f32::INFINITY, |h| h.age);
        for k in 0..=10 {
            assert_eq!(displacement(k as f32 / 10.0, 0.5, age), 0.0);
        }
    }
}

#[test]
fn strings_shape_per_pulse_count() {
    use topo::Shape;
    let height = |n: usize| match topo::shape(TopologyKind::Strings, &playing(&vec![4; n])) {
        Shape::Low(h) => h,
        other => panic!("{other:?}"),
    };
    // The card is the drawing (28 + 22 per pulse, at most 200) plus its chrome.
    let chrome = height(3) - 94.0;
    assert!(chrome > 50.0, "{chrome}");
    assert!((height(1) - 50.0 - chrome).abs() < 12.0);
    assert!(height(16) > 200.0 + chrome - 12.0);
    assert!(height(16) < 200.0 + chrome + 12.0);
    assert!(height(8) <= height(16) + 12.0);
}

#[test]
fn strings_shape_budget() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let mut state = playing(&[pm_core::MAX_LOADED_BEATS; pm_core::MAX_PULSES]);
    hit_all_at(&mut state, 5, Instant::now(), Duration::from_millis(40));
    assert!(topo::frame(&state).pulses.iter().all(|p| p.hit.is_some()));
    let shapes = paint_once(&ctx, &state, TopologyKind::Strings, 360.0);
    assert!(shapes < 20_000, "{shapes} shapes");
}

/// `state` lit `age` after its hits, painted as Strings in a card-sized low band.
fn strings_snapshot(name: &str, mut state: UiState, hits: &[(usize, u32, u64)], age: Duration) {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED {name}: no wgpu adapter available");
        return;
    };
    state.topology = TopologyKind::Strings;
    let at = Instant::now();
    state.advance(at, 0.0);
    let ids = ids(&state);
    for &(i, beat, frame) in hits {
        state.on_beat(&BeatEvent {
            pulse: ids[i],
            beat,
            frame,
            at,
            muted: false,
        });
    }
    state.advance(at + age, 0.0);
    let topo::Shape::Low(height) = topo::shape(TopologyKind::Strings, &state) else {
        panic!("strings are low");
    };
    let width = 360.0;
    let mut h = Harness::builder()
        .with_size(Vec2::new(width + 16.0, height + 16.0))
        .with_context_setup(theme::install)
        .renderer(renderer)
        .build_ui_state(
            move |ui, state: &mut UiState| {
                let card = state.theme.card;
                ui.painter().rect_filled(ui.max_rect(), 0.0, card);
                let rect = egui::Rect::from_min_size(ui.max_rect().min, Vec2::new(width, height));
                topo::paint(TopologyKind::Strings, ui, state, rect);
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
fn topology_strings_hit_dark() {
    strings_snapshot(
        "topology_strings_hit_dark",
        pt_br(playing(&[3, 4, 5])),
        &[(0, 1, 100), (1, 2, 100), (2, 0, 100)],
        Duration::from_millis(30),
    );
}

#[test]
fn topology_strings_many_light() {
    let mut state = pt_br(playing(&[17, 23, 4, 5, 6, 32, 64, 12]));
    state.theme = Arc::new(Theme::builtin("light").unwrap());
    strings_snapshot(
        "topology_strings_many_light",
        state,
        &[(0, 5, 100), (1, 11, 100), (5, 31, 100), (6, 40, 100)],
        Duration::from_millis(30),
    );
}

#[test]
fn arcs_ball_on_baseline_at_each_beat() {
    let center = egui::pos2(100.0, 80.0);
    for k in 0..8 {
        let at = arcs_ball(center, 50.0, f64::from(k));
        assert!((at.y - center.y).abs() < 1e-3, "{k}: {at:?}");
    }
    // Halfway through a beat it is at the top of the arc.
    let top = arcs_ball(center, 50.0, 2.5);
    assert!((top.y - (center.y - 50.0)).abs() < 1e-3, "{top:?}");
    assert!((top.x - center.x).abs() < 1e-3, "{top:?}");
}

#[test]
fn arcs_all_left_at_beat_one() {
    let center = egui::pos2(100.0, 80.0);
    for r in [20.0, 50.0, 90.0] {
        let at = arcs_ball(center, r, 0.0);
        assert!((at.x - (center.x - r)).abs() < 1e-3, "{r}: {at:?}");
    }
}

#[test]
fn arcs_alternate_direction() {
    let center = egui::pos2(0.0, 0.0);
    // Even beat: left to right. Odd beat: right to left.
    assert!(arcs_ball(center, 10.0, 0.1).x < -5.0);
    assert!(arcs_ball(center, 10.0, 0.9).x > 5.0);
    assert!(arcs_ball(center, 10.0, 1.1).x > 5.0);
    assert!(arcs_ball(center, 10.0, 1.9).x < -5.0);
    assert!(arcs_ball(center, 10.0, 2.1).x < -5.0);
    // And at constant speed: equal phase steps move it equal angles.
    let a = arcs_ball(center, 10.0, 0.25);
    let b = arcs_ball(center, 10.0, 0.5);
    assert!((a.to_vec2().angle() - b.to_vec2().angle()).abs() > 0.5);
}

#[test]
fn arcs_radii_span_30_to_95_percent() {
    assert!((arcs_radius(0, 3, 100.0) - 95.0).abs() < 1e-4);
    assert!((arcs_radius(2, 3, 100.0) - 30.0).abs() < 1e-4);
    assert!(arcs_radius(0, 1, 100.0) > 90.0);
    assert!(arcs_radius(0, 16, 100.0) > arcs_radius(1, 16, 100.0));
}

#[test]
fn arcs_shape_budget() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let mut state = playing(&[pm_core::MAX_LOADED_BEATS; pm_core::MAX_PULSES]);
    hit_all_at(&mut state, 5, Instant::now(), Duration::from_millis(40));
    assert!(topo::frame(&state).pulses.iter().all(|p| p.hit.is_some()));
    let shapes = paint_once(&ctx, &state, TopologyKind::Arcs, 360.0);
    assert!(shapes < 20_000, "{shapes} shapes");
}

/// `state` lit `age` after its hits, painted as Arcs in a card-sized 2:1 area.
fn arcs_snapshot(name: &str, mut state: UiState, hits: &[(usize, u32, u64)], age: Duration) {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED {name}: no wgpu adapter available");
        return;
    };
    state.topology = TopologyKind::Arcs;
    let at = Instant::now();
    state.advance(at, 0.0);
    let ids = ids(&state);
    for &(i, beat, frame) in hits {
        state.on_beat(&BeatEvent {
            pulse: ids[i],
            beat,
            frame,
            at,
            muted: false,
        });
    }
    state.advance(at + age, 0.0);
    let (width, height) = (360.0, 180.0);
    let mut h = Harness::builder()
        .with_size(Vec2::new(width + 16.0, height + 16.0))
        .with_context_setup(theme::install)
        .renderer(renderer)
        .build_ui_state(
            move |ui, state: &mut UiState| {
                let card = state.theme.card;
                ui.painter().rect_filled(ui.max_rect(), 0.0, card);
                let rect = egui::Rect::from_min_size(ui.max_rect().min, Vec2::new(width, height));
                topo::paint(TopologyKind::Arcs, ui, state, rect);
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
fn topology_arcs_hit_dark() {
    arcs_snapshot(
        "topology_arcs_hit_dark",
        pt_br(playing(&[3, 4, 5])),
        &[(0, 1, 100), (1, 2, 100), (2, 0, 100)],
        Duration::from_millis(40),
    );
}

#[test]
fn topology_arcs_coincidence_light() {
    let mut state = pt_br(playing(&[3, 4, 5]));
    state.theme = Arc::new(Theme::builtin("light").unwrap());
    arcs_snapshot(
        "topology_arcs_coincidence_light",
        state,
        &[(0, 0, 100), (1, 0, 100), (2, 0, 100)],
        Duration::from_millis(50),
    );
}
