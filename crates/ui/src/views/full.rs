//! The full window: tempo card, pulse cards, topology and the play button.
//!
//! Layout (spec §6): the topology and the play button are always visible and
//! only the pulse list scrolls; every pulse card has its own height, so
//! opening one card's settings never resizes its neighbours. Wide windows use
//! two columns like the web app (controls left, topology right); narrow ones
//! stack everything in one column.

use std::time::Instant;

use egui::epaint::Shadow;
use egui::style::HandleShape;
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Painter, Pos2, Rect, Response, Role, ScrollArea,
    Sense, Slider, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};
use pm_core::{Bpm, Intensity, MAX_PULSES, Pulse, PulseId, SoundType};

use super::topology::topology;
use super::widgets;
use crate::state::{UiAction, UiState, WindowMode};
use crate::theme;

/// Below this width everything stacks in one column.
const WIDE_MIN: f32 = 760.0;
/// Content never gets wider than this (web: `max-w-5xl` plus some air).
const MAX_CONTENT_WIDTH: f32 = 1100.0;
const GAP: f32 = 12.0;
const CARD_GAP: f32 = 12.0;
/// Narrowest pulse card that still fits its header icons and title.
const CARD_MIN_WIDTH: f32 = 132.0;
const MAX_COLUMNS: usize = 3;
const PULSE_RADIUS: u8 = 16;
/// Room around the cards inside the scroll area for their glow and the scroll bar.
const LIST_INSET: f32 = 6.0;
const SCROLL_BAR_ROOM: f32 = 8.0;

/// zinc-300, text on idle pill buttons.
const TEXT_SOFT: Color32 = Color32::from_rgb(0xd4, 0xd4, 0xd8);
/// zinc-200, hovered icons.
const TEXT_HOT: Color32 = Color32::from_rgb(0xe4, 0xe4, 0xe7);
/// Between zinc-800 and zinc-700 (Tailwind's `zinc-750` in the web app).
const HOVER_FILL: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x33);

/// Draws the full window into `ui`, editing `state`; returns what the app must forward.
/// `now` timestamps taps.
pub fn full_view(ui: &mut Ui, state: &mut UiState, now: Instant) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let screen = ui.max_rect();
    ui.painter().rect_filled(screen, 0.0, theme::BG);
    let mut layout = Layout::new(screen);

    top_bar(ui, layout.top, state, layout.wide);
    bpm_card(ui, layout.bpm, state, now, layout.wide, &mut actions);
    topology_card(ui, layout.topology, state);
    pulses_header(ui, layout.header, state, &mut actions);

    // After the header, which may have added a pulse.
    let grid = Grid::new(ui.painter(), state, layout.list.width());
    if layout.wide {
        // Like the web, Play follows the cards; it only stays pinned to the
        // bottom once the list has to scroll.
        let shift = (layout.list.height() - grid.height).max(0.0);
        layout.list.max.y -= shift;
        layout.play = layout.play.translate(vec2(0.0, -shift));
    }
    pulse_list(ui, layout.list, &grid, state, &mut actions);
    play_button(ui, layout.play, state, layout.wide, &mut actions);
    actions
}

/// Where each block of the window goes.
struct Layout {
    wide: bool,
    top: Rect,
    bpm: Rect,
    topology: Rect,
    header: Rect,
    list: Rect,
    play: Rect,
}

fn row(top: f32, height: f32, left: f32, right: f32) -> Rect {
    Rect::from_min_max(pos2(left, top), pos2(right, top + height))
}

impl Layout {
    fn new(screen: Rect) -> Self {
        let wide = screen.width() >= WIDE_MIN;
        let pad = if wide { 24.0 } else { 16.0 };
        let mut content = screen.shrink(pad);
        if content.width() > MAX_CONTENT_WIDTH {
            let extra = content.width() - MAX_CONTENT_WIDTH;
            content = content.shrink2(vec2(extra / 2.0, 0.0));
        }
        let (l, r) = (content.left(), content.right());
        if wide {
            let top = row(content.top(), 32.0, l, r);
            let body_top = top.bottom() + 16.0;
            let column_gap = 24.0;
            let left_width = ((content.width() - column_gap) * 7.0 / 12.0).floor();
            let (ll, lr) = (l, l + left_width);
            let bpm = row(body_top, 184.0, ll, lr);
            let play = row(content.bottom() - 64.0, 64.0, ll, lr);
            let header = row(bpm.bottom() + 20.0, 28.0, ll, lr);
            let list =
                Rect::from_min_max(pos2(ll, header.bottom() + 8.0), pos2(lr, play.top() - 12.0));
            let right = Rect::from_min_max(pos2(lr + column_gap, body_top), content.max);
            let side = right.width().min(right.height());
            let topology = Rect::from_min_size(
                pos2(right.center().x - side / 2.0, right.top()),
                Vec2::splat(side),
            );
            Layout {
                wide,
                top,
                bpm,
                topology,
                header,
                list,
                play,
            }
        } else {
            let top = row(content.top(), 24.0, l, r);
            let bpm = row(top.bottom() + GAP, 144.0, l, r);
            let play = row(content.bottom() - 56.0, 56.0, l, r);
            let header_h = 28.0;
            // What is left between the tempo card and the play button goes
            // half to the topology, half to the pulse list.
            let flex = (play.top() - GAP - (bpm.bottom() + GAP) - GAP - header_h - 8.0).max(0.0);
            let side = (flex * 0.5).min(content.width()).floor();
            let topology = Rect::from_min_size(
                pos2(content.center().x - side / 2.0, bpm.bottom() + GAP),
                Vec2::splat(side),
            );
            let header = row(topology.bottom() + GAP, header_h, l, r);
            let list =
                Rect::from_min_max(pos2(l, header.bottom() + 8.0), pos2(r, play.top() - GAP));
            Layout {
                wide,
                top,
                bpm,
                topology,
                header,
                list,
                play,
            }
        }
    }
}

