//! Small painted building blocks shared by the views: a named hit area
//! (every control is reachable by its AccessKit label), spaced text, and
//! line icons in the style of the web app's lucide set.

use std::f32::consts::{PI, TAU};

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Painter, Pos2, Rect, Response, Role, Sense, Shape,
    Stroke, Ui, Vec2, WidgetInfo,
};

/// zinc-300, text on idle pill buttons.
pub(crate) const TEXT_SOFT: Color32 = Color32::from_rgb(0xd4, 0xd4, 0xd8);
/// zinc-200, hovered icons.
pub(crate) const TEXT_HOT: Color32 = Color32::from_rgb(0xe4, 0xe4, 0xe7);
/// Between zinc-800 and zinc-700 (Tailwind's `zinc-750` in the web app).
pub(crate) const HOVER_FILL: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x33);

/// `idle` normally, brighter while hovered.
pub(crate) fn idle_color(resp: &Response, idle: Color32) -> Color32 {
    if resp.hovered() { TEXT_HOT } else { idle }
}

/// A button at `rect` named `label`. Disabled buttons keep their node (marked
/// disabled) but ignore clicks.
pub(crate) fn button(ui: &Ui, rect: Rect, id: Id, label: &str, enabled: bool) -> Response {
    named(ui, rect, id, label, enabled, Sense::click(), None)
}

/// A toggle button: like [`button`] but reports `on` as its toggled state.
pub(crate) fn toggle(ui: &Ui, rect: Rect, id: Id, label: &str, on: bool) -> Response {
    named(ui, rect, id, label, true, Sense::click(), Some(on))
}

/// A drag handle named `label`.
pub(crate) fn handle(ui: &Ui, rect: Rect, id: Id, label: &str) -> Response {
    named(ui, rect, id, label, true, Sense::click_and_drag(), None)
}

fn named(
    ui: &Ui,
    rect: Rect,
    id: Id,
    label: &str,
    enabled: bool,
    sense: Sense,
    toggled: Option<bool>,
) -> Response {
    let sense = if enabled { sense } else { Sense::hover() };
    let response = ui.interact(rect, id, sense);
    response.widget_info(|| match toggled {
        Some(on) => WidgetInfo::selected(Role::Button, enabled, on, label),
        None => WidgetInfo::labeled(Role::Button, enabled, label),
    });
    if !enabled {
        return response;
    }
    let cursor = match (sense.senses_drag(), response.dragged()) {
        (true, true) => CursorIcon::Grabbing,
        (true, false) => CursorIcon::Grab,
        (false, _) => CursorIcon::PointingHand,
    };
    response.on_hover_cursor(cursor)
}

