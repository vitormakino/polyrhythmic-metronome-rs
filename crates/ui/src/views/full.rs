//! The full window: tempo card, pulse cards, topology and the play button.
//!
//! Layout (spec §6): the topology and the play button are always visible and
//! only the pulse list scrolls; every pulse card has its own height, so
//! opening one card's settings never resizes its neighbours. Wide windows use
//! two columns like the web app (controls left, topology right); narrow ones
//! stack everything in one column. With session files, the session strip sits
//! right above the play button, taking its height from the pulse list.

use std::time::Instant;

use egui::epaint::Shadow;
use egui::{Align2, CornerRadius, Rect, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2};
use pm_core::MAX_PULSES;

use super::bpm_card::bpm_card;
use super::footer::{self, footer};
use super::pulse_card::{Grid, pulse_list};
use super::session_strip::{self, session_strip};
use super::topology::topology;
use super::widgets;
use crate::state::{UiAction, UiState};
use crate::theme;

/// Below this width everything stacks in one column.
const WIDE_MIN: f32 = 760.0;
/// Content never gets wider than this (web: `max-w-5xl` plus some air).
const MAX_CONTENT_WIDTH: f32 = 1100.0;
const GAP: f32 = 12.0;

/// Draws the full window into `ui`, editing `state`; returns what the app must forward.
/// `now` timestamps taps.
pub fn full_view(ui: &mut Ui, state: &mut UiState, now: Instant) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let t = state.theme.clone();
    let screen = ui.max_rect();
    ui.painter().rect_filled(screen, 0.0, t.bg);
    state.keep_session_summary();
    let mut layout = Layout::new(screen, session_strip::strip_height(state));

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
        layout.strip = layout.strip.map(|s| s.translate(vec2(0.0, -shift)));
    }
    pulse_list(ui, layout.list, &grid, state, &mut actions);
    if let Some(strip) = layout.strip {
        session_strip(ui, strip, state, &mut actions);
    }
    play_button(ui, layout.play, state, layout.wide, &mut actions);
    footer(ui, layout.footer, state, &mut actions);
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
    /// The session strip, when there are session files.
    strip: Option<Rect>,
    play: Rect,
    footer: Rect,
}

fn row(top: f32, height: f32, left: f32, right: f32) -> Rect {
    Rect::from_min_max(pos2(left, top), pos2(right, top + height))
}

impl Layout {
    /// `strip_h` is the session strip's height, 0 for none.
    fn new(screen: Rect, strip_h: f32) -> Self {
        let wide = screen.width() >= WIDE_MIN;
        let pad = if wide { 24.0 } else { 16.0 };
        let mut content = screen.shrink(pad);
        if content.width() > MAX_CONTENT_WIDTH {
            let extra = content.width() - MAX_CONTENT_WIDTH;
            content = content.shrink2(vec2(extra / 2.0, 0.0));
        }
        // The footer sits in the lower half of the bottom padding.
        let footer = row(
            screen.bottom() - pad / 2.0 - footer::HEIGHT,
            footer::HEIGHT,
            content.left(),
            content.right(),
        );
        content.max.y = footer.top() - 6.0;
        let (l, r) = (content.left(), content.right());
        if wide {
            let top = row(content.top(), 32.0, l, r);
            let body_top = top.bottom() + 16.0;
            let column_gap = 24.0;
            let left_width = ((content.width() - column_gap) * 7.0 / 12.0).floor();
            let (ll, lr) = (l, l + left_width);
            let bpm = row(body_top, 184.0, ll, lr);
            let play = row(content.bottom() - 64.0, 64.0, ll, lr);
            let strip = (strip_h > 0.0).then(|| row(play.top() - GAP - strip_h, strip_h, ll, lr));
            let above = strip.map_or(play.top(), |s| s.top());
            let header = row(bpm.bottom() + 20.0, 28.0, ll, lr);
            let list = Rect::from_min_max(pos2(ll, header.bottom() + 8.0), pos2(lr, above - 12.0));
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
                strip,
                play,
                footer,
            }
        } else {
            let top = row(content.top(), 24.0, l, r);
            let bpm = row(top.bottom() + GAP, 144.0, l, r);
            let play = row(content.bottom() - 56.0, 56.0, l, r);
            let strip = (strip_h > 0.0).then(|| row(play.top() - GAP - strip_h, strip_h, l, r));
            let above = strip.map_or(play.top(), |s| s.top());
            let header_h = 28.0;
            // What is left between the tempo card and the play button (or
            // the session strip) goes half to the topology, half to the pulse list.
            let flex = (above - GAP - (bpm.bottom() + GAP) - GAP - header_h - 8.0).max(0.0);
            let side = (flex * 0.5).min(content.width()).floor();
            let topology = Rect::from_min_size(
                pos2(content.center().x - side / 2.0, bpm.bottom() + GAP),
                Vec2::splat(side),
            );
            let header = row(topology.bottom() + GAP, header_h, l, r);
            let list = Rect::from_min_max(pos2(l, header.bottom() + 8.0), pos2(r, above - GAP));
            Layout {
                wide,
                top,
                bpm,
                topology,
                header,
                list,
                strip,
                play,
                footer,
            }
        }
    }
}

