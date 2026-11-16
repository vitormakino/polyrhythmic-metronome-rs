//! Arcs: a semicircle per pulse over a shared baseline. Each pulse's ball
//! crosses its arc once per beat at constant speed, alternating direction, so
//! it touches the baseline exactly on every beat (all of them on the left at
//! beat 1). A hit makes the baseline vibrate, grows a half ripple at the
//! contact point and lights the pulse's arc. A coincidence adds a central
//! glow and a light, deterministic shake.

use std::f32::consts::PI;

use egui::{Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2};

use super::{TopologyFrame, fx};
use crate::theme;

/// The arcs' tunables. Fixed, not shown in the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcsStyle {
    /// Smallest arc radius, as a share of half the width.
    pub inner: f32,
    /// Largest arc radius (pulse 0), as a share of half the width.
    pub outer: f32,
    /// Peak vibration of the baseline, pt.
    pub vibration: f32,
    /// Time constant of a hit's glow, vibration and color, s (0.1..=0.4).
    pub hit_tau: f32,
    /// How long the half ripple lives, s.
    pub ripple_secs: f32,
    /// Reach of the half ripple, pt (at the sketch's size).
    pub ripple_reach: f32,
    /// Past positions kept in a ball's trail at full [`fx::budget`].
    pub trail: usize,
    /// Step between trail positions, s.
    pub trail_step: f32,
    /// Peak shake of the whole drawing on a coincidence, pt (0..=5).
    pub shake: f32,
    /// Time constant of the shake and the coincidence glow, s (0.1..=0.4).
    pub shake_tau: f32,
}

impl ArcsStyle {
    /// The values the arcs are drawn with.
    pub const DEFAULT: ArcsStyle = ArcsStyle {
        inner: 0.30,
        outer: 0.95,
        vibration: 1.8,
        hit_tau: 0.2,
        ripple_secs: 0.6,
        ripple_reach: 22.0,
        trail: 12,
        trail_step: 1.0 / 60.0,
        shake: 3.0,
        shake_tau: 0.22,
    };
}

/// Room kept free left and right of the baseline, above the outer arc and below the baseline.
const SIDE_MARGIN: f32 = 8.0;
const TOP_MARGIN: f32 = 10.0;
const BOTTOM_MARGIN: f32 = 10.0;
/// Below this an effect's envelope is not drawn at all.
const VISIBLE: f32 = 0.01;
/// Segments of an arc, and of the baseline, at most.
const ARC_SEGMENTS: usize = 48;
const BASELINE_SEGMENTS: usize = 120;
/// The size the sketch's pt values were drawn for: half the width of its canvas.
const SKETCH_HALF: f32 = 150.0;

/// Radius of pulse `index`'s arc among `count`, given `half` (half the
/// width): the outermost (pulse 0) at 95 %, the innermost at 30 %.
pub fn arcs_radius(index: usize, count: usize, half: f32) -> f32 {
    let style = ArcsStyle::DEFAULT;
    let share = if count <= 1 {
        style.outer
    } else {
        style.outer - (style.outer - style.inner) * index as f32 / (count - 1) as f32
    };
    half * share
}

/// Where a ball on the arc of radius `r` over `center` (a point of the
/// baseline) is at pulse phase `pulse_phase` (`k + u`: beat `k`, fraction `u`).
/// An even beat crosses left to right, an odd one back, so it is on the
/// baseline exactly at every integer phase (on the left at phase 0 of an
/// even measure start).
pub fn arcs_ball(center: Pos2, r: f32, pulse_phase: f64) -> Pos2 {
    arcs_ball_at(center, r, pulse_phase, 0)
}

/// [`arcs_ball`] on the beat numbered `offset` more than the phase's own:
/// only its parity matters, and it decides the direction. Passing
/// `measure_count * beats` makes direction alternate across measures too, so
/// an odd pulse does not jump at the wrap.
pub fn arcs_ball_at(center: Pos2, r: f32, pulse_phase: f64, offset: u64) -> Pos2 {
    let k = pulse_phase.floor();
    let u = (pulse_phase - k) as f32;
    let s = if (k as u64 + offset).is_multiple_of(2) {
        u
    } else {
        1.0 - u
    };
    let angle = PI + s * PI;
    pos2(center.x + r * angle.cos(), center.y + r * angle.sin())
}

