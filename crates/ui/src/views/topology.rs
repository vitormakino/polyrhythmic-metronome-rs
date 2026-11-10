//! Rhythm topology: one ring per pulse, one dot per beat, a central flash on
//! the downbeat and a coincidence light when every pulse hits beat 0 together.
//!
//! The web version put ring `i` at radius `110 - 25 i`, so from the 6th pulse
//! on the radius was <= 0 and the rings vanished, and with many beats the dots
//! became an unreadable cloud. Here the radii are spread evenly between 30 % and
//! 95 % of the half-size for any pulse count, and the dots shrink (never
//! overlapping their neighbours, on the ring or on the next ring) as beats pile up.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use egui::{Align2, Pos2, Role, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo, pos2};

use super::widgets;
use crate::state::UiState;
use crate::theme;

/// Ring radii as fractions of the half-size.
const OUTER: f32 = 0.95;
const INNER: f32 = 0.30;
/// Dots at rest: radius range (web: 3.5) and the lit radius (web: 7).
const DOT_MIN: f32 = 1.5;
const DOT_MAX: f32 = 3.5;
const DOT_LIT: f32 = 7.0;
/// A dot may take at most this fraction of the room towards its neighbour, so
/// neighbours are always separated by a visible gap.
const ROOM: f32 = 0.45;
/// Beats above which the connecting polygon is dropped (it only adds noise).
const MAX_POLYGON_BEATS: u32 = 24;
/// Space kept free around the rings for the lit dots and their halo.
const MARGIN: f32 = 8.0;

/// Geometry of one ring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RingGeom {
    pub radius: f32,
    /// Radius of an idle dot.
    pub dot_radius: f32,
    pub show_polygon: bool,
}

/// Rings for `pulse_count` pulses in a `size` x `size` square, outermost
/// (first pulse) first. Radii are spread evenly between 30 % and 95 % of
/// `size / 2`, so every pulse gets a ring. `dot_radius` and `show_polygon` are
/// computed for a ring of `max_beats` beats, which is the tightest case.
pub fn ring_layout(pulse_count: usize, max_beats: u32, size: f32) -> Vec<RingGeom> {
    let half = size / 2.0;
    let (outer, inner) = (OUTER * half, INNER * half);
    let step = if pulse_count > 1 {
        (outer - inner) / (pulse_count - 1) as f32
    } else {
        0.0
    };
    (0..pulse_count)
        .map(|i| {
            let radius = outer - step * i as f32;
            RingGeom {
                radius,
                dot_radius: dot_radius(radius, max_beats, ring_gap(pulse_count, size)),
                show_polygon: polygon_visible(max_beats),
            }
        })
        .collect()
}

/// Distance between two neighbouring rings (infinite with a single ring).
pub fn ring_gap(pulse_count: usize, size: f32) -> f32 {
    if pulse_count > 1 {
        (OUTER - INNER) * size / 2.0 / (pulse_count - 1) as f32
    } else {
        f32::INFINITY
    }
}

/// Radius of an idle dot on a ring of `radius` with `beats` dots and `ring_gap`
/// to the next ring: `clamp(π r / beats * 0.35, 1.5, 3.5)`, then shrunk when
/// needed so it never touches its neighbours (not overlapping outranks the 1.5 floor).
pub fn dot_radius(radius: f32, beats: u32, ring_gap: f32) -> f32 {
    let beats = beats.max(1);
    let mut dot = (PI * radius / beats as f32 * 0.35).clamp(DOT_MIN, DOT_MAX);
    if beats >= 2 {
        let chord = 2.0 * radius * (PI / beats as f32).sin();
        dot = dot.min(ROOM * chord);
    }
    dot.min(ROOM * ring_gap)
}

/// Whether the polygon joining the dots is drawn: more than 2 beats (a line is
/// not a polygon) and at most 24 (beyond that it is a second circle).
pub fn polygon_visible(beats: u32) -> bool {
    beats > 2 && beats <= MAX_POLYGON_BEATS
}

/// Radius of a fully lit dot: twice the idle one up to 7, but never reaching
/// its idle neighbours.
fn lit_radius(dot: f32, radius: f32, beats: u32) -> f32 {
    let mut lit = (2.0 * dot).min(DOT_LIT);
    if beats >= 2 {
        let chord = 2.0 * radius * (PI / beats as f32).sin();
        lit = lit.min(chord - dot - 0.5);
    }
    lit.max(dot)
}

/// Height reserved under the rings for the indicators.
fn strip_height(size: f32) -> f32 {
    (size * 0.1).clamp(18.0, 30.0)
}

