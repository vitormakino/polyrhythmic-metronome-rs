//! Living polygons: one regular polygon per pulse. A runner travels the edges
//! at constant speed and reaches vertex `k` exactly on beat `k`, so every
//! runner starts at the top vertex on beat 1. A hit lights its vertex, makes
//! the polygon's edges wobble like elastic and sends sparks out of the vertex.
//! One beat is a single point and two beats a line. Above 24 beats there are
//! no edges or wobble, only the vertices and the runner. A coincidence adds a
//! central glow and a light, deterministic shake.

use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{Painter, Pos2, Rect, Shape, Stroke, Vec2};

use super::{TopologyFrame, fx, ring_gap, ring_layout};
use crate::theme;

/// The polygons' tunables. Fixed, not shown in the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolygonsStyle {
    /// Time constant of the edges' wobble and glow, s (0.1..=0.5).
    pub wobble_tau: f32,
    /// Time constant of a lit vertex's color, s (0.1..=0.4).
    pub hit_tau: f32,
    /// Peak sideways displacement of an edge's middle, pt.
    pub wobble: f32,
    /// Wobble frequency, rad/s.
    pub wobble_freq: f32,
    /// Points per edge while it wobbles, at full [`fx::budget`].
    pub edge_points: usize,
    /// Sparks per hit at full budget.
    pub sparks: usize,
    /// Spark speed, as a share of the polygons' size per second.
    pub spark_speed: f32,
    /// Past positions kept in a runner's trail at full budget.
    pub trail: usize,
    /// Step between trail positions, s.
    pub trail_step: f32,
    /// Peak shake of the whole drawing on a coincidence, pt (0..=5).
    pub shake: f32,
    /// Time constant of the shake and the coincidence glow, s (0.1..=0.4).
    pub shake_tau: f32,
}

impl PolygonsStyle {
    /// The values the polygons are drawn with.
    pub const DEFAULT: PolygonsStyle = PolygonsStyle {
        wobble_tau: 0.3,
        hit_tau: 0.2,
        wobble: 6.0,
        wobble_freq: 50.0,
        edge_points: 24,
        sparks: 10,
        spark_speed: 0.2,
        trail: 30,
        trail_step: 1.0 / 60.0,
        shake: 4.0,
        shake_tau: 0.22,
    };
}

/// Space kept free around the polygons for the lit vertices and the sparks.
const MARGIN: f32 = 10.0;
/// Below this an effect's envelope is not drawn at all.
const VISIBLE: f32 = 0.01;
/// Beats above which a polygon has no edges or wobble.
const MAX_EDGE_BEATS: u32 = 24;
/// Fewest points of a wobbling edge.
const MIN_EDGE_POINTS: usize = 4;

/// Vertex `k` of the polygon of `beats` vertices on the circle of radius `r`
/// around `center`: vertex 0 at 12 o'clock, then clockwise.
pub fn polygons_vertex(center: Pos2, r: f32, beats: u32, k: u32) -> Pos2 {
    let beats = beats.max(1);
    let angle = -FRAC_PI_2 + (k % beats) as f32 / beats as f32 * TAU;
    center + r * Vec2::angled(angle)
}

/// Where the runner is at pulse phase `pulse_phase` (`k + u`): on vertex `k`
/// at `u = 0`, `u` of the way along the edge to the next vertex otherwise, at
/// constant speed. One beat is a single point; two beats a line, there and back.
pub fn polygons_runner(center: Pos2, r: f32, beats: u32, pulse_phase: f64) -> Pos2 {
    let beats = beats.max(1);
    let pos = pulse_phase.rem_euclid(f64::from(beats));
    let k = (pos.floor() as u32).min(beats - 1);
    let u = (pos - f64::from(k)) as f32;
    let a = polygons_vertex(center, r, beats, k);
    let b = polygons_vertex(center, r, beats, k + 1);
    a.lerp(b, u)
}

/// Whether the polygon of `beats` vertices is drawn with edges: a line or
/// more, up to 24 vertices.
fn edges_visible(beats: u32) -> bool {
    (2..=MAX_EDGE_BEATS).contains(&beats)
}