fn pulse_color(pulse: &Pulse) -> Color32 {
    let [r, g, b] = pulse.color;
    Color32::from_rgb(r, g, b)
}

/// `idle` normally, brighter while hovered.
fn idle_color(resp: &Response, idle: Color32) -> Color32 {
    if resp.hovered() { TEXT_HOT } else { idle }
}

fn top_bar(ui: &Ui, rect: Rect, state: &UiState, wide: bool) {
    let painter = ui.painter_at(rect);
    let icon = if wide { 20.0 } else { 16.0 };
    let cy = rect.center().y;
    widgets::zap(
        &painter,
        pos2(rect.left() + icon / 2.0, cy),
        icon,
        theme::ACCENT,
    );
    let title = widgets::text(
        &painter,
        pos2(rect.left() + icon + 8.0, cy),
        Align2::LEFT_CENTER,
        "POLYRHYTHMIC METRONOME",
        theme::font_bold(if wide { 20.0 } else { 14.0 }),
        theme::TEXT,
        0.0,
    );
    let mut right = rect.right();
    if state.underruns > 0 {
        let text = format!("{} underruns", state.underruns);
        let r = widgets::text(
            &painter,
            pos2(right, cy),
            Align2::RIGHT_CENTER,
            &text,
            theme::font_mono(10.0),
            theme::PALETTE[4],
            0.0,
        );
        right = r.left() - 10.0;
    }
    if let Some(name) = &state.device_name {
        let max = right - title.right() - 16.0;
        if max > 24.0 {
            widgets::elided(
                &painter,
                pos2(right, cy),
                Align2::RIGHT_CENTER,
                name,
                theme::font_mono(10.0),
                theme::TEXT_FAINT,
                max,
            );
        }
    }
}

