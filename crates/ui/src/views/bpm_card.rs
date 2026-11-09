//! The tempo card: TAP, mini-window toggle, -/number/+ and the BPM slider.
//! Shared by the full window and (Task 12) the mini window.

use std::time::Instant;

use egui::style::HandleShape;
use egui::{
    Align2, Color32, CornerRadius, Rect, Slider, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2,
    vec2,
};
use pm_core::Bpm;

use super::widgets;
use crate::state::{UiAction, UiState, WindowMode};
use crate::theme;

/// Draws the tempo card in `rect`. `wide` picks the roomy variant (72 pt
/// number); otherwise it is compact (56 pt). Edits `state.bpm`/`state.mode`
/// and pushes `Tap`, `SetBpm` and `ToggleMini` actions.
pub(crate) fn bpm_card(
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
        widgets::idle_color(&pin, theme::TEXT_MUTED),
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
            widgets::idle_color(&resp, theme::TEXT_DIM)
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
