//! The full window's footer: audio output status ("sem saída de áudio",
//! "Falhas de áudio: n") and the "Configurações" popover with the visual
//! offset slider.

use std::time::Duration;

use egui::style::HandleShape;
use egui::{
    Align2, CornerRadius, Frame, Margin, Popup, PopupCloseBehavior, Rect, RectAlign, Role, Sense,
    Slider, Stroke, Ui, Vec2, WidgetInfo, pos2, vec2,
};

use super::widgets;
use crate::state::{UiAction, UiState};
use crate::theme;

/// Height of the footer row (pt).
pub(crate) const HEIGHT: f32 = 20.0;
/// Largest visual offset the slider allows (ms).
pub const MAX_VISUAL_OFFSET_MS: u32 = 500;
/// Accessible name of the visual offset slider.
pub const OFFSET_LABEL: &str = "Offset visual (ms)";

const ROSE: egui::Color32 = theme::PALETTE[4];

pub(crate) fn footer(ui: &mut Ui, rect: Rect, state: &mut UiState, actions: &mut Vec<UiAction>) {
    let painter = ui.painter().clone();
    let font = theme::font_mono(10.0);
    let cy = rect.center().y;
    let mut x = rect.left() + 4.0;
    let mut status = |ui: &Ui, key: &str, text: &str| {
        painter.circle_filled(pos2(x + 3.0, cy), 3.0, ROSE);
        let r = widgets::text(
            &painter,
            pos2(x + 12.0, cy),
            Align2::LEFT_CENTER,
            text,
            font.clone(),
            theme::TEXT_MUTED,
            0.5,
        );
        let node = Rect::from_min_max(pos2(x, rect.top()), pos2(r.right(), rect.bottom()));
        let label = text.to_owned();
        ui.interact(node, ui.make_persistent_id(key), Sense::hover())
            .widget_info(|| WidgetInfo::labeled(Role::Label, true, &label));
        x = r.right() + 16.0;
    };
    if state.device_name.is_none() {
        status(ui, "no-output", "sem saída de áudio");
    }
    if state.underruns > 0 {
        status(
            ui,
            "underruns",
            &format!("Falhas de áudio: {}", state.underruns),
        );
    }

    let gear = Rect::from_center_size(pos2(rect.right() - 11.0, cy), Vec2::splat(22.0));
    let id = ui.make_persistent_id("app-settings");
    let open = Popup::is_id_open(ui.ctx(), id.with("popup"));
    let resp = widgets::toggle(ui, gear, id, "Configurações", open);
    if resp.hovered() || open {
        ui.painter().rect_filled(gear, 6.0, theme::BORDER);
    }
    widgets::settings(
        ui.painter(),
        gear.center(),
        14.0,
        if open {
            theme::ACCENT
        } else {
            widgets::idle_color(&resp, theme::TEXT_MUTED)
        },
    );
    Popup::from_toggle_button_response(&resp)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .align(RectAlign::TOP_END)
        .gap(6.0)
        .frame(
            Frame::new()
                .fill(theme::CARD)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(12)),
        )
        .show(|ui| offset_slider(ui, state, actions));
}

fn offset_slider(ui: &mut Ui, state: &mut UiState, actions: &mut Vec<UiAction>) {
    let (title, _) = ui.allocate_exact_size(vec2(200.0, 14.0), Sense::hover());
    widgets::text(
        ui.painter(),
        title.left_center(),
        Align2::LEFT_CENTER,
        "OFFSET VISUAL (MS)",
        theme::font_mono(9.0),
        theme::TEXT_MUTED,
        1.5,
    );
    ui.add_space(4.0);
    let mut ms = u32::try_from(state.visual_offset.as_millis())
        .unwrap_or(u32::MAX)
        .min(MAX_VISUAL_OFFSET_MS);
    let before = ms;
    let changed = ui
        .name_inputs(OFFSET_LABEL, |ui| {
            ui.spacing_mut().slider_width = 150.0;
            ui.add(
                Slider::new(&mut ms, 0..=MAX_VISUAL_OFFSET_MS)
                    .step_by(1.0)
                    .suffix(" ms")
                    .trailing_fill(true)
                    .handle_shape(HandleShape::Circle),
            )
        })
        .changed();
    if changed && ms != before {
        state.visual_offset = Duration::from_millis(u64::from(ms));
        actions.push(UiAction::SetVisualOffset(state.visual_offset));
    }
}
