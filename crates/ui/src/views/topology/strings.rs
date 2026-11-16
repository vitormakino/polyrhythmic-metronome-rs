//! Strings: one horizontal line per pulse, top to bottom, with a mark at
//! every beat. A cursor crosses all of them once per measure; when a pulse
//! hits, its string is plucked at the beat's mark and rings down, the color
//! vibrating with it. A coincidence adds a central glow and a light,
//! deterministic shake.

use egui::{Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2};

use super::{TopologyFrame, fx};
use crate::theme;

/// The strings' tunables. Fixed, not shown in the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StringsStyle {
    /// Peak displacement of a plucked string, pt (the sketch's 9, scaled by the effects intensity).
    pub pluck: f32,
    /// Time constant of the ringing, s (0.1..=0.6).
    pub pluck_tau: f32,
    /// Angular speed of the ringing, rad/s.
    pub pluck_freq: f32,
    /// Amplitude of the ripple riding on the string, pt (0..=3).
    pub ripple: f32,
    /// Ripple wave number along the string, rad per string length.
    pub ripple_k: f32,
    /// Ripple angular speed, rad/s.
    pub ripple_freq: f32,
    /// Time constant of the color vibration, s (0.05..=0.4).
    pub color_tau: f32,
    /// Segments per string at full [`fx::budget`].
    pub segments: usize,
    /// Peak shake of the whole drawing on a coincidence, pt (0..=5).
    pub shake: f32,
    /// Time constant of the shake and the coincidence glow, s (0.1..=0.4).
    pub shake_tau: f32,
}

impl StringsStyle {
    /// The values the strings are drawn with.
    pub const DEFAULT: StringsStyle = StringsStyle {
        pluck: 9.0 * fx::FX_STRENGTH,
        pluck_tau: 0.35,
        pluck_freq: 55.0,
        ripple: 1.2,
        ripple_k: 40.0,
        ripple_freq: 90.0,
        color_tau: 0.15,
        segments: 120,
        shake: 3.0,
        shake_tau: 0.22,
    };
}

/// Room kept free left and right of the strings, and above and below.
const MARGIN: f32 = 14.0;
const VERTICAL_MARGIN: f32 = 6.0;
/// The most a string may swing, as a share of the spacing between strings.
const MAX_SWING: f32 = 0.4;
/// Fewest segments of a string, however heavy the load.
const MIN_SEGMENTS: usize = 24;
/// Below this an effect's envelope is not drawn at all.
const VISIBLE: f32 = 0.01;
/// Marks closer than this (pt) are not drawn, except the lit one and beat 1.
const MIN_MARK_SPACING: f32 = 2.5;
/// Radius of an idle mark.
const MARK: f32 = 2.5;

/// Height of the strings themselves: 28 pt plus 22 per pulse, at most 200.
fn drawing_height(pulses: usize) -> f32 {
    (28.0 + 22.0 * pulses as f32).min(200.0)
}

/// Height of a card whose strings get [`drawing_height`]: the drawing plus
/// the card's own chrome (title strip and padding, the indicator strip, the
/// strings' vertical margins). It is solved for the indicator strip, which
/// is a tenth of the area above it (see [`super::strip_height`]).
pub(super) fn card_height(pulses: usize) -> f32 {
    let drawing = drawing_height(pulses) + 2.0 * VERTICAL_MARGIN;
    let strip = super::strip_height(drawing * 10.0 / 9.0);
    let strip = super::strip_height(drawing + strip);
    drawing + strip + super::CARD_TITLE_STRIP + 2.0 * super::CARD_INNER_PAD
}

/// How much of the swing a string keeps when strings are `row` pt apart: 1,
/// or less so that pluck and ripple together stay within [`MAX_SWING`] of the spacing.
fn swing_scale(row: f32) -> f32 {
    let style = StringsStyle::DEFAULT;
    (MAX_SWING * row / (style.pluck + style.ripple)).clamp(0.0, 1.0)
}

/// Where the cursor is, `phase` (in `[0, 1]`) of the way across `track`.
pub fn cursor_x(track: Rect, phase: f64) -> f32 {
    track.left() + track.width() * phase as f32
}

/// The triangle that peaks at `pluck` (0..1) and is 0 at both ends.
fn triangle(u: f32, pluck: f32) -> f32 {
    let pluck = pluck.clamp(0.01, 0.99);
    if u < pluck {
        u / pluck
    } else {
        (1.0 - u) / (1.0 - pluck)
    }
    .clamp(0.0, 1.0)
}

/// Displacement (pt) of a string at `u` (0..1 along it) `age` seconds after it
/// was plucked at `pluck` (0..1): the triangle times the amplitude, ringing
/// down. Zero at both ends and, without a pluck, everywhere.
pub fn displacement(u: f32, pluck: f32, age: f32) -> f32 {
    let style = StringsStyle::DEFAULT;
    if !age.is_finite() {
        return 0.0;
    }
    triangle(u, pluck)
        * style.pluck
        * fx::envelope(age, style.pluck_tau)
        * (age * style.pluck_freq).sin()
}

/// The small ripple that rides on top of [`displacement`] (pt).
fn ripple(style: &StringsStyle, u: f32, age: f32) -> f32 {
    style.ripple
        * fx::envelope(age, style.pluck_tau)
        * (u * style.ripple_k + age * style.ripple_freq).sin()
}

