//! Rhythm topology. Placeholder until Task 11: one dashed ring per pulse.

use egui::{Color32, Pos2, Sense, Shape, Stroke, Vec2};

use crate::state::UiState;
use crate::theme;

/// Paints the topology in a `size` x `size` square.
pub fn topology(ui: &mut egui::Ui, state: &UiState, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let count = state.pattern.pulses.len().max(1) as f32;
    let outer = size * 0.42;
    for (i, pulse) in state.pattern.pulses.iter().enumerate() {
        let radius = outer * (1.0 - i as f32 / (count + 1.0));
        let [r, g, b] = pulse.color;
        let color = theme::with_alpha(Color32::from_rgb(r, g, b), 0.6);
        let points: Vec<Pos2> = (0..=96)
            .map(|k| {
                let a = k as f32 / 96.0 * std::f32::consts::TAU;
                center + radius * Vec2::angled(a)
            })
            .collect();
        painter.extend(Shape::dashed_line(
            &points,
            Stroke::new(1.0, color),
            4.0,
            4.0,
        ));
    }
}
