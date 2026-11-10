//! The mini window: a small always-on-top card with the animated topology,
//! the tempo (buttons, scroll and drag on the number) and Play/Stop. No pulse
//! editing; the expand button goes back to the full window.

use std::time::Instant;

use egui::{Rect, Ui, UiBuilder, Vec2, pos2, vec2};

use super::bpm_card::{bpm_gestures, bpm_stepper};
use super::full::play_button;
use super::topology::topology;
use super::widgets;
use crate::state::{UiAction, UiState, WindowMode};
use crate::theme;

/// Default window size (pt).
pub const DEFAULT_SIZE: Vec2 = Vec2::new(260.0, 300.0);
/// Smallest window size (pt).
pub const MIN_SIZE: Vec2 = Vec2::new(220.0, 260.0);

const PAD: f32 = 12.0;
const GAP: f32 = 6.0;
const BPM_H: f32 = 52.0;
const PLAY_H: f32 = 40.0;

/// Draws the mini view over the whole of `ui`'s rect: a rounded dark card
/// (the window is transparent) holding topology, BPM and Play/Stop.
pub fn mini_view(ui: &mut Ui, state: &mut UiState, _now: Instant) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let card = ui.max_rect();
    theme::paint_card(ui.painter(), card, theme::BORDER, 0.0);
    let inner = card.shrink(PAD);

    let play = Rect::from_min_max(pos2(inner.left(), inner.bottom() - PLAY_H), inner.max);
    let bpm = Rect::from_min_max(
        pos2(inner.left(), play.top() - GAP - BPM_H),
        pos2(inner.right(), play.top() - GAP),
    );
    let topo = Rect::from_min_max(inner.min, pos2(inner.right(), bpm.top() - GAP));
    let side = topo.width().min(topo.height()).max(1.0);
    let square = Rect::from_center_size(topo.center(), Vec2::splat(side));
    ui.scope_builder(UiBuilder::new().max_rect(square), |ui| {
        topology(ui, state, side);
    });

    let number = bpm_stepper(
        ui,
        bpm.center(),
        theme::font_light(40.0),
        32.0,
        state,
        &mut actions,
    );
    bpm_gestures(ui, number.expand2(vec2(4.0, 6.0)), state, &mut actions);

    play_button(ui, play, state, false, &mut actions);

    // Expand: a small corner button, over the card's top-right.
    let expand = Rect::from_center_size(
        pos2(inner.right() - 12.0, inner.top() + 12.0),
        Vec2::splat(24.0),
    );
    let resp = widgets::button(
        ui,
        expand,
        ui.make_persistent_id("expand"),
        &state.messages.expand,
        true,
    );
    if resp.hovered() {
        ui.painter().rect_filled(expand, 6.0, theme::BORDER);
    }
    widgets::mini_window(
        ui.painter(),
        expand.center(),
        14.0,
        widgets::idle_color(&resp, theme::TEXT_MUTED),
    );
    if resp.clicked() {
        state.mode = WindowMode::Full;
        actions.push(UiAction::ToggleMini);
    }
    actions
}