/// Paints the strings of `frame` in `area`.
pub(super) fn paint(painter: &Painter, frame: &TopologyFrame, area: Rect) {
    let style = StringsStyle::DEFAULT;
    let t = frame.theme;
    let pulses = &frame.pulses;
    if pulses.is_empty() || area.height() < 1.0 {
        return;
    }
    let max_beats = pulses.iter().map(|p| p.beats).max().unwrap_or(1);
    let budget = fx::budget(pulses.len(), max_beats);
    let segments = ((style.segments as f32 * budget) as usize).max(MIN_SEGMENTS);

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
    if coincidence > VISIBLE {
        theme::paint_glow(
            painter,
            area.center(),
            area.width().min(area.height() * 3.0) * 0.45,
            t.strong,
            0.5 * coincidence * fx::FX_STRENGTH,
        );
    }

    let track = Rect::from_min_max(
        pos2(area.left() + MARGIN, area.top() + VERTICAL_MARGIN),
        pos2(area.right() - MARGIN, area.bottom() - VERTICAL_MARGIN),
    );
    let row = track.height() / pulses.len() as f32;
    let swing = swing_scale(row);
    for (index, pulse) in pulses.iter().enumerate() {
        let y = track.top() + row * (index as f32 + 0.5);
        let opacity = if pulse.enabled { 1.0 } else { 0.2 };
        let beats = pulse.beats;
        let x_of = |u: f32| track.left() + track.width() * u;
        let age = pulse.hit.map_or(f32::INFINITY, |h| h.age);
        let ring = fx::envelope(age, style.pluck_tau);
        let decay = if pulse.hit.is_some() { ring } else { 0.0 };
        let color = match pulse.hit {
            Some(h) => fx::vibrate(pulse.color, fx::envelope(h.age, style.color_tau), h.age),
            None => pulse.color,
        };

        // The string: at rest a straight line, plucked at the hit's mark.
        let pluck = pulse.hit.map(|h| h.beat as f32 / beats as f32);
        let points: Vec<Pos2> = (0..=segments)
            .map(|k| {
                let u = k as f32 / segments as f32;
                let wobble = match pluck {
                    Some(p) if decay > VISIBLE => {
                        (displacement(u, p, age) + ripple(&style, u, age)) * swing
                    }
                    _ => 0.0,
                };
                pos2(x_of(u), y + wobble)
            })
            .collect();
        if decay > VISIBLE {
            painter.add(Shape::line(
                points.clone(),
                Stroke::new(5.0 * decay, theme::with_alpha(color, 0.25 * decay)),
            ));
        }
        painter.add(Shape::line(
            points,
            Stroke::new(
                2.0 + 1.5 * decay,
                theme::with_alpha(color, (0.35 + 0.65 * decay) * opacity),
            ),
        ));

        // The marks, the lit one swelling with the ringing.
        let spacing = track.width() / beats as f32;
        let idle = MARK.min(spacing * 0.4).min(row * 0.3).max(0.5);
        let lit_beat = pulse.hit.map(|h| h.beat);
        if spacing >= MIN_MARK_SPACING {
            for b in 0..beats {
                if Some(b) != lit_beat {
                    painter.circle_filled(
                        pos2(x_of(b as f32 / beats as f32), y),
                        idle,
                        t.dot_inactive.gamma_multiply(opacity),
                    );
                }
            }
        } else {
            painter.circle_filled(
                pos2(track.left(), y),
                idle,
                t.dot_inactive.gamma_multiply(opacity),
            );
        }
        if let Some(b) = lit_beat.filter(|_| decay > VISIBLE) {
            let at = pos2(x_of(b as f32 / beats as f32), y);
            let r = (idle + (1.5 + 3.0 * decay)).min(row * 0.45).max(idle);
            theme::paint_glow(painter, at, r * 2.6, color, 0.7 * decay);
            painter.circle_filled(at, r, color);
        }
    }

    // The cursor crosses every string once per measure; stopped it waits at beat 1.
    let x = cursor_x(track, frame.measure_phase.unwrap_or(0.0));
    painter.line_segment(
        [pos2(x, track.top()), pos2(x, track.bottom())],
        Stroke::new(3.0, theme::with_alpha(t.strong, 0.15)),
    );
    painter.line_segment(
        [pos2(x, track.top()), pos2(x, track.bottom())],
        Stroke::new(1.5, theme::with_alpha(t.strong, 0.85)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_strings_get_94_pt() {
        assert_eq!(drawing_height(3), 28.0 + 66.0);
        assert_eq!(drawing_height(16), 200.0);
        // The card's area minus the strip and the strings' margins is the drawing.
        let card = card_height(3);
        let area = card - super::super::CARD_TITLE_STRIP - 2.0 * super::super::CARD_INNER_PAD;
        let strip = super::super::strip_height(area);
        assert!((area - strip - 2.0 * VERTICAL_MARGIN - 94.0).abs() < 0.01);
    }

    #[test]
    fn swing_stays_within_the_spacing() {
        let style = StringsStyle::DEFAULT;
        for pulses in 1..=16 {
            let row = drawing_height(pulses) / pulses as f32;
            let peak = (style.pluck + style.ripple) * swing_scale(row);
            assert!(peak <= MAX_SWING * row + 1e-4, "{pulses}: {peak} vs {row}");
        }
        assert!(
            (swing_scale(200.0) - 1.0).abs() < 1e-6,
            "roomy rows keep it all"
        );
    }

    #[test]
    fn triangle_peaks_at_the_pluck() {
        assert_eq!(triangle(0.0, 0.3), 0.0);
        assert_eq!(triangle(1.0, 0.3), 0.0);
        assert!((triangle(0.3, 0.3) - 1.0).abs() < 1e-6);
        // A pluck at an end (beat 1) stays finite.
        assert!(triangle(0.5, 0.0).is_finite());
    }
}