/// Paints the polygons of `frame` centered in `area`, as big as its smaller side allows.
pub(super) fn paint(painter: &Painter, frame: &TopologyFrame, area: Rect) {
    let style = PolygonsStyle::DEFAULT;
    let t = frame.theme;
    let pulses = &frame.pulses;
    if pulses.is_empty() {
        return;
    }
    let size = (area.width().min(area.height()) - 2.0 * MARGIN).max(1.0);
    let half = size / 2.0;
    let max_beats = pulses.iter().map(|p| p.beats).max().unwrap_or(1);
    let rings = ring_layout(pulses.len(), max_beats, size);
    let gap = ring_gap(pulses.len(), size);
    let budget = fx::budget(pulses.len(), max_beats);

    let coincidence = frame
        .coincidence_age
        .map_or(0.0, |age| fx::envelope(age, style.shake_tau));
    let mut center = area.center();
    if let Some(age) = frame.coincidence_age.filter(|_| coincidence > VISIBLE) {
        let seed = pulses
            .iter()
            .find(|p| p.hit.is_some())
            .map_or(0, |p| p.seed);
        center += fx::shake(seed, style.shake * coincidence, age);
    }
    if coincidence > VISIBLE {
        theme::paint_glow(
            painter,
            center,
            half * 0.3 * (1.0 + coincidence),
            t.strong,
            0.5 * coincidence * fx::FX_STRENGTH,
        );
    }

    let measure_secs = frame.measure_secs.max(1e-3);
    let sparks = (style.sparks as f32 * budget).round() as usize;
    let mut runners = Vec::with_capacity(pulses.len());
    let mut bursts = Vec::new();
    for (pulse, ring) in pulses.iter().zip(&rings) {
        let opacity = if pulse.enabled { 1.0 } else { 0.2 };
        let beats = pulse.beats.max(1);
        let r = ring.radius;
        let vertex = |k: u32| polygons_vertex(center, r, beats, k);
        let (wobble_env, age) = pulse.hit.map_or((0.0, f32::INFINITY), |h| {
            (fx::envelope(h.age, style.wobble_tau), h.age)
        });
        let wobbling = wobble_env > VISIBLE;
        let e = if wobbling { wobble_env } else { 0.0 };
        let hit_energy = pulse
            .hit
            .map_or(0.0, |h| fx::envelope(h.age, style.hit_tau));
        let color = if hit_energy > VISIBLE {
            fx::vibrate(pulse.color, hit_energy, age)
        } else {
            pulse.color
        };

        // The edges, wobbling like elastic after a hit: sideways by
        // sin(u π) · sin(ω t + k), fading with the hit's age.
        if edges_visible(beats) {
            let stroke = Stroke::new(
                1.5 + e,
                theme::with_alpha(color, (0.35 + 0.4 * e) * opacity),
            );
            if wobbling {
                let points =
                    ((style.edge_points as f32 * budget).round() as usize).max(MIN_EDGE_POINTS);
                // Two beats are one line, not two.
                let edges = if beats == 2 { 1 } else { beats };
                let mut line = Vec::with_capacity(edges as usize * points + 1);
                for k in 0..edges {
                    let (a, b) = (vertex(k), vertex(k + 1));
                    let normal = (b - a).rot90().normalized();
                    let swing = (age * style.wobble_freq + k as f32).sin()
                        * style.wobble
                        * e
                        * fx::FX_STRENGTH;
                    for j in 0..points {
                        let u = j as f32 / points as f32;
                        let w = (u * std::f32::consts::PI).sin() * swing;
                        line.push(a.lerp(b, u) + w * normal);
                    }
                }
                line.push(vertex(edges));
                painter.add(Shape::line(line, stroke));
            } else if beats == 2 {
                painter.add(Shape::line_segment([vertex(0), vertex(1)], stroke));
            } else {
                painter.add(Shape::closed_line((0..beats).map(vertex).collect(), stroke));
            }
        }

        // The vertices: idle ones in one shape, the lit one on top with its glow.
        let idle = super::dot_radius(r, beats, gap);
        let lit_beat = pulse.hit.map(|h| h.beat);
        painter.add(Shape::Vec(
            (0..beats)
                .filter(|&k| Some(k) != lit_beat)
                .map(|k| {
                    Shape::circle_filled(
                        vertex(k),
                        idle,
                        theme::with_alpha(t.dot_inactive, opacity),
                    )
                })
                .collect(),
        ));
        if let Some(hit) = pulse.hit {
            let at = vertex(hit.beat);
            let lit = (idle * 1.6).max(idle + 1.5) + 3.0 * hit_energy;
            theme::paint_glow(painter, at, lit * 2.6, color, 0.7 * hit_energy);
            painter.circle_filled(at, lit, color);
            bursts.push((at, hit.age, pulse.seed, color));
        }

        // The runner's trail: where it was a few frames ago, from the phase,
        // at most one beat's travel so it never wraps around the polygon.
        let phase = pulse.phase.filter(|_| frame.playing);
        let runner_r = (idle * 1.5).clamp(2.5, 5.0);
        if let Some(phase) = phase {
            let beat_secs = measure_secs / f64::from(beats);
            let one_beat = beat_secs / f64::from(style.trail_step);
            let steps =
                ((style.trail as f32 * budget).round() as f64).min(one_beat.floor()) as usize;
            let rate = f64::from(beats) / measure_secs;
            let mut from = polygons_runner(center, r, beats, phase);
            for j in 1..=steps {
                let past = phase - rate * f64::from(style.trail_step) * j as f64;
                let to = polygons_runner(center, r, beats, past);
                let fade = 1.0 - j as f32 / (steps as f32 + 1.0);
                painter.add(Shape::line_segment(
                    [from, to],
                    Stroke::new(
                        (3.0 * fade).max(0.5),
                        theme::with_alpha(pulse.color, 0.9 * fade * opacity),
                    ),
                ));
                from = to;
            }
        }
        runners.push((
            polygons_runner(center, r, beats, phase.unwrap_or(0.0)),
            runner_r,
            opacity,
        ));
    }

    // Sparks over every polygon.
    let speed = style.spark_speed * size;
    for (at, age, seed, color) in bursts {
        for (p, alpha) in fx::sparks(seed, at, age, sparks, speed) {
            if alpha > 0.0 {
                let r = 1.8 * alpha + 0.4;
                painter.circle_filled(p, r * 2.2, theme::with_alpha(color, 0.25 * alpha));
                painter.circle_filled(p, r, theme::with_alpha(color, alpha));
            }
        }
    }

    // The runners, white, over everything.
    for (at, radius, opacity) in runners {
        if opacity >= 1.0 {
            theme::paint_glow(painter, at, radius * 3.0, t.strong, 0.45 * fx::FX_STRENGTH);
        }
        painter.circle_filled(at, radius, t.strong.gamma_multiply(opacity));
    }
}
