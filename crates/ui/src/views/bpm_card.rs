//! The tempo card: TAP, mini-window toggle, -/number/+ and the BPM slider.
//! Shared by the full window and (Task 12) the mini window.

use std::time::Instant;

use egui::style::HandleShape;
use egui::{
    Align2, CornerRadius, FontId, Pos2, Rect, Role, Sense, Slider, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};
use pm_core::Bpm;

use super::widgets;
use crate::state::{UiAction, UiState, WindowMode};
use crate::theme::{self, Theme};

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
    let t = state.theme.clone();
    let painter = ui.painter().clone();
    theme::paint_card(&painter, &t, rect, t.border, 0.0);
    theme::paint_top_accent(&painter, &t, rect);
    let pad = if wide { 24.0 } else { 16.0 };
    let inner = rect.shrink2(vec2(pad, if wide { 20.0 } else { 14.0 }));

    let slider_h = 20.0;

    // Header: caption left, mini window and TAP right.
    let header_y = inner.top() + 12.0;
    widgets::text(
        &painter,
        pos2(inner.left(), header_y),
        Align2::LEFT_CENTER,
        &state.messages.tempo_heading,
        theme::font_mono(11.0),
        t.text_muted,
        2.0,
    );
    let tap_rect = Rect::from_center_size(pos2(inner.right() - 24.0, header_y), vec2(48.0, 22.0));
    let tap = widgets::button(
        ui,
        tap_rect,
        ui.make_persistent_id("tap"),
        &state.messages.tap,
        true,
    );
    let tap_shape = if tap.is_pointer_button_down_on() {
        tap_rect.shrink(1.0)
    } else {
        tap_rect
    };
    painter.rect(
        tap_shape,
        4.0,
        theme::with_alpha(t.accent, 0.1),
        Stroke::new(1.0, theme::with_alpha(t.accent, 0.2)),
        StrokeKind::Inside,
    );
    widgets::text(
        &painter,
        tap_shape.center(),
        Align2::CENTER_CENTER,
        &state.messages.tap.to_uppercase(),
        theme::font_mono_medium(10.0),
        theme::with_alpha(t.accent, if tap.hovered() { 1.0 } else { 0.8 }),
        1.0,
    );
    if tap.clicked() {
        state.tap(now, actions);
    }
    let pin_rect = Rect::from_center_size(
        pos2(tap_rect.left() - 8.0 - 13.0, header_y),
        Vec2::splat(26.0),
    );
    let pin = widgets::button(
        ui,
        pin_rect,
        ui.make_persistent_id("mini"),
        &state.messages.pin_mini,
        true,
    );
    if pin.hovered() {
        painter.rect_filled(pin_rect, 6.0, t.border);
    }
    widgets::mini_window(
        &painter,
        pin_rect.center(),
        16.0,
        widgets::idle_color(&t, &pin, t.text_muted),
    );
    if pin.clicked() {
        state.mode = match state.mode {
            WindowMode::Full => WindowMode::Mini,
            WindowMode::Mini => WindowMode::Full,
        };
        actions.push(UiAction::ToggleMini);
    }

    // Big number between - and +.
    let number_area = Rect::from_min_max(
        pos2(inner.left(), header_y + 12.0),
        pos2(inner.right(), inner.bottom() - slider_h - 4.0),
    );
    let font = theme::font_light(if wide { 72.0 } else { 56.0 });
    bpm_stepper(
        ui,
        number_area.center(),
        font,
        if wide { 48.0 } else { 40.0 },
        state,
        actions,
    );

    // Slider along the bottom.
    let slider_rect = Rect::from_min_max(
        pos2(inner.left(), inner.bottom() - slider_h),
        inner.right_bottom(),
    );
    let mut value = i32::from(state.bpm.get());
    let changed = ui
        .scope_builder(UiBuilder::new().max_rect(slider_rect), |ui| {
            style_slider(ui, &t, slider_rect.width());
            ui.name_inputs(&state.messages.bpm, |ui| {
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

/// The big BPM number centred on `center` with a round "-" and "+" button on
/// either side (`step` pt wide). Returns the number's rect. Shared by the tempo
/// card and the mini view.
pub(super) fn bpm_stepper(
    ui: &mut Ui,
    center: Pos2,
    font: FontId,
    step: f32,
    state: &mut UiState,
    actions: &mut Vec<UiAction>,
) -> Rect {
    let painter = ui.painter().clone();
    let m = state.messages.clone();
    let t = state.theme.clone();
    let digits = widgets::text_width(&painter, "300", font.clone(), -2.0);
    widgets::text(
        &painter,
        center,
        Align2::CENTER_CENTER,
        &state.bpm.get().to_string(),
        font.clone(),
        t.strong,
        -2.0,
    );
    let gap = (step * 0.5).min(24.0);
    let offset = digits / 2.0 + gap + step / 2.0;
    let icon = (step * 0.6).min(24.0);
    for (label, delta) in [(&m.bpm_decrease, -1), (&m.bpm_increase, 1)] {
        let c = center + vec2(offset * delta as f32, 0.0);
        let r = Rect::from_center_size(c, Vec2::splat(step));
        let resp = widgets::button(
            ui,
            r,
            ui.make_persistent_id(("bpm-step", delta)),
            label,
            true,
        );
        if resp.hovered() {
            painter.circle_filled(c, step / 2.0, t.border);
        }
        let at_limit = (delta < 0 && state.bpm.get() == Bpm::MIN)
            || (delta > 0 && state.bpm.get() == Bpm::MAX);
        let color = if at_limit {
            t.dot_inactive
        } else {
            widgets::idle_color(&t, &resp, t.text_dim)
        };
        if delta < 0 {
            widgets::minus(&painter, c, icon, color);
        } else {
            widgets::plus(&painter, c, icon, color);
        }
        if resp.clicked() {
            set_bpm(state, i32::from(state.bpm.get()) + delta, actions);
        }
    }
    Rect::from_center_size(center, vec2(digits, font.size * 1.1))
}

/// Points of mouse-wheel / trackpad scroll that make one BPM step.
fn scroll_notch(ui: &Ui) -> f32 {
    ui.ctx().options(|o| o.input_options.line_scroll_speed)
}

/// Vertical drag distance (pt) per BPM step.
const DRAG_STEP: f32 = 4.0;

/// Scroll over / vertical drag on the BPM number: one wheel notch is +-1,
/// dragging up raises by 1 per [`DRAG_STEP`] px. The sub-step remainder is
/// kept between frames so slow gestures still add up.
pub(super) fn bpm_gestures(
    ui: &mut Ui,
    rect: Rect,
    state: &mut UiState,
    actions: &mut Vec<UiAction>,
) {
    let id = ui.make_persistent_id("bpm-gestures");
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    resp.widget_info(|| WidgetInfo::labeled(Role::SpinButton, true, &state.messages.bpm));
    let resp = resp.on_hover_cursor(egui::CursorIcon::ResizeVertical);

    let scroll = if resp.hovered() {
        ui.input(|i| i.smooth_scroll_delta.y)
    } else {
        0.0
    };
    let drag = if resp.dragged() {
        -resp.drag_delta().y
    } else {
        0.0
    };
    let acc_id = id.with("acc");
    let (mut scroll_acc, mut drag_acc): (f32, f32) =
        ui.data(|d| d.get_temp(acc_id)).unwrap_or((0.0, 0.0));
    if !resp.hovered() && !resp.dragged() {
        (scroll_acc, drag_acc) = (0.0, 0.0);
    }
    scroll_acc += scroll;
    drag_acc += drag;
    let notch = scroll_notch(ui);
    let scroll_steps = (scroll_acc / notch).trunc();
    let drag_steps = (drag_acc / DRAG_STEP).trunc();
    scroll_acc -= scroll_steps * notch;
    drag_acc -= drag_steps * DRAG_STEP;
    ui.data_mut(|d| d.insert_temp(acc_id, (scroll_acc, drag_acc)));
    let steps = (scroll_steps + drag_steps) as i32;
    if steps != 0 {
        set_bpm(state, i32::from(state.bpm.get()) + steps, actions);
    }
}

pub(super) fn set_bpm(state: &mut UiState, value: i32, actions: &mut Vec<UiAction>) {
    let bpm = Bpm::new(value);
    if bpm != state.bpm {
        state.bpm = bpm;
        actions.push(UiAction::SetBpm(bpm));
    }
}

/// Web look for the range input: thin zinc-800 rail, orange fill and knob.
fn style_slider(ui: &mut Ui, t: &Theme, width: f32) {
    let style = ui.style_mut();
    style.spacing.slider_width = width;
    style.spacing.slider_rail_height = 4.0;
    style.spacing.interact_size.y = 20.0;
    style.visuals.selection.bg_fill = t.accent;
    let w = &mut style.visuals.widgets;
    // The rail is painted with `inactive.bg_fill`.
    w.inactive.bg_fill = t.border;
    w.inactive.corner_radius = CornerRadius::same(2);
    w.inactive.fg_stroke = Stroke::new(1.0, t.accent);
    w.hovered.fg_stroke = Stroke::new(1.0, t.accent_light);
    w.active.fg_stroke = Stroke::new(1.0, t.accent_light);
    w.hovered.bg_fill = t.accent;
    w.active.bg_fill = t.accent;
}
