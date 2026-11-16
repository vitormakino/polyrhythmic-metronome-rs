//! The circle: one ring per pulse, one dot per beat, a central flash on the
//! downbeat. Each hit lights its dot and sends a ripple and a few sparks out
//! of it; the ring trembles and the pulse's color vibrates for a moment. A
//! coincidence adds a central glow and a light, deterministic shake.
//!
//! The web version put ring `i` at radius `110 - 25 i`, so from the 6th pulse
//! on the radius was <= 0 and the rings vanished, and with many beats the dots
//! became an unreadable cloud. Here the radii are spread evenly between 30 % and
//! 95 % of the half-size for any pulse count, and the dots shrink (never
//! overlapping their neighbours, on the ring or on the next ring) as beats pile up.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use super::{TopologyFrame, fx};
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

/// The circle's effect tunables. Fixed, not shown in the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CircleStyle {
    /// Time constant of a dot lighting up, s (0.01..=0.1).
    pub dot_rise_tau: f32,
    /// Time constant of the previous dot fading out, s (0.02..=0.2).
    pub dot_fall_tau: f32,
    /// Time constant of the central downbeat flash, s (0.05..=0.3).
    pub flash_tau: f32,
    /// Peak radial wobble of a ring on a hit, pt (0..=4).
    pub tremble: f32,
    /// Time constant of the wobble, s (0.05..=0.4).
    pub tremble_tau: f32,
    /// Most wobble lobes around a ring (2 per beat up to this, 4..=32).
    pub tremble_lobes: u32,
    /// Time constant of the color vibration, s (0.05..=0.4).
    pub color_tau: f32,
    /// Sparks per hit at full [`fx::budget`] (0..=16).
    pub sparks: usize,
    /// Spark speed as a fraction of the ring area's size per second (0.1..=0.6).
    pub spark_speed: f32,
    /// How far a ripple reaches, as a fraction of the ring area's size (0.05..=0.2).
    pub ripple_reach: f32,
    /// Peak shake of the whole drawing on a coincidence, pt (0..=5).
    pub shake: f32,
    /// Time constant of the shake and the coincidence glow, s (0.1..=0.4).
    pub shake_tau: f32,
}

impl CircleStyle {
    /// The values the circle is drawn with.
    pub const DEFAULT: CircleStyle = CircleStyle {
        dot_rise_tau: 0.03,
        dot_fall_tau: 0.06,
        flash_tau: 0.12,
        tremble: 2.2,
        tremble_tau: 0.18,
        tremble_lobes: 16,
        color_tau: 0.18,
        sparks: 6,
        spark_speed: 0.3,
        ripple_reach: 0.12,
        shake: 3.0,
        shake_tau: 0.22,
    };
}

/// Below this an effect's envelope is not drawn at all.
const VISIBLE: f32 = 0.01;
/// Segments of a ring's outline.
const RING_SEGMENTS: usize = 128;
/// Fewest segments of the colored outline of a trembling ring.
const MIN_TREMBLE_SEGMENTS: f32 = 32.0;

/// The effects of one young hit, painted over every ring.
struct Burst {
    at: Pos2,
    age: f32,
    seed: u64,
    color: Color32,
}