fn bpm_card(
    ui: &mut Ui,
    rect: Rect,
    state: &mut UiState,
    now: Instant,
    wide: bool,
    actions: &mut Vec<UiAction>,
) {
    let painter = ui.painter().clone();
    theme::paint_card(&painter, rect, theme::BORDER, 0.0);
    theme::paint_top_accent(&painter, rect);
    let pad = if wide { 24.0 } else { 16.0 };
    let inner = rect.shrink2(vec2(pad, if wide { 20.0 } else { 14.0 }));

    // Header: caption left, mini window and TAP right.
    let header_y = inner.top() + 12.0;
    widgets::text(
        &painter,
        pos2(inner.left(), header_y),
        Align2::LEFT_CENTER,
        "TEMPO / BPM",
        theme::font_mono(11.0),
        theme::TEXT_MUTED,
        2.0,
    );
    let tap_rect = Rect::from_center_size(pos2(inner.right() - 24.0, header_y), vec2(48.0, 22.0));
    let tap = widgets::button(ui, tap_rect, ui.make_persistent_id("tap"), "Tap", true);
    let tap_shape = if tap.is_pointer_button_down_on() {
        tap_rect.shrink(1.0)
    } else {
        tap_rect
    };
    painter.rect(
        tap_shape,
        4.0,
        theme::with_alpha(theme::ACCENT, 0.1),
        Stroke::new(1.0, theme::with_alpha(theme::ACCENT, 0.2)),
        StrokeKind::Inside,
    );
    widgets::text(
        &painter,
        tap_shape.center(),
        Align2::CENTER_CENTER,
        "TAP",
        theme::font_mono_medium(10.0),
        theme::with_alpha(theme::ACCENT, if tap.hovered() { 1.0 } else { 0.8 }),
        1.0,
    );
    if tap.clicked() {
        let (at, bpm) = state.tap(now);
        actions.push(UiAction::Tap(at));
        if let Some(bpm) = bpm {
            state.bpm = bpm;
            actions.push(UiAction::SetBpm(bpm));
        }
    }
    let pin_rect = Rect::from_center_size(
        pos2(tap_rect.left() - 8.0 - 13.0, header_y),
        Vec2::splat(26.0),
    );
    let pin = widgets::button(
        ui,
        pin_rect,
        ui.make_persistent_id("mini"),
        "Fixar mini",
        true,
    );
    if pin.hovered() {
        painter.rect_filled(pin_rect, 6.0, theme::BORDER);
    }
    widgets::mini_window(
        &painter,
        pin_rect.center(),
        16.0,
        idle_color(&pin, theme::TEXT_MUTED),
    );
    if pin.clicked() {
        state.mode = match state.mode {
            WindowMode::Full => WindowMode::Mini,
            WindowMode::Mini => WindowMode::Full,
        };
        actions.push(UiAction::ToggleMini);
    }

    // Big number between - and +.
    let slider_h = 20.0;
    let number_area = Rect::from_min_max(
        pos2(inner.left(), header_y + 12.0),
        pos2(inner.right(), inner.bottom() - slider_h - 4.0),
    );
    let font = theme::font_light(if wide { 72.0 } else { 56.0 });
    let digits = widgets::text_width(&painter, "300", font.clone(), -2.0);
    let center = number_area.center();
    widgets::text(
        &painter,
        center,
        Align2::CENTER_CENTER,
        &state.bpm.get().to_string(),
        font,
        Color32::WHITE,
        -2.0,
    );
    let step = if wide { 48.0 } else { 40.0 };
    let offset = digits / 2.0 + 24.0 + step / 2.0;
    for (label, delta) in [("Diminuir BPM", -1), ("Aumentar BPM", 1)] {
        let c = center + vec2(offset * delta as f32, 0.0);
        let r = Rect::from_center_size(c, Vec2::splat(step));
        let resp = widgets::button(ui, r, ui.make_persistent_id(label), label, true);
        if resp.hovered() {
            painter.circle_filled(c, step / 2.0, theme::BORDER);
        }
        let at_limit = (delta < 0 && state.bpm.get() == Bpm::MIN)
            || (delta > 0 && state.bpm.get() == Bpm::MAX);
        let color = if at_limit {
            theme::DOT_INACTIVE
        } else {
            idle_color(&resp, theme::TEXT_DIM)
        };
        if delta < 0 {
            widgets::minus(&painter, c, 24.0, color);
        } else {
            widgets::plus(&painter, c, 24.0, color);
        }
        if resp.clicked() {
            set_bpm(state, i32::from(state.bpm.get()) + delta, actions);
        }
    }

    // Slider along the bottom.
    let slider_rect = Rect::from_min_max(
        pos2(inner.left(), inner.bottom() - slider_h),
        inner.right_bottom(),
    );
    let mut value = i32::from(state.bpm.get());
    let changed = ui
        .scope_builder(UiBuilder::new().max_rect(slider_rect), |ui| {
            style_slider(ui, slider_rect.width());
            ui.name_inputs("BPM", |ui| {
                ui.add(
                    Slider::new(&mut value, i32::from(Bpm::MIN)..=i32::from(Bpm::MAX))
                        .show_value(false)
                        .trailing_fill(true)
                        .handle_shape(HandleShape::Circle),
                )
            })
            .changed()
        })
        .inner;
    if changed {
        set_bpm(state, value, actions);
    }
}

fn set_bpm(state: &mut UiState, value: i32, actions: &mut Vec<UiAction>) {
    let bpm = Bpm::new(value);
    if bpm != state.bpm {
        state.bpm = bpm;
        actions.push(UiAction::SetBpm(bpm));
    }
}

/// Web look for the range input: thin zinc-800 rail, orange fill and knob.
fn style_slider(ui: &mut Ui, width: f32) {
    let style = ui.style_mut();
    style.spacing.slider_width = width;
    style.spacing.slider_rail_height = 4.0;
    style.spacing.interact_size.y = 20.0;
    style.visuals.selection.bg_fill = theme::ACCENT;
    let w = &mut style.visuals.widgets;
    // The rail is painted with `inactive.bg_fill`.
    w.inactive.bg_fill = theme::BORDER;
    w.inactive.corner_radius = CornerRadius::same(2);
    w.inactive.fg_stroke = Stroke::new(1.0, theme::ACCENT);
    w.hovered.fg_stroke = Stroke::new(1.0, theme::ACCENT_LIGHT);
    w.active.fg_stroke = Stroke::new(1.0, theme::ACCENT_LIGHT);
    w.hovered.bg_fill = theme::ACCENT;
    w.active.bg_fill = theme::ACCENT;
}

fn topology_card(ui: &mut Ui, rect: Rect, state: &UiState) {
    if rect.width() < 1.0 {
        return;
    }
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(theme::CARD_RADIUS),
        theme::with_alpha(theme::CARD, 0.4),
        Stroke::new(1.0, theme::with_alpha(theme::BORDER, 0.5)),
        StrokeKind::Inside,
    );
    let mut inner = rect.shrink(rect.width() * 0.06);
    if rect.width() >= 220.0 {
        widgets::text(
            painter,
            pos2(rect.center().x, rect.top() + 28.0),
            Align2::CENTER_CENTER,
            "TOPOLOGIA RÍTMICA",
            theme::font_mono(9.0),
            theme::TEXT_FAINT,
            3.6,
        );
        inner = inner.shrink(12.0);
    }
    let side = inner.width().min(inner.height());
    let square = Rect::from_center_size(rect.center(), Vec2::splat(side));
    ui.scope_builder(UiBuilder::new().max_rect(square), |ui| {
        topology(ui, state, side);
    });
}