/// Paints the topology in a `size` x `size` square: the rings above, the
/// indicators in a strip at the bottom (outside the ring area).
pub fn topology(ui: &mut Ui, state: &UiState, size: f32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, &state.messages.topology));
    let painter = ui.painter_at(rect);
    let t = state.theme.clone();

    let strip = strip_height(size);
    let area = size - strip;
    let center = pos2(rect.center().x, rect.top() + area / 2.0);
    let layout_size = (area - 2.0 * MARGIN).max(1.0);

    let pulses = &state.pattern.pulses;
    let max_beats = pulses.iter().map(|p| p.beats).max().unwrap_or(1);
    let rings = ring_layout(pulses.len(), max_beats, layout_size);
    let gap = ring_gap(pulses.len(), layout_size);

    let flash = state.downbeat_flash();
    theme::paint_glow(&painter, center, layout_size * 0.4, t.strong, 0.6 * flash);

    for (pulse, ring) in pulses.iter().zip(&rings) {
        let color = t.pulse_color(pulse.color);
        let opacity = if pulse.enabled { 1.0 } else { 0.2 };
        let beats = pulse.beats.max(1);

        let circle: Vec<Pos2> = (0..=128)
            .map(|k| center + ring.radius * Vec2::angled(k as f32 / 128.0 * TAU))
            .collect();
        painter.extend(Shape::dashed_line(
            &circle,
            Stroke::new(1.0, theme::with_alpha(t.border, 0.5 * opacity)),
            4.0,
            4.0,
        ));

        let point = |i: u32| {
            let angle = i as f32 / beats as f32 * TAU - FRAC_PI_2;
            center + ring.radius * Vec2::angled(angle)
        };
        if polygon_visible(beats) {
            let mut points: Vec<Pos2> = (0..beats).map(point).collect();
            points.push(points[0]);
            painter.add(Shape::line(
                points,
                Stroke::new(0.5, theme::with_alpha(color, 0.1 * opacity)),
            ));
        }

        let idle = dot_radius(ring.radius, beats, gap);
        let lit = lit_radius(idle, ring.radius, beats);
        for i in 0..beats {
            let v = state.dot(pulse.id, i);
            let at = point(i);
            let radius = idle + (lit - idle) * v;
            let fill = t.dot_inactive.lerp_to_gamma(color, v.clamp(0.0, 1.0));
            if v > 0.05 {
                theme::paint_glow(&painter, at, radius * 2.6, color, 0.7 * v.min(1.0));
            }
            painter.circle_filled(at, radius, fill.gamma_multiply(opacity));
        }
    }

    indicators(&painter, rect, strip, size, state);
}

/// Coincidence light and engine status in the strip under the rings.
fn indicators(painter: &egui::Painter, rect: egui::Rect, strip: f32, size: f32, state: &UiState) {
    const LIGHT: f32 = 3.0;
    const SPACING: f32 = 1.2;
    const GAP: f32 = 6.0;
    const BETWEEN: f32 = 20.0;
    let m = &state.messages;
    let t = &state.theme;
    let font = theme::font_mono(8.0);
    let y = rect.bottom() - strip / 2.0;
    let coincidence_w = widgets::text_width(painter, &m.coincidence, font.clone(), SPACING);
    let engine_w = widgets::text_width(painter, &m.engine, font.clone(), SPACING);
    let light_w = 2.0 * LIGHT * 1.25;
    let first = light_w + GAP + coincidence_w;
    let second = engine_w + GAP + light_w;
    // The engine light is dropped when the square is too narrow for both.
    let both = first + BETWEEN + second <= size;
    let total = if both {
        first + BETWEEN + second
    } else {
        first
    };
    let mut x = rect.center().x - total / 2.0;

    let lit = state.coincidence();
    let light = pos2(x + light_w / 2.0, y);
    if lit {
        theme::paint_glow(painter, light, 9.0, t.strong, 1.0);
        painter.circle_filled(light, LIGHT * 1.25, t.strong);
    } else {
        painter.circle_filled(light, LIGHT, t.border);
    }
    x += light_w + GAP;
    widgets::text(
        painter,
        pos2(x, y),
        Align2::LEFT_CENTER,
        &m.coincidence,
        font.clone(),
        t.text_muted,
        SPACING,
    );
    if both {
        x += coincidence_w + BETWEEN;
        widgets::text(
            painter,
            pos2(x, y),
            Align2::LEFT_CENTER,
            &m.engine,
            font,
            t.text_muted,
            SPACING,
        );
        let light = pos2(x + engine_w + GAP + light_w / 2.0, y);
        if state.playing {
            theme::paint_glow(painter, light, 8.0, t.accent, 0.8);
            painter.circle_filled(light, LIGHT, t.accent);
        } else {
            painter.circle_filled(light, LIGHT, t.dot_inactive);
        }
    }
}
