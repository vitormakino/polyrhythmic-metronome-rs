//! Orbits: a clock with one ring per pulse. Each pulse's ball makes one turn
//! per beat at constant speed and passes the 12 o'clock gate exactly on its
//! beat, so every ball lines up at 12 on beat 1. A hit lights the gate, rings
//! the gate point with a ripple and brightens the pulse's ring. A pulse whose
//! turn is too fast to follow is drawn as a faint full ring instead of a ball
//! and trail. A coincidence adds a central glow and a light, deterministic shake.

use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2};

use super::{TopologyFrame, fx, ring_layout};
use crate::theme;

/// The orbits' tunables. Fixed, not shown in the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitsStyle {
    /// Time constant of a hit's glow, ring brightness and color, s (0.1..=0.4).
    pub hit_tau: f32,
    /// A turn shorter than this, s, draws as a faint ring instead of a ball.
    pub min_turn_secs: f32,
    /// Ripple reach at the gate, as a share of the ring area's size.
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

impl OrbitsStyle {
    /// The values the orbits are drawn with.
    pub const DEFAULT: OrbitsStyle = OrbitsStyle {
        hit_tau: 0.2,
        min_turn_secs: 0.15,
        ripple_reach: 0.1,
        trail: 16,
        trail_step: 1.0 / 60.0,
        shake: 4.0,
        shake_tau: 0.22,
    };
}

/// Space kept free around the rings for the gate, the ripple and the balls.
const MARGIN: f32 = 10.0;
/// Below this an effect's envelope is not drawn at all.
const VISIBLE: f32 = 0.01;
/// Segments of a ring's outline.
const RING_SEGMENTS: usize = 96;

/// Angle of a ball at pulse phase `pulse_phase` (`k + u`): one turn per beat,
/// from 12 o'clock (`-π/2`) clockwise, so it is at 12 on every integer phase.
pub fn orbits_angle(pulse_phase: f64) -> f32 {
    -FRAC_PI_2 + pulse_phase.rem_euclid(1.0) as f32 * TAU
}

/// Where a ball on the ring of radius `r` around `center` is at `pulse_phase`.
pub fn orbits_ball(center: Pos2, r: f32, pulse_phase: f64) -> Pos2 {
    center + r * Vec2::angled(orbits_angle(pulse_phase))
}

/// Whether a pulse of `beats` beats in a measure of `measure_secs` turns too
/// fast to follow: its turn (`measure_secs / beats`) lasts under 0.15 s.
pub fn orbits_is_ring(measure_secs: f64, beats: u32) -> bool {
    measure_secs / f64::from(beats.max(1)) < f64::from(OrbitsStyle::DEFAULT.min_turn_secs)
}

/// Paints the orbits of `frame` centered in `area`, as big as its smaller side allows.
pub(super) fn paint(painter: &Painter, frame: &TopologyFrame, area: Rect) {
    let style = OrbitsStyle::DEFAULT;
    let t = frame.theme;
    let pulses = &frame.pulses;
    if pulses.is_empty() {
        return;
    }
    let size = (area.width().min(area.height()) - 2.0 * MARGIN).max(1.0);
    let half = size / 2.0;
    let max_beats = pulses.iter().map(|p| p.beats).max().unwrap_or(1);
    let rings = ring_layout(pulses.len(), max_beats, size);
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

    let measure_secs = frame.measure_secs.max(1e-3);
    let steps = (style.trail as f32 * budget).round() as usize;
    let ball_r = (half * 0.035).clamp(2.5, 6.0);
    let mut strongest = 0.0f32;
    let mut balls = Vec::with_capacity(pulses.len());
    for (pulse, ring) in pulses.iter().zip(&rings) {
        let opacity = if pulse.enabled { 1.0 } else { 0.2 };
        let (energy, age) = pulse.hit.map_or((0.0, f32::INFINITY), |h| {
            (fx::envelope(h.age, style.hit_tau), h.age)
        });
        let lit = energy > VISIBLE;
        let energy = if lit { energy } else { 0.0 };
        strongest = strongest.max(energy);
        let color = if lit {
            fx::vibrate(pulse.color, energy, age)
        } else {
            pulse.color
        };
        let fast = orbits_is_ring(measure_secs, pulse.beats);

        // The ring; a fast pulse's ball is smeared over all of it.
        let base = if fast { 0.4 } else { 0.18 };
        painter.add(Shape::closed_line(
            (0..RING_SEGMENTS)
                .map(|k| center + ring.radius * Vec2::angled(k as f32 / RING_SEGMENTS as f32 * TAU))
                .collect(),
            Stroke::new(
                1.5,
                theme::with_alpha(pulse.color, (base + 0.4 * energy) * opacity),
            ),
        ));

        // The ripple where the ball touched the gate.
        if lit {
            fx::ripple(
                painter,
                center + Vec2::new(0.0, -ring.radius),
                age,
                color,
                style.ripple_reach * size,
            );
        }
        if fast {
            continue;
        }

        // The ball, and where it was a few frames ago (from the phase).
        let phase = pulse.phase.unwrap_or(0.0);
        if let Some(phase) = pulse.phase.filter(|_| steps > 0) {
            let rate = f64::from(pulse.beats) / measure_secs;
            for j in (1..=steps).rev() {
                let past = phase - rate * f64::from(style.trail_step) * j as f64;
                let fade = 1.0 - j as f32 / (steps as f32 + 1.0);
                painter.circle_filled(
                    orbits_ball(center, ring.radius, past),
                    ball_r * (0.5 + 0.5 * fade),
                    theme::with_alpha(color, 0.45 * fade * opacity),
                );
            }
        }
        balls.push((
            orbits_ball(center, ring.radius, phase),
            color,
            energy,
            opacity,
        ));
    }

    // The 12 o'clock gate, glowing with the strongest current envelope.
    let gate_top = pos2(center.x, center.y - half * 1.04);
    let gate_bottom = pos2(center.x, center.y - half * 0.22);
    if strongest > VISIBLE {
        painter.add(Shape::line_segment(
            [gate_top, gate_bottom],
            Stroke::new(
                7.0 * strongest,
                theme::with_alpha(t.strong, 0.25 * strongest),
            ),
        ));
    }
    painter.add(Shape::line_segment(
        [gate_top, gate_bottom],
        Stroke::new(2.0, theme::with_alpha(t.strong, 0.45 + 0.55 * strongest)),
    ));

    for (at, color, energy, opacity) in balls {
        let r = ball_r * (1.0 + 0.4 * energy);
        if opacity >= 1.0 {
            theme::paint_glow(painter, at, r * 3.0, color, 0.45 + 0.4 * energy);
        }
        painter.circle_filled(at, r, color.gamma_multiply(opacity));
    }

    // The hub, swelling with the coincidence.
    if coincidence > VISIBLE {
        theme::paint_glow(
            painter,
            center,
            half * 0.3 * (1.0 + coincidence),
            t.strong,
            0.5 * coincidence * fx::FX_STRENGTH,
        );
    }
    painter.circle_filled(center, 3.0 + 4.0 * coincidence, t.strong);
}
