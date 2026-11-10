//! The full window's footer: audio output status ("no audio output",
//! "Audio dropouts: n") and the settings popover with the visual offset
//! slider. All texts come from the active [`Messages`](crate::i18n::Messages).

use std::time::Duration;

use egui::style::HandleShape;
use egui::{
    Align2, CornerRadius, Frame, Margin, Popup, PopupCloseBehavior, Rect, RectAlign, Role, Sense,
    Slider, Stroke, Ui, Vec2, WidgetInfo, pos2, vec2,
};

use super::widgets;
use crate::i18n::fill;
use crate::state::{UiAction, UiState};
use crate::theme;

/// Height of the footer row (pt).
pub(crate) const HEIGHT: f32 = 20.0;
/// Largest visual offset the slider allows (ms).
pub const MAX_VISUAL_OFFSET_MS: u32 = 500;

const ROSE: egui::Color32 = theme::PALETTE[4];

pub(crate) fn footer(ui: &mut Ui, rect: Rect, state: &mut UiState, actions: &mut Vec<UiAction>) {
    let painter = ui.painter().clone();
    let m = state.messages.clone();
    let font = theme::font_mono(10.0);
    let cy = rect.center().y;
    let mut x = rect.left() + 4.0;
    let mut status = |ui: &Ui, key: &str, text: &str, hover: Option<&str>| {
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
        let resp = ui.interact(node, ui.make_persistent_id(key), Sense::hover());
        resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, &label));
        if let Some(hover) = hover {
            resp.on_hover_text(hover);
        }
        x = r.right() + 16.0;
    };
    if state.device_name.is_none() {
        let error = state.audio_error.as_deref();
        status(ui, "no-output", &m.no_output, error);
    }
    if state.underruns > 0 {
        status(
            ui,
            "underruns",
            &fill(&m.underruns, &[("n", &state.underruns)]),
            None,
        );
    }

    let gear = Rect::from_center_size(pos2(rect.right() - 11.0, cy), Vec2::splat(22.0));
    let id = ui.make_persistent_id("app-settings");
    let open = Popup::is_id_open(ui.ctx(), id.with("popup"));
    let resp = widgets::toggle(ui, gear, id, &m.settings, open);
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
        .show(|ui| offset_slider(ui, state, &m.visual_offset, actions));
}

fn offset_slider(ui: &mut Ui, state: &mut UiState, label: &str, actions: &mut Vec<UiAction>) {
    let (title, _) = ui.allocate_exact_size(vec2(200.0, 14.0), Sense::hover());
    widgets::text(
        ui.painter(),
        title.left_center(),
        Align2::LEFT_CENTER,
        &label.to_uppercase(),
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
        .name_inputs(label, |ui| {
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