fn top_bar(ui: &Ui, rect: Rect, state: &UiState, wide: bool) {
    let t = &state.theme;
    let painter = ui.painter_at(rect);
    let icon = if wide { 20.0 } else { 16.0 };
    let cy = rect.center().y;
    widgets::zap(&painter, pos2(rect.left() + icon / 2.0, cy), icon, t.accent);
    let title = widgets::text(
        &painter,
        pos2(rect.left() + icon + 8.0, cy),
        Align2::LEFT_CENTER,
        &state.messages.app_heading,
        theme::font_bold(if wide { 20.0 } else { 14.0 }),
        t.text,
        0.0,
    );
    let right = rect.right();
    if let Some(name) = &state.device_name {
        let max = right - title.right() - 16.0;
        if max > 24.0 {
            widgets::elided(
                &painter,
                pos2(right, cy),
                Align2::RIGHT_CENTER,
                name,
                theme::font_mono(10.0),
                t.text_faint,
                max,
            );
        }
    }
}

fn topology_card(ui: &mut Ui, rect: Rect, state: &UiState) {
    let t = &state.theme;
    if rect.width() < 1.0 {
        return;
    }
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(theme::CARD_RADIUS),
        theme::with_alpha(t.card, 0.4),
        Stroke::new(1.0, theme::with_alpha(t.border, 0.5)),
        StrokeKind::Inside,
    );
    let mut inner = rect.shrink(rect.width() * 0.06);
    if rect.width() >= 220.0 {
        widgets::text(
            painter,
            pos2(rect.center().x, rect.top() + 28.0),
            Align2::CENTER_CENTER,
            &state.messages.topology.to_uppercase(),
            theme::font_mono(9.0),
            t.text_faint,
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
    let t = state.theme.clone();
    let painter = ui.painter();
    widgets::text(
        painter,
        pos2(rect.left() + 4.0, rect.center().y),
        Align2::LEFT_CENTER,
        &state.messages.active_pulses,
        theme::font_mono(10.0),
        t.text_muted,
        2.0,
    );
    let font = theme::font_semibold(10.0);
    let m = state.messages.clone();
    let label = m.add_pulse.to_uppercase();
    let text_w = widgets::text_width(painter, &label, font.clone(), 0.5);
    // padding 12 | plus 12 | gap 6 | text | padding 12
    let w = 12.0 + 12.0 + 6.0 + text_w + 12.0;
    let r = Rect::from_min_max(pos2(rect.right() - w, rect.top()), rect.right_bottom());
    let enabled = state.pattern.pulses.len() < MAX_PULSES;
    let resp = widgets::button(
        ui,
        r,
        ui.make_persistent_id("add-pulse"),
        &m.add_pulse,
        enabled,
    );
    let fill = if resp.hovered() { t.border } else { t.card };
    painter.rect(
        r,
        r.height() / 2.0,
        fill,
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    let color = if enabled { t.text_soft } else { t.dot_inactive };
    widgets::plus(painter, pos2(r.left() + 18.0, r.center().y), 12.0, color);
    widgets::text(
        painter,
        pos2(r.left() + 30.0, r.center().y),
        Align2::LEFT_CENTER,
        &label,
        font,
        color,
        0.5,
    );
    if resp.clicked() && state.pattern.add_pulse().is_some() {
        actions.push(UiAction::PatternChanged);
    }
}

/// The Play/Stop button (also used by the mini view).
pub(super) fn play_button(
    ui: &Ui,
    rect: Rect,
    state: &mut UiState,
    wide: bool,
    actions: &mut Vec<UiAction>,
) {
    let t = state.theme.clone();
    let m = &state.messages;
    let label = if state.playing { &m.stop } else { &m.play };
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
            t.border,
            Stroke::new(1.0, theme::with_alpha(t.accent, 0.3)),
            StrokeKind::Inside,
        );
        (t.accent, m.stop.to_uppercase())
    } else {
        let glow = if resp.hovered() { 0.3 } else { 0.2 };
        painter.add(
            Shadow {
                offset: [0, 0],
                blur: 40,
                spread: 0,
                color: theme::with_alpha(t.accent, glow),
            }
            .as_shape(shape, radius),
        );
        painter.rect_filled(shape, radius, t.accent);
        (t.on_accent, m.play.to_uppercase())
    };
    let font = theme::font_bold(if wide { 20.0 } else { 18.0 });
    let text_w = widgets::text_width(painter, &text, font.clone(), 1.0);
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
        &text,
        font,
        fg,
        1.0,
    );
    if resp.clicked() {
        state.playing = !state.playing;
        if state.playing {
            // A new run: the last one's summary and progress are over.
            state.session_summary = None;
            state.session_progress = None;
        } else {
            state.clear_beats();
        }
        actions.push(UiAction::TogglePlay);
    }
}