/// Paints the rings of `frame` centered in `area`, as big as its smaller side allows.
pub(super) fn paint(painter: &Painter, frame: &TopologyFrame, area: Rect) {
    let style = CircleStyle::DEFAULT;
    let t = frame.theme;
    let pulses = &frame.pulses;
    let layout_size = (area.width().min(area.height()) - 2.0 * MARGIN).max(1.0);
    let max_beats = pulses.iter().map(|p| p.beats).max().unwrap_or(1);
    let rings = ring_layout(pulses.len(), max_beats, layout_size);
    let gap = ring_gap(pulses.len(), layout_size);
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

    let flash = pulses
        .iter()
        .filter_map(|p| p.hit.filter(|h| h.beat == 0))
        .map(|h| fx::envelope(h.age, style.flash_tau))
        .fold(0.0, f32::max);
    theme::paint_glow(painter, center, layout_size * 0.4, t.strong, 0.6 * flash);
    if coincidence > VISIBLE {
        theme::paint_glow(
            painter,
            center,
            layout_size * 0.12 * (1.0 + coincidence),
            t.strong,
            0.5 * coincidence * fx::FX_STRENGTH,
        );
    }

    let mut bursts = Vec::new();
    for (index, (pulse, ring)) in pulses.iter().zip(&rings).enumerate() {
        let opacity = if pulse.enabled { 1.0 } else { 0.2 };
        let beats = pulse.beats;
        let age = pulse.hit.map_or(f32::INFINITY, |h| h.age);
        let color = if pulse.hit.is_some() {
            fx::vibrate(pulse.color, fx::envelope(age, style.color_tau), age)
        } else {
            pulse.color
        };

        // The ring, trembling with `lobes` waves for a moment after a hit.
        let shiver = fx::envelope(age, style.tremble_tau);
        let wobble = if shiver > VISIBLE {
            style.tremble * shiver * (age * 70.0 + index as f32).sin()
        } else {
            0.0
        };
        let lobes = (2 * beats).min(style.tremble_lobes) as f32;
        let outline = |segments: usize| -> Vec<Pos2> {
            (0..=segments)
                .map(|k| {
                    let a = k as f32 / segments as f32 * TAU;
                    let r = if wobble == 0.0 {
                        ring.radius
                    } else {
                        ring.radius + wobble * (a * lobes).sin()
                    };
                    center + r * Vec2::angled(a)
                })
                .collect()
        };
        painter.extend(Shape::dashed_line(
            &outline(RING_SEGMENTS),
            Stroke::new(1.0, theme::with_alpha(t.border, 0.5 * opacity)),
            4.0,
            4.0,
        ));
        if shiver > VISIBLE {
            let segments = (RING_SEGMENTS as f32 * budget).max(MIN_TREMBLE_SEGMENTS) as usize;
            painter.add(Shape::line(
                outline(segments),
                Stroke::new(1.0 + 1.5 * shiver, theme::with_alpha(color, 0.6 * shiver)),
            ));
        }

        let point = |i: u32| {
            let angle = i as f32 / beats as f32 * TAU - FRAC_PI_2;
            center + ring.radius * Vec2::angled(angle)
        };
        if polygon_visible(beats) {
            let mut points: Vec<Pos2> = (0..beats).map(point).collect();
            points.push(points[0]);
            painter.add(Shape::line(
                points,
                Stroke::new(0.5, theme::with_alpha(pulse.color, 0.1 * opacity)),
            ));
        }

        let idle = dot_radius(ring.radius, beats, gap);
        let lit = lit_radius(idle, ring.radius, beats);
        for i in 0..beats {
            let v = pulse
                .hit
                .map_or(0.0, |h| dot_level(&style, h.beat, h.age, i, beats));
            let at = point(i);
            let radius = idle + (lit - idle) * v;
            let fill = t.dot_inactive.lerp_to_gamma(color, v.clamp(0.0, 1.0));
            if v > 0.05 {
                theme::paint_glow(painter, at, radius * 2.6, color, 0.7 * v.min(1.0));
            }
            painter.circle_filled(at, radius, fill.gamma_multiply(opacity));
        }

        if let Some(hit) = pulse.hit {
            bursts.push(Burst {
                at: point(hit.beat),
                age: hit.age,
                seed: pulse.seed,
                color,
            });
        }
    }

    // Ripples and sparks over every ring.
    let sparks = (style.sparks as f32 * budget).round() as usize;
    for burst in bursts {
        fx::ripple(
            painter,
            burst.at,
            burst.age,
            burst.color,
            style.ripple_reach * layout_size,
        );
        let speed = style.spark_speed * layout_size;
        for (at, alpha) in fx::sparks(burst.seed, burst.at, burst.age, sparks, speed) {
            if alpha > 0.0 {
                let r = 1.8 * alpha + 0.4;
                painter.circle_filled(at, r * 2.2, theme::with_alpha(burst.color, 0.25 * alpha));
                painter.circle_filled(at, r, theme::with_alpha(burst.color, alpha));
            }
        }
    }
}

/// How lit dot `beat` of a `beats`-beat ring is, `age` seconds after
/// `lit_beat` was hit: the hit beat rises as `1 - exp(-age / rise)`, the one
/// before it fades as `exp(-age / fall)`, the rest stay idle.
fn dot_level(style: &CircleStyle, lit_beat: u32, age: f32, beat: u32, beats: u32) -> f32 {
    let age = age.max(0.0);
    let prev = (lit_beat + beats - 1) % beats;
    if beat == lit_beat {
        1.0 - (-age / style.dot_rise_tau).exp()
    } else if beat == prev {
        (-age / style.dot_fall_tau).exp()
    } else {
        0.0
    }
}