/// Where the ball touched the baseline on beat `beat` of a pulse of `beats`
/// beats, now at `phase` in measure `measures`. A hit on a beat past the
/// current one happened before the measure wrapped, in the measure before.
pub fn arcs_contact(
    center: Pos2,
    r: f32,
    beat: u32,
    beats: u32,
    measures: u64,
    phase: Option<f64>,
) -> Pos2 {
    let current = phase.map_or(0, |p| p.floor() as u32);
    let measure = if beat > current {
        measures.saturating_sub(1)
    } else {
        measures
    };
    arcs_ball_at(center, r, f64::from(beat), measure * u64::from(beats))
}

/// The points of the half circle of radius `r` over `center`, left to right.
fn arc_points(center: Pos2, r: f32, segments: usize) -> Vec<Pos2> {
    (0..=segments)
        .map(|k| {
            let angle = PI + PI * k as f32 / segments as f32;
            pos2(center.x + r * angle.cos(), center.y + r * angle.sin())
        })
        .collect()
}

/// Paints the arcs of `frame` in `area`.
pub(super) fn paint(painter: &Painter, frame: &TopologyFrame, area: Rect) {
    let style = ArcsStyle::DEFAULT;
    let t = frame.theme;
    let pulses = &frame.pulses;
    if pulses.is_empty() || area.height() < 1.0 {
        return;
    }
    let max_beats = pulses.iter().map(|p| p.beats).max().unwrap_or(1);
    let budget = fx::budget(pulses.len(), max_beats);

    let coincidence = frame
        .coincidence_age
        .map_or(0.0, |age| fx::envelope(age, style.shake_tau));
    let mut shift = Vec2::ZERO;
    if let Some(age) = frame.coincidence_age.filter(|_| coincidence > VISIBLE) {
        let seed = pulses
            .iter()
            .find(|p| p.hit.is_some())
            .map_or(0, |p| p.seed);
        shift = fx::shake(seed, style.shake * coincidence, age);
    }
    let area = area.translate(shift);

    // The semicircles never get squashed: the biggest one that fits the
    // area's width and its height above the baseline.
    let base_y = area.bottom() - BOTTOM_MARGIN;
    let half = (area.width() / 2.0 - SIDE_MARGIN).min(base_y - area.top() - TOP_MARGIN);
    if half < 4.0 {
        return;
    }
    let center = pos2(area.center().x, base_y);
    let scale = half / SKETCH_HALF;
    if coincidence > VISIBLE {
        theme::paint_glow(
            painter,
            pos2(center.x, center.y - half * 0.35),
            half * 1.1,
            t.strong,
            0.5 * coincidence * fx::FX_STRENGTH,
        );
    }

    let ball_r = (5.0 * scale).clamp(3.0, 6.0);
    let count = pulses.len();
    let steps = (style.trail as f32 * budget).round() as usize;
    let measure_secs = frame.measure_secs.max(1e-3);
    let measures = frame.measure_count;
    let mut balls = Vec::with_capacity(count);
    let mut strongest = (0.0f32, 0.0f32);
    for (index, pulse) in pulses.iter().enumerate() {
        let r = arcs_radius(index, count, half);
        let opacity = if pulse.enabled { 1.0 } else { 0.2 };
        let (energy, age) = pulse.hit.map_or((0.0, f32::INFINITY), |h| {
            (fx::envelope(h.age, style.hit_tau), h.age)
        });
        let lit = energy > VISIBLE;
        if lit && energy > strongest.0 {
            strongest = (energy, age);
        }
        let color = match pulse.hit {
            Some(h) if lit => fx::vibrate(pulse.color, energy, h.age),
            _ => pulse.color,
        };
        let energy = if lit { energy } else { 0.0 };

        // The arc, glowing while the pulse is hit.
        let points = arc_points(center, r, ARC_SEGMENTS);
        if lit {
            painter.add(Shape::line(
                points.clone(),
                Stroke::new(6.0 * energy, theme::with_alpha(color, 0.25 * energy)),
            ));
        }
        painter.add(Shape::line(
            points,
            Stroke::new(
                1.5 + 1.5 * energy,
                theme::with_alpha(color, (0.25 + 0.6 * energy) * opacity),
            ),
        ));

        // The half ripple at the contact point, where the ball touched down.
        if let (Some(h), true) = (pulse.hit, pulse.enabled) {
            let u = h.age / style.ripple_secs;
            if (0.0..1.0).contains(&u) {
                let at = arcs_contact(center, r, h.beat, pulse.beats, measures, pulse.phase);
                let reach = (4.0 + style.ripple_reach * u) * scale.max(0.5);
                painter.add(Shape::line(
                    arc_points(at, reach, 24),
                    Stroke::new(
                        2.0,
                        theme::with_alpha(pulse.color, (1.0 - u) * 0.9 * fx::FX_STRENGTH),
                    ),
                ));
            }
        }

        // The ball, and the positions it had a few frames ago (from the phase).
        let phase = pulse.phase.unwrap_or(0.0);
        if let Some(phase) = pulse.phase.filter(|_| steps > 0) {
            let rate = f64::from(pulse.beats) / measure_secs;
            let cycle = f64::from(pulse.beats);
            for j in (1..=steps).rev() {
                let back = rate * f64::from(style.trail_step) * j as f64;
                let raw = phase - back;
                let past = raw.rem_euclid(cycle);
                let measure = measures.saturating_sub(u64::from(raw < 0.0));
                let fade = 1.0 - j as f32 / (steps as f32 + 1.0);
                painter.circle_filled(
                    arcs_ball_at(center, r, past, measure * u64::from(pulse.beats)),
                    ball_r * (0.5 + 0.5 * fade),
                    theme::with_alpha(color, 0.45 * fade * opacity),
                );
            }
        }
        balls.push((
            arcs_ball_at(center, r, phase, measures * u64::from(pulse.beats)),
            color,
            energy,
            opacity,
        ));
    }

    // The baseline, vibrating on any hit.
    let (hit, age) = strongest;
    let left = area.left() + SIDE_MARGIN * 0.75;
    let right = area.right() - SIDE_MARGIN * 0.75;
    let line: Vec<Pos2> = (0..=BASELINE_SEGMENTS)
        .map(|k| {
            let u = k as f32 / BASELINE_SEGMENTS as f32;
            let wobble = (k as f32 * 0.6 + age * 80.0).sin() * style.vibration * hit;
            pos2(left + (right - left) * u, base_y + wobble)
        })
        .collect();
    if hit > VISIBLE {
        painter.add(Shape::line(
            line.clone(),
            Stroke::new(5.0 * hit, theme::with_alpha(t.strong, 0.25 * hit)),
        ));
    }
    painter.add(Shape::line(
        line,
        Stroke::new(2.0, theme::with_alpha(t.strong, 0.5 + 0.5 * hit)),
    ));

    for (at, color, energy, opacity) in balls {
        let r = ball_r * (1.0 + 0.4 * energy);
        if opacity >= 1.0 {
            theme::paint_glow(painter, at, r * 3.0, color, 0.45 + 0.4 * energy);
        }
        painter.circle_filled(at, r, color.gamma_multiply(opacity));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radii_are_distinct_and_inside() {
        for count in 1..=16 {
            let mut last = f32::INFINITY;
            for i in 0..count {
                let r = arcs_radius(i, count, 100.0);
                assert!((30.0 - 1e-3..=95.0 + 1e-3).contains(&r), "{r}");
                assert!(r < last);
                last = r;
            }
        }
    }
}