fn pulses_header(ui: &Ui, rect: Rect, state: &mut UiState, actions: &mut Vec<UiAction>) {
    let painter = ui.painter();
    widgets::text(
        painter,
        pos2(rect.left() + 4.0, rect.center().y),
        Align2::LEFT_CENTER,
        "PULSOS ATIVOS",
        theme::font_mono(10.0),
        theme::TEXT_MUTED,
        2.0,
    );
    let font = theme::font_semibold(10.0);
    let label = "ADICIONAR PULSO";
    let text_w = widgets::text_width(painter, label, font.clone(), 0.5);
    // padding 12 | plus 12 | gap 6 | text | padding 12
    let w = 12.0 + 12.0 + 6.0 + text_w + 12.0;
    let r = Rect::from_min_max(pos2(rect.right() - w, rect.top()), rect.right_bottom());
    let enabled = state.pattern.pulses.len() < MAX_PULSES;
    let resp = widgets::button(
        ui,
        r,
        ui.make_persistent_id("add-pulse"),
        "Adicionar pulso",
        enabled,
    );
    let fill = if resp.hovered() {
        theme::BORDER
    } else {
        theme::CARD
    };
    painter.rect(
        r,
        r.height() / 2.0,
        fill,
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
    let color = if enabled {
        TEXT_SOFT
    } else {
        theme::DOT_INACTIVE
    };
    widgets::plus(painter, pos2(r.left() + 18.0, r.center().y), 12.0, color);
    widgets::text(
        painter,
        pos2(r.left() + 30.0, r.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        color,
        0.5,
    );
    if resp.clicked() && state.pattern.add_pulse().is_some() {
        actions.push(UiAction::PatternChanged);
    }
}

fn play_button(ui: &Ui, rect: Rect, state: &mut UiState, wide: bool, actions: &mut Vec<UiAction>) {
    let label = if state.playing { "Stop" } else { "Play" };
    let resp = widgets::button(ui, rect, ui.make_persistent_id("play"), label, true);
    let shape = if resp.is_pointer_button_down_on() {
        rect.shrink2(rect.size() * 0.025)
    } else {
        rect
    };
    let painter = ui.painter();
    let radius = CornerRadius::same(16);
    let (fg, text) = if state.playing {
        painter.rect(
            shape,
            radius,
            theme::BORDER,
            Stroke::new(1.0, theme::with_alpha(theme::ACCENT, 0.3)),
            StrokeKind::Inside,
        );
        (theme::ACCENT, "STOP")
    } else {
        let glow = if resp.hovered() { 0.3 } else { 0.2 };
        painter.add(
            Shadow {
                offset: [0, 0],
                blur: 40,
                spread: 0,
                color: theme::with_alpha(theme::ACCENT, glow),
            }
            .as_shape(shape, radius),
        );
        painter.rect_filled(shape, radius, theme::ACCENT);
        (Color32::WHITE, "PLAY")
    };
    let font = theme::font_bold(if wide { 20.0 } else { 18.0 });
    let text_w = widgets::text_width(painter, text, font.clone(), 1.0);
    let icon = 22.0;
    let x = shape.center().x - (icon + 14.0 + text_w) / 2.0;
    let c = pos2(x + icon / 2.0, shape.center().y);
    if state.playing {
        widgets::stop(painter, c, icon, fg);
    } else {
        widgets::play(painter, c, icon, fg);
    }
    widgets::text(
        painter,
        pos2(x + icon + 14.0, shape.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        fg,
        1.0,
    );
    if resp.clicked() {
        state.playing = !state.playing;
        if !state.playing {
            state.clear_beats();
        }
        actions.push(UiAction::TogglePlay);
    }
}

/// An edit requested by a pulse card, applied after the list is drawn.
enum Edit {
    Beats(PulseId, u32),
    Enabled(PulseId, bool),
    Remove(PulseId),
    Settings(PulseId),
    Cycle(PulseId, usize),
    Sound(PulseId, SoundType),
}

/// What the drag handles reported this frame.
#[derive(Default)]
struct DragOutcome {
    started: Option<usize>,
    /// Card under the pointer while a drag is in progress.
    hovered: Option<usize>,
    stopped: bool,
}

fn pulse_list(
    ui: &mut Ui,
    rect: Rect,
    grid: &Grid,
    state: &mut UiState,
    actions: &mut Vec<UiAction>,
) {
    if rect.height() < 1.0 {
        return;
    }
    let mut edits = Vec::new();
    let mut drag = DragOutcome::default();
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ScrollArea::vertical()
            .id_salt("pulse-list")
            .auto_shrink(false)
            .show(ui, |ui| pulse_grid(ui, grid, state, &mut edits, &mut drag));
    });

    let mut changed = false;
    for edit in edits {
        let p = &mut state.pattern;
        match edit {
            Edit::Beats(id, beats) => p.set_beats(id, beats),
            Edit::Enabled(id, on) => p.set_enabled(id, on),
            Edit::Remove(id) => {
                if !p.remove(id) {
                    continue;
                }
            }
            Edit::Settings(id) => {
                state.toggle_settings(id);
                continue;
            }
            Edit::Cycle(id, beat) => p.cycle_intensity(id, beat),
            Edit::Sound(id, sound) => p.set_sound(id, sound),
        }
        changed = true;
    }

    if let Some(from) = drag.started {
        state.dragging = Some((from, from));
    }
    if let (Some((from, _)), Some(target)) = (state.dragging, drag.hovered) {
        state.dragging = Some((from, target));
    }
    if drag.stopped
        && let Some((from, to)) = state.dragging.take()
        && from != to
    {
        state.pattern.move_pulse(from, to);
        changed = true;
    }
    if changed {
        actions.push(UiAction::PatternChanged);
    }
}

/// Card sizes and positions (relative to the list's top-left) for a list `width` wide.
struct Grid {
    columns: usize,
    card_w: f32,
    metrics: Vec<CardMetrics>,
    /// Top of each row.
    tops: Vec<f32>,
    /// Content height, insets included.
    height: f32,
}

impl Grid {
    fn new(painter: &Painter, state: &UiState, width: f32) -> Self {
        let width = width - 2.0 * LIST_INSET - SCROLL_BAR_ROOM;
        let fit = ((width + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).floor() as usize;
        let columns = fit.clamp(1, MAX_COLUMNS);
        let card_w = ((width - CARD_GAP * (columns - 1) as f32) / columns as f32).floor();
        let metrics: Vec<CardMetrics> = state
            .pattern
            .pulses
            .iter()
            .map(|p| CardMetrics::new(painter, p, card_w, state.settings_open(p.id)))
            .collect();
        // Rows are as tall as their tallest card; each card keeps its own height.
        let mut tops = Vec::new();
        let mut y = 0.0;
        for chunk in metrics.chunks(columns) {
            tops.push(y);
            y += chunk.iter().map(|m| m.height).fold(0.0, f32::max) + CARD_GAP;
        }
        Self {
            columns,
            card_w,
            metrics,
            tops,
            height: (y - CARD_GAP).max(0.0) + 2.0 * LIST_INSET,
        }
    }

    fn rect(&self, origin: Pos2, i: usize) -> Rect {
        let (row, col) = (i / self.columns, i % self.columns);
        Rect::from_min_size(
            origin + vec2(col as f32 * (self.card_w + CARD_GAP), self.tops[row]),
            vec2(self.card_w, self.metrics[i].height),
        )
    }
}

fn pulse_grid(
    ui: &mut Ui,
    grid: &Grid,
    state: &UiState,
    edits: &mut Vec<Edit>,
    drag: &mut DragOutcome,
) {
    let (_, area) = ui.allocate_space(vec2(ui.available_width(), grid.height));
    let origin = area.min + Vec2::splat(LIST_INSET);
    let pulses = &state.pattern.pulses;
    let rects: Vec<Rect> = (0..pulses.len()).map(|i| grid.rect(origin, i)).collect();

    if state.dragging.is_some()
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        drag.hovered = rects.iter().position(|r| r.contains(pos));
    }
    let drop_target = state
        .dragging
        .and_then(|(from, to)| Some(drag.hovered.unwrap_or(to)).filter(|&t| t != from));
    for (i, (pulse, metrics)) in pulses.iter().zip(&grid.metrics).enumerate() {
        Card {
            index: i,
            id: ui.make_persistent_id(("pulse", pulse.id)),
            pulse,
            rect: rects[i],
            metrics,
            can_remove: pulses.len() > 1,
            open: state.settings_open(pulse.id),
            lit: state.playing && pulse.enabled && state.active_beat(pulse.id).is_some(),
            flash: state.flash(pulse.id),
            drop_target: drop_target == Some(i),
        }
        .show(ui, edits, drag);
    }
}

const ICON_BUTTON: f32 = 24.0;
const ICON_ROW: f32 = 28.0;
const TITLE_ROW: f32 = 16.0;
const STEP_H: f32 = 22.0;
const NUMBER_H: f32 = 38.0;
const BAR_W: f32 = 16.0;
const BAR_H: f32 = 48.0;
const BAR_GAP: f32 = 4.0;
const BAR_LABEL: f32 = 12.0;
const BAR_ROW_GAP: f32 = 6.0;
const SETTINGS_PAD: f32 = 10.0;
const SOUND_CAPTION: f32 = 12.0;
const SOUND_H: f32 = 20.0;
const SOUND_GAP: f32 = 4.0;
const BOTTOM_BAR: f32 = 4.0;

fn title_font() -> FontId {
    theme::font_mono_medium(9.0)
}

/// Vertical metrics of a pulse card; they depend only on the card's own content.
struct CardMetrics {
    /// The title gets its own row when it does not fit between the icons.
    title_row: bool,
    /// Height of everything above the settings panel.
    base: f32,
    settings: f32,
    /// Intensity bars per row in the settings panel.
    bars_per_row: usize,
    height: f32,
}

impl CardMetrics {
    fn new(painter: &Painter, pulse: &Pulse, width: f32, open: bool) -> Self {
        // grip + settings on the left, power + trash on the right.
        let icons = 2.0 * (4.0 + 2.0 * ICON_BUTTON);
        let title_w = widgets::text_width(painter, "PULSO 16", title_font(), 1.0);
        let title_row = width - icons < title_w + 8.0;
        let base =
            ICON_ROW + if title_row { TITLE_ROW } else { 0.0 } + 2.0 * STEP_H + NUMBER_H + 8.0;
        let fit = ((width - 2.0 * SETTINGS_PAD + BAR_GAP) / (BAR_W + BAR_GAP)).floor();
        let bars_per_row = (fit as usize).max(1);
        let settings = if open {
            let rows = (pulse.intensities.len().max(1)).div_ceil(bars_per_row) as f32;
            SETTINGS_PAD
                + rows * (BAR_H + BAR_LABEL)
                + (rows - 1.0) * BAR_ROW_GAP
                + 12.0
                + SOUND_CAPTION
                + 2.0 * SOUND_H
                + SOUND_GAP
                + SETTINGS_PAD
        } else {
            0.0
        };
        Self {
            title_row,
            base,
            settings,
            bars_per_row,
            height: base + settings + BOTTOM_BAR,
        }
    }
}

struct Card<'a> {
    index: usize,
    id: Id,
    pulse: &'a Pulse,
    rect: Rect,
    metrics: &'a CardMetrics,
    can_remove: bool,
    open: bool,
    /// The pulse has a lit beat while playing.
    lit: bool,
    flash: f32,
    drop_target: bool,
}

