use egui::{Color32, Pos2, Rect, Sense, Vec2};
use egui_kittest::{Harness, SnapshotOptions, wgpu::WgpuTestRenderer};
use pm_ui::theme;

/// Per ruling R3: without a wgpu adapter the snapshot is skipped, not failed.
fn renderer() -> Option<WgpuTestRenderer> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(WgpuTestRenderer::new);
    std::panic::set_hook(previous);
    result.ok()
}

fn scene(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(520.0, 220.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, theme::BG);
    let left = Rect::from_min_size(rect.min + Vec2::new(20.0, 20.0), Vec2::new(230.0, 180.0));
    let right = Rect::from_min_size(rect.min + Vec2::new(270.0, 20.0), Vec2::new(230.0, 180.0));
    theme::paint_card(painter, left, theme::PALETTE[1], 0.0);
    theme::paint_top_accent(painter, left);
    theme::paint_card(painter, right, theme::PALETTE[0], 1.0);
    theme::paint_top_accent(painter, right);
    let c = left.center();
    theme::paint_glow(painter, c, 20.0, theme::PALETTE[1], 0.0);
    painter.circle_filled(c, 10.0, theme::DOT_INACTIVE);
    let c = right.center();
    theme::paint_glow(painter, c, 20.0, theme::PALETTE[0], 1.0);
    painter.circle_filled(c, 10.0, theme::PALETTE[0]);
    painter.text(
        Pos2::new(left.center().x, left.max.y - 30.0),
        egui::Align2::CENTER_CENTER,
        "120 BPM",
        theme::font_mono_medium(22.0),
        Color32::WHITE,
    );
    painter.text(
        Pos2::new(right.center().x, right.max.y - 30.0),
        egui::Align2::CENTER_CENTER,
        "Polyrhythm",
        theme::font_semibold(18.0),
        Color32::WHITE,
    );
}

#[test]
fn primitives_snapshot() {
    let Some(renderer) = renderer() else {
        eprintln!("SKIPPED primitives_snapshot: no wgpu adapter available");
        return;
    };
    let mut harness = Harness::builder()
        .with_size(Vec2::new(520.0, 220.0))
        .with_context_setup(theme::install)
        .renderer(renderer)
        .build_ui(scene);
    harness.run();
    harness.snapshot_options(
        "primitives_card_glow",
        &SnapshotOptions::new().threshold(2.0).max_failed_pixels(50),
    );
}

#[test]
fn glow_uses_at_most_eight_layers() {
    for intensity in [-1.0, 0.0, 0.25, 0.5, 1.0, 5.0] {
        assert!(
            theme::glow_layer_count(intensity) <= 8,
            "intensity {intensity}"
        );
    }
    assert_eq!(theme::glow_layer_count(0.0), 0);
}