/// Paints `text` with `spacing` points between letters, anchored at `pos` by `align`.
/// Returns the painted rect.
pub(crate) fn text(
    painter: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
    spacing: f32,
) -> Rect {
    let job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            color,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// Width `text` would take with [`text`].
pub(crate) fn text_width(painter: &Painter, text: &str, font: FontId, spacing: f32) -> f32 {
    let job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    painter.layout_job(job).size().x
}

/// Single-line text cut with "…" to fit `max_width`, anchored at `pos` by `align`.
pub(crate) fn elided(
    painter: &Painter,
    pos: Pos2,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Rect {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::truncate_at_width(max_width.max(0.0));
    let galley = painter.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

// ---- icons: `c` is the center, `s` the icon size (lucide's 24-unit box) ----

fn p(c: Pos2, s: f32, x: f32, y: f32) -> Pos2 {
    // lucide coordinates are 0..24 with the center at 12.
    c + Vec2::new(x - 12.0, y - 12.0) * (s / 24.0)
}

fn stroke(s: f32, color: Color32) -> Stroke {
    Stroke::new((2.0 * s / 24.0).max(1.0), color)
}

pub(crate) fn plus(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    let st = stroke(s, color);
    painter.line_segment([p(c, s, 5.0, 12.0), p(c, s, 19.0, 12.0)], st);
    painter.line_segment([p(c, s, 12.0, 5.0), p(c, s, 12.0, 19.0)], st);
}

pub(crate) fn minus(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    painter.line_segment([p(c, s, 5.0, 12.0), p(c, s, 19.0, 12.0)], stroke(s, color));
}

pub(crate) fn play(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    painter.add(Shape::convex_polygon(
        vec![p(c, s, 6.0, 3.0), p(c, s, 20.0, 12.0), p(c, s, 6.0, 21.0)],
        color,
        Stroke::NONE,
    ));
}

pub(crate) fn stop(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    let rect = Rect::from_min_max(p(c, s, 3.0, 3.0), p(c, s, 21.0, 21.0));
    painter.rect_filled(rect, 2.0 * s / 24.0, color);
}

/// lucide `settings-2`: two sliders.
pub(crate) fn settings(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    let st = stroke(s, color);
    painter.line_segment([p(c, s, 4.0, 7.0), p(c, s, 14.0, 7.0)], st);
    painter.circle_stroke(p(c, s, 17.0, 7.0), 3.0 * s / 24.0, st);
    painter.line_segment([p(c, s, 10.0, 17.0), p(c, s, 20.0, 17.0)], st);
    painter.circle_stroke(p(c, s, 7.0, 17.0), 3.0 * s / 24.0, st);
}

pub(crate) fn power(painter: &Painter, c: Pos2, s: f32, color: Color32, fill: Option<Color32>) {
    let st = stroke(s, color);
    let center = p(c, s, 12.0, 13.0);
    let r = 8.0 * s / 24.0;
    if let Some(fill) = fill {
        painter.circle_filled(center, r, fill);
    }
    // Open ring with the gap at the top.
    let gap = PI / 4.0;
    let points: Vec<Pos2> = (0..=32)
        .map(|i| {
            let a = -PI / 2.0 + gap + (TAU - 2.0 * gap) * i as f32 / 32.0;
            center + r * Vec2::angled(a)
        })
        .collect();
    painter.add(Shape::line(points, st));
    painter.line_segment([p(c, s, 12.0, 2.0), p(c, s, 12.0, 12.0)], st);
}

pub(crate) fn trash(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    let st = stroke(s, color);
    painter.line_segment([p(c, s, 3.0, 6.0), p(c, s, 21.0, 6.0)], st);
    painter.add(Shape::line(
        vec![
            p(c, s, 19.0, 6.0),
            p(c, s, 18.0, 20.0),
            p(c, s, 6.0, 20.0),
            p(c, s, 5.0, 6.0),
        ],
        st,
    ));
    painter.add(Shape::line(
        vec![
            p(c, s, 8.0, 6.0),
            p(c, s, 9.0, 3.0),
            p(c, s, 15.0, 3.0),
            p(c, s, 16.0, 6.0),
        ],
        st,
    ));
}

/// lucide `grip-vertical`: 2 x 3 dots.
pub(crate) fn grip(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    let r = 1.3 * s / 24.0;
    for x in [9.0, 15.0] {
        for y in [5.0, 12.0, 19.0] {
            painter.circle_filled(p(c, s, x, y), r.max(1.0), color);
        }
    }
}

/// lucide `zap`, filled.
pub(crate) fn zap(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    // Concave bolt drawn as two convex halves.
    painter.add(Shape::convex_polygon(
        vec![p(c, s, 13.0, 2.0), p(c, s, 4.0, 14.0), p(c, s, 12.0, 14.0)],
        color,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![
            p(c, s, 12.0, 10.0),
            p(c, s, 20.0, 10.0),
            p(c, s, 11.0, 22.0),
        ],
        color,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![
            p(c, s, 12.0, 14.0),
            p(c, s, 4.0, 14.0),
            p(c, s, 12.0, 10.0),
            p(c, s, 20.0, 10.0),
        ],
        color,
        Stroke::NONE,
    ));
}

/// lucide `picture-in-picture-2`: a window with a small window in its corner.
pub(crate) fn mini_window(painter: &Painter, c: Pos2, s: f32, color: Color32) {
    let st = stroke(s, color);
    let outer = Rect::from_min_max(p(c, s, 2.0, 4.0), p(c, s, 22.0, 20.0));
    painter.rect_stroke(outer, 2.0 * s / 24.0, st, egui::StrokeKind::Middle);
    let inner = Rect::from_min_max(p(c, s, 12.0, 12.0), p(c, s, 20.0, 18.0));
    painter.rect_filled(inner, 1.0 * s / 24.0, color);
}