impl Card<'_> {
    fn label(&self, what: &str) -> String {
        format!("Pulso {}: {what}", self.index + 1)
    }

    fn show(&self, ui: &Ui, edits: &mut Vec<Edit>, drag: &mut DragOutcome) {
        let (pulse, rect, id) = (self.pulse, self.rect, self.id);
        let mut painter = ui.painter().clone();
        if !pulse.enabled {
            painter.multiply_opacity(0.5);
        }
        let color = if pulse.enabled {
            pulse_color(pulse)
        } else {
            theme::DOT_INACTIVE
        };

        // The card's own node, so automation can find it and read its rect.
        let card = ui.interact(rect, id.with("card"), Sense::hover());
        let name = format!("Pulso {}", self.index + 1);
        card.widget_info(|| WidgetInfo::labeled(Role::Group, true, &name));

        let glow = if self.lit {
            0.5 + 0.5 * self.flash
        } else {
            0.0
        };
        let border = if self.lit { color } else { theme::BORDER };
        theme::paint_card_with_radius(&painter, rect, border, glow, PULSE_RADIUS);
        let radius = CornerRadius::same(PULSE_RADIUS);
        if self.lit && self.flash > 0.01 {
            let ring = Stroke::new(2.0, theme::with_alpha(color, 0.8 * self.flash));
            painter.rect_stroke(rect, radius, ring, StrokeKind::Outside);
        }
        if self.drop_target {
            let ring = Stroke::new(1.5, theme::ACCENT);
            painter.rect_stroke(rect, radius, ring, StrokeKind::Outside);
        }

        self.header(ui, &painter, edits, drag);
        self.counter(ui, &painter, color, edits);

        let inner = rect.shrink(1.0);
        let inner_radius = f32::from(PULSE_RADIUS) - 1.0;
        if self.open {
            let panel = Rect::from_min_size(
                pos2(inner.left(), rect.top() + self.metrics.base),
                vec2(inner.width(), self.metrics.settings),
            );
            self.settings(ui, &painter, panel, color, edits);
        }

        // Bottom bar: one segment per beat, alpha by intensity.
        let bar = Rect::from_min_max(pos2(inner.left(), inner.bottom() - BOTTOM_BAR), inner.max);
        theme::paint_band(&painter, inner, inner_radius, bar, theme::BORDER);
        let seg = bar.width() / pulse.intensities.len().max(1) as f32;
        for (k, intensity) in pulse.intensities.iter().enumerate() {
            let alpha = if pulse.enabled {
                intensity.gain() * 0.7
            } else {
                0.3
            };
            let x0 = bar.left() + seg * k as f32;
            let r = Rect::from_min_max(pos2(x0, bar.top()), pos2(x0 + seg - 0.5, bar.bottom()));
            theme::paint_band(
                &painter,
                inner,
                inner_radius,
                r,
                theme::with_alpha(color, alpha),
            );
        }
    }

    /// Drag handle and settings on the left, mute and remove on the right, title between.
    fn header(&self, ui: &Ui, painter: &Painter, edits: &mut Vec<Edit>, drag: &mut DragOutcome) {
        let (pulse, rect, id) = (self.pulse, self.rect, self.id);
        let icon_y = rect.top() + 4.0 + ICON_BUTTON / 2.0;
        let slot = |x: f32| Rect::from_center_size(pos2(x, icon_y), Vec2::splat(ICON_BUTTON));
        let left = rect.left() + 4.0 + ICON_BUTTON / 2.0;
        let right = rect.right() - 4.0 - ICON_BUTTON / 2.0;

        let grip_rect = slot(left);
        let grip = widgets::handle(ui, grip_rect, id.with("move"), &self.label("mover"));
        let grip_color = if grip.hovered() || grip.dragged() {
            theme::TEXT_MUTED
        } else {
            theme::DOT_INACTIVE
        };
        widgets::grip(painter, grip_rect.center(), 14.0, grip_color);
        if grip.drag_started() {
            drag.started = Some(self.index);
        }
        if grip.drag_stopped() {
            drag.stopped = true;
        }

        let gear_rect = slot(left + ICON_BUTTON);
        let gear = widgets::toggle(
            ui,
            gear_rect,
            id.with("settings"),
            &self.label("ajustes"),
            self.open,
        );
        let gear_color = if self.open {
            theme::ACCENT
        } else {
            idle_color(&gear, theme::TEXT_MUTED)
        };
        widgets::settings(painter, gear_rect.center(), 16.0, gear_color);
        if gear.clicked() {
            edits.push(Edit::Settings(pulse.id));
        }

        let trash_rect = slot(right);
        let trash = widgets::button(
            ui,
            trash_rect,
            id.with("remove"),
            &self.label("remover"),
            self.can_remove,
        );
        let trash_color = if !self.can_remove {
            theme::BORDER
        } else if trash.hovered() {
            theme::PALETTE[4]
        } else {
            theme::TEXT_MUTED
        };
        widgets::trash(painter, trash_rect.center(), 15.0, trash_color);
        if trash.clicked() {
            edits.push(Edit::Remove(pulse.id));
        }

        let power_rect = slot(right - ICON_BUTTON);
        let power = widgets::toggle(
            ui,
            power_rect,
            id.with("mute"),
            &self.label("silenciar"),
            !pulse.enabled,
        );
        let green = theme::PALETTE[2];
        let (power_color, power_fill) = if pulse.enabled {
            let c = if power.hovered() {
                green
            } else {
                theme::TEXT_MUTED
            };
            (c, Some(theme::with_alpha(green, 0.2)))
        } else {
            (idle_color(&power, theme::DOT_INACTIVE), None)
        };
        widgets::power(painter, power_rect.center(), 14.0, power_color, power_fill);
        if power.clicked() {
            edits.push(Edit::Enabled(pulse.id, !pulse.enabled));
        }

        let title_y = if self.metrics.title_row {
            rect.top() + ICON_ROW + TITLE_ROW / 2.0
        } else {
            icon_y
        };
        widgets::text(
            painter,
            pos2(rect.center().x, title_y),
            Align2::CENTER_CENTER,
            &format!("PULSO {}", self.index + 1),
            title_font(),
            theme::TEXT_MUTED,
            1.0,
        );
    }

    /// `+`, the beat count in the pulse color, `-`.
    fn counter(&self, ui: &Ui, painter: &Painter, color: Color32, edits: &mut Vec<Edit>) {
        let (pulse, rect, id) = (self.pulse, self.rect, self.id);
        let cx = rect.center().x;
        let mut y = rect.top()
            + ICON_ROW
            + if self.metrics.title_row {
                TITLE_ROW
            } else {
                0.0
            };
        let up = Rect::from_min_size(pos2(cx - 16.0, y), vec2(32.0, STEP_H));
        let up_resp = widgets::button(
            ui,
            up,
            id.with("more"),
            &self.label("aumentar batidas"),
            true,
        );
        widgets::plus(
            painter,
            up.center(),
            14.0,
            idle_color(&up_resp, theme::TEXT_DIM),
        );
        if up_resp.clicked() {
            edits.push(Edit::Beats(pulse.id, pulse.beats + 1));
        }
        y += STEP_H;
        widgets::text(
            painter,
            pos2(cx, y + NUMBER_H / 2.0),
            Align2::CENTER_CENTER,
            &pulse.beats.to_string(),
            theme::font_light(30.0),
            color,
            0.0,
        );
        y += NUMBER_H;
        let down = Rect::from_min_size(pos2(cx - 16.0, y), vec2(32.0, STEP_H));
        let can_less = pulse.beats > 1;
        let down_resp = widgets::button(
            ui,
            down,
            id.with("less"),
            &self.label("diminuir batidas"),
            can_less,
        );
        let down_color = if can_less {
            idle_color(&down_resp, theme::TEXT_DIM)
        } else {
            theme::DOT_INACTIVE
        };
        widgets::minus(painter, down.center(), 14.0, down_color);
        if down_resp.clicked() {
            edits.push(Edit::Beats(pulse.id, pulse.beats - 1));
        }
    }

    /// Per-beat intensity bars (wrapped, centered rows) and the 2 x 2 sound grid.
    fn settings(
        &self,
        ui: &Ui,
        painter: &Painter,
        panel: Rect,
        color: Color32,
        edits: &mut Vec<Edit>,
    ) {
        let (pulse, id) = (self.pulse, self.id);
        painter.rect_filled(panel, 0.0, Color32::from_black_alpha(51));
        painter.hline(
            panel.x_range(),
            panel.top(),
            Stroke::new(1.0, theme::with_alpha(theme::BORDER, 0.5)),
        );

        let per_row = self.metrics.bars_per_row;
        let mut y = panel.top() + SETTINGS_PAD;
        for (row, chunk) in pulse.intensities.chunks(per_row).enumerate() {
            let count = chunk.len() as f32;
            let row_w = count * BAR_W + (count - 1.0) * BAR_GAP;
            let x0 = panel.center().x - row_w / 2.0;
            for (col, intensity) in chunk.iter().enumerate() {
                let k = row * per_row + col;
                let r = Rect::from_min_size(
                    pos2(x0 + col as f32 * (BAR_W + BAR_GAP), y),
                    vec2(BAR_W, BAR_H),
                );
                let label = self.label(&format!("batida {}", k + 1));
                let resp = widgets::button(ui, r, id.with(("beat", k)), &label, true);
                let r = if resp.is_pointer_button_down_on() {
                    r.shrink(0.6)
                } else {
                    r
                };
                let bg = if resp.hovered() {
                    HOVER_FILL
                } else {
                    theme::BORDER
                };
                let edge = Stroke::new(1.0, theme::with_alpha(theme::DOT_INACTIVE, 0.3));
                painter.rect(r, 2.0, bg, edge, StrokeKind::Inside);
                if *intensity != Intensity::Mute {
                    let top = r.bottom() - r.height() * intensity.gain();
                    let fill = Rect::from_min_max(pos2(r.left(), top), r.max);
                    painter.rect_filled(fill, 2.0, color);
                }
                widgets::text(
                    painter,
                    pos2(r.center().x, r.bottom() + BAR_LABEL / 2.0 + 1.0),
                    Align2::CENTER_CENTER,
                    &(k + 1).to_string(),
                    theme::font_mono(8.0),
                    theme::TEXT_FAINT,
                    0.0,
                );
                if resp.clicked() {
                    edits.push(Edit::Cycle(pulse.id, k));
                }
            }
            y += BAR_H + BAR_LABEL + BAR_ROW_GAP;
        }
        y += 12.0 - BAR_ROW_GAP;

        let left = panel.left() + SETTINGS_PAD;
        widgets::text(
            painter,
            pos2(left, y + SOUND_CAPTION / 2.0),
            Align2::LEFT_CENTER,
            "SOM",
            theme::font_mono(8.0),
            theme::TEXT_MUTED,
            1.0,
        );
        y += SOUND_CAPTION;
        let cell_w = (panel.width() - 2.0 * SETTINGS_PAD - SOUND_GAP) / 2.0;
        let sounds = [
            (SoundType::Sine, "Sine"),
            (SoundType::Wood, "Wood"),
            (SoundType::Bell, "Bell"),
            (SoundType::Electronic, "Electronic"),
        ];
        for (i, (sound, name)) in sounds.into_iter().enumerate() {
            let (row, col) = (i / 2, i % 2);
            let r = Rect::from_min_size(
                pos2(
                    left + col as f32 * (cell_w + SOUND_GAP),
                    y + row as f32 * (SOUND_H + SOUND_GAP),
                ),
                vec2(cell_w, SOUND_H),
            );
            let selected = pulse.sound == sound;
            let label = self.label(&format!("som {name}"));
            let resp = widgets::toggle(ui, r, id.with(("sound", i)), &label, selected);
            let (bg, edge, text) = if selected {
                (theme::DOT_INACTIVE, theme::TEXT_FAINT, Color32::WHITE)
            } else if resp.hovered() {
                (HOVER_FILL, theme::BORDER, theme::TEXT_DIM)
            } else {
                (theme::BORDER, theme::BORDER, theme::TEXT_MUTED)
            };
            painter.rect(r, 4.0, bg, Stroke::new(1.0, edge), StrokeKind::Inside);
            widgets::elided(
                painter,
                r.center(),
                Align2::CENTER_CENTER,
                &name.to_uppercase(),
                theme::font_semibold(8.0),
                text,
                r.width() - 6.0,
            );
            if resp.clicked() && !selected {
                edits.push(Edit::Sound(pulse.id, sound));
            }
        }
    }
}
