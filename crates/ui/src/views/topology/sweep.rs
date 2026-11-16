//! Sweep: a clock with one hand. The hand makes one turn per measure at
//! constant speed and leaves a light trail behind it. Every pulse has a ring
//! of radial marks, one per beat, at `beat / beats` of the turn, so the hand
//! crosses a mark exactly on that beat and all rings start together at 12
//! o'clock. A hit lights its mark, thicker and glowing. A coincidence makes
//! the centre dot glow and adds a light, deterministic shake.

use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{Painter, Pos2, Rect, Shape, Stroke, Vec2};

use super::{TopologyFrame, fx, ring_layout};
use crate::theme;

/// The sweep's tunables. Fixed, not shown in the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepStyle {
    /// Time constant of a hit's glow and mark, s (0.1..=0.4).
    pub hit_tau: f32,
    /// Past hand angles kept in the trail at full [`fx::budget`].
    pub trail: usize,
    /// Step between trail angles, s.
    pub trail_step: f32,
    /// Peak shake of the whole drawing on a coincidence, pt (0..=5).
    pub shake: f32,
    /// Time constant of the shake and the coincidence glow, s (0.1..=0.4).
    pub shake_tau: f32,
}

impl SweepStyle {
    /// The values the sweep is drawn with.
    pub const DEFAULT: SweepStyle = SweepStyle {
        hit_tau: 0.2,
        trail: 20,
        trail_step: 1.0 / 60.0,
        shake: 4.0,
        shake_tau: 0.22,
    };
}

/// Space kept free around the rings for the marks and the hand's glow.
const MARGIN: f32 = 12.0;
/// Below this an effect's envelope is not drawn at all.
const VISIBLE: f32 = 0.01;
/// Marks closer than this (pt) along their ring are not drawn, except the lit one and beat 1.
const MIN_MARK_SPACING: f32 = 2.5;
/// Half the length of an idle mark, pt.
const MARK_HALF: f32 = 6.0;
/// Segments of a ring's outline.
const RING_SEGMENTS: usize = 96;

/// Angle of the hand at `measure_phase`: one turn per measure, from 12
/// o'clock (`-π/2`) clockwise.
pub fn sweep_angle(measure_phase: f64) -> f32 {
    -FRAC_PI_2 + measure_phase.rem_euclid(1.0) as f32 * TAU
}

/// Where the tip of a hand of length `len` around `center` is at `measure_phase`.
pub fn sweep_hand(center: Pos2, len: f32, measure_phase: f64) -> Pos2 {
    center + len * Vec2::angled(sweep_angle(measure_phase))
}

/// Paints the sweep of `frame` centered in `area`, as big as its smaller side allows.
pub(super) fn paint(painter: &Painter, frame: &TopologyFrame, area: Rect) {
    let style = SweepStyle::DEFAULT;
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

    // The hand reaches a little past the outermost ring.
    let outer = rings.first().map_or(half, |r| r.radius);
    let hand_len = outer + MARK_HALF + 3.0;
    let measure_secs = frame.measure_secs.max(1e-3);
    let phase = frame.measure_phase.filter(|_| frame.playing);

    // The trail: where the hand was a few frames ago, from the phase, at most
    // one turn so it never wraps around itself.
    if let Some(phase) = phase {
        let one_turn = measure_secs / f64::from(style.trail_step);
        let steps = (f64::from(style.trail as f32 * budget).round()).min(one_turn.floor()) as usize;
        for j in (1..=steps).rev() {
            let past = phase - f64::from(style.trail_step) * j as f64 / measure_secs;
            let fade = 1.0 - j as f32 / (steps as f32 + 1.0);
            painter.add(Shape::line_segment(
                [center, sweep_hand(center, hand_len, past)],
                Stroke::new(3.0, theme::with_alpha(t.strong, 0.22 * fade * fade)),
            ));
        }
    }

    let mut lit_marks = Vec::new();
    for (pulse, ring) in pulses.iter().zip(&rings) {
        let opacity = if pulse.enabled { 1.0 } else { 0.2 };
        painter.add(Shape::closed_line(
            (0..RING_SEGMENTS)
                .map(|k| center + ring.radius * Vec2::angled(k as f32 / RING_SEGMENTS as f32 * TAU))
                .collect(),
            Stroke::new(1.0, theme::with_alpha(t.dot_inactive, 0.6 * opacity)),
        ));

        let beats = pulse.beats.max(1);
        let spacing = TAU * ring.radius / beats as f32;
        let gap = super::ring_gap(pulses.len(), size);
        let half_len = MARK_HALF.min(gap * 0.4).max(2.0);
        let lit = pulse
            .hit
            .map(|h| (h.beat, fx::envelope(h.age, style.hit_tau), h.age))
            .filter(|&(_, e, _)| e > VISIBLE);
        let lit_beat = lit.map(|(b, _, _)| b);
        let mark = |b: u32, half_len: f32| {
            let dir = Vec2::angled(sweep_angle(f64::from(b) / f64::from(beats)));
            [
                center + (ring.radius - half_len) * dir,
                center + (ring.radius + half_len) * dir,
            ]
        };
        let idle = Stroke::new(
            (spacing * 0.5).clamp(1.0, 2.0),
            theme::with_alpha(pulse.color, 0.45 * opacity),
        );
        if spacing >= MIN_MARK_SPACING {
            for b in (0..beats).filter(|&b| Some(b) != lit_beat) {
                painter.add(Shape::line_segment(mark(b, half_len), idle));
            }
        } else if lit_beat != Some(0) {
            painter.add(Shape::line_segment(mark(0, half_len), idle));
        }
        if let Some((b, energy, age)) = lit {
            lit_marks.push((
                mark(b, half_len * (1.0 + 0.5 * energy)),
                pulse.color,
                energy,
                age,
            ));
        }
    }

    // Lit marks over everything else of their ring, with their glow.
    for (ends, color, energy, age) in lit_marks {
        let color = fx::vibrate(color, energy, age);
        theme::paint_glow(
            painter,
            ends[0].lerp(ends[1], 0.5),
            9.0 + 12.0 * energy,
            color,
            0.6 * energy,
        );
        painter.add(Shape::line_segment(
            ends,
            Stroke::new(
                3.0 + 3.0 * energy,
                theme::with_alpha(color, 0.5 + 0.5 * energy),
            ),
        ));
    }

    // The hand: up at beat 1, and while stopped.
    let tip = sweep_hand(center, hand_len, phase.unwrap_or(0.0));
    if phase.is_some() {
        theme::paint_glow(painter, tip, 8.0, t.strong, 0.35 * fx::FX_STRENGTH);
    }
    painter.add(Shape::line_segment(
        [center, tip],
        Stroke::new(2.0, theme::with_alpha(t.strong, 0.9)),
    ));

    // The centre dot, glowing on a coincidence.
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
