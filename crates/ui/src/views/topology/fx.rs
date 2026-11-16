//! The effects kit shared by the topology drawings. Every effect is a pure
//! function of a hit's age (seconds since it touched its mark, in the shown
//! time) and of a seed derived from the hit: the same instant always gives
//! the same image, and nothing accumulates from frame to frame.

use std::f32::consts::TAU;

use egui::{Color32, Painter, Pos2, Stroke, Vec2};

use crate::theme;

/// The fixed "effects intensity" (the sketches' slider, at its 70 % default).
pub const FX_STRENGTH: f32 = 0.7;

/// Seconds a spark lives.
const SPARK_SECS: f32 = 0.6;
/// Sparks slow down like the sketch's `v *= 0.96` per 60 Hz frame: the speed
/// is `v0 * exp(-SPARK_DRAG * age)`.
const SPARK_DRAG: f32 = 2.449;
/// Seconds a ripple takes to expand and vanish.
const RIPPLE_SECS: f32 = 0.7;
/// Load up to which every effect is shown in full.
const FULL_PULSES: f32 = 4.0;
const FULL_BEATS: f32 = 16.0;
/// The least of the effects [`budget`] keeps.
const MIN_BUDGET: f32 = 0.02;

/// Decay of an effect: `exp(-age / tau)`, 1 at the hit (and before it).
pub fn envelope(age: f32, tau: f32) -> f32 {
    (-age.max(0.0) / tau).exp()
}

/// `color` brightened towards white by `energy` (0..=1, scaled by
/// [`FX_STRENGTH`]), with a fast wobble at `t` seconds; blue moves least, so
/// the color warms as it lights. Alpha is kept.
pub fn vibrate(color: Color32, energy: f32, t: f32) -> Color32 {
    let wobble = 0.5 + 0.5 * (t * 60.0).sin();
    let k = energy.clamp(0.0, 1.0) * FX_STRENGTH * (0.55 + 0.45 * wobble);
    let lift = |c: u8, share: f32| {
        let c = f32::from(c);
        (c + (255.0 - c) * k * share).round().clamp(0.0, 255.0) as u8
    };
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(lift(r, 1.0), lift(g, 0.8), lift(b, 0.6), a)
}

/// A deterministic jitter of up to `amount` points per axis at `t` seconds:
/// two fast sines with phases taken from `seed`.
pub fn shake(seed: u64, amount: f32, t: f32) -> Vec2 {
    let x = (t * 67.0 + TAU * unit(seed, 0)).sin();
    let y = (t * 53.0 + TAU * unit(seed, 1)).sin();
    amount * Vec2::new(x, y)
}

/// `count` sparks flying out of `origin` `age` seconds after the hit, as
/// (position, alpha). Each spark's direction and speed (`speed` times
/// 0.4..1.4, in points per second) come from `seed`; they slow down and fade
/// out linearly, alpha 0 from 0.6 s on.
pub fn sparks(
    seed: u64,
    origin: Pos2,
    age: f32,
    count: usize,
    speed: f32,
) -> impl Iterator<Item = (Pos2, f32)> {
    let age = age.max(0.0);
    let alpha = (1.0 - age / SPARK_SECS).clamp(0.0, 1.0);
    let travel = (1.0 - (-SPARK_DRAG * age).exp()) / SPARK_DRAG;
    (0..count as u64).map(move |i| {
        let angle = TAU * unit(seed, 2 * i + 2);
        let v = speed * (0.4 + unit(seed, 2 * i + 3));
        (origin + v * travel * Vec2::angled(angle), alpha)
    })
}

/// A ring expanding from `center` to `max_r` and fading out over 0.7 s,
/// `age` seconds after the hit. Nothing outside that window.
pub fn ripple(painter: &Painter, center: Pos2, age: f32, color: Color32, max_r: f32) {
    if !(0.0..RIPPLE_SECS).contains(&age) {
        return;
    }
    let u = age / RIPPLE_SECS;
    let alpha = (1.0 - u) * 0.8 * FX_STRENGTH;
    painter.circle_stroke(
        center,
        max_r * (0.15 + 0.85 * u),
        Stroke::new(1.5, theme::with_alpha(color, alpha)),
    );
}

/// How much of the effects the load allows, in `(0, 1]`: 1 up to 4 pulses
/// and 16 beats, then shrinking with the pulse count and the square root of
/// the beat count. Particle counts and wobble segments are scaled by it.
pub fn budget(pulses: usize, max_beats: u32) -> f32 {
    let by_pulses = (FULL_PULSES / pulses.max(1) as f32).min(1.0);
    let by_beats = (FULL_BEATS / max_beats.max(1) as f32).sqrt().min(1.0);
    (by_pulses * by_beats).clamp(MIN_BUDGET, 1.0)
}

/// A pseudo-random number in `[0, 1)` from `seed` and `k` (SplitMix64).
fn unit(seed: u64, k: u64) -> f32 {
    let mut z = seed.wrapping_add(k.wrapping_add(1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn envelope_is_one_at_zero_and_decays() {
        assert_eq!(envelope(0.0, 0.18), 1.0);
        assert!((envelope(0.18, 0.18) - (-1.0f32).exp()).abs() < 1e-6);
        assert!(envelope(0.1, 0.2) > envelope(0.2, 0.2));
        assert!(envelope(5.0, 0.2) < 1e-6);
        assert_eq!(envelope(-1.0, 0.2), 1.0, "not yet shown counts as the hit");
    }

    #[test]
    fn shake_is_deterministic() {
        assert_eq!(shake(42, 3.0, 0.05), shake(42, 3.0, 0.05));
        assert_ne!(shake(42, 3.0, 0.05), shake(43, 3.0, 0.05));
        assert_ne!(shake(42, 3.0, 0.05), Vec2::ZERO);
        assert_eq!(shake(42, 0.0, 0.05), Vec2::ZERO);
        for i in 0..200 {
            let v = shake(i, 3.0, i as f32 * 0.013);
            assert!(v.x.abs() <= 3.0 && v.y.abs() <= 3.0, "{v:?}");
        }
    }

    #[test]
    fn sparks_are_deterministic_and_fade() {
        let origin = pos2(100.0, 100.0);
        let a: Vec<_> = sparks(7, origin, 0.2, 6, 90.0).collect();
        let b: Vec<_> = sparks(7, origin, 0.2, 6, 90.0).collect();
        assert_eq!(a, b);
        assert_eq!(a.len(), 6);
        assert!(a.iter().all(|(p, alpha)| *p != origin && *alpha > 0.0));
        let other: Vec<_> = sparks(8, origin, 0.2, 6, 90.0).collect();
        assert_ne!(a, other);
        // They fly out and fade.
        let later: Vec<_> = sparks(7, origin, 0.4, 6, 90.0).collect();
        for ((p, alpha), (q, beta)) in a.iter().zip(&later) {
            assert!(q.distance(origin) > p.distance(origin));
            assert!(beta < alpha);
        }
        for age in [0.6, 0.7, 3.0] {
            assert!(sparks(7, origin, age, 6, 90.0).all(|(_, alpha)| alpha == 0.0));
        }
    }

    #[test]
    fn vibrate_brightens_and_wobbles() {
        let c = Color32::from_rgb(59, 130, 246);
        assert_eq!(vibrate(c, 0.0, 0.3), c);
        let lit = vibrate(c, 1.0, 0.0);
        assert!(lit.r() > c.r() && lit.g() > c.g() && lit.b() >= c.b());
        assert_ne!(vibrate(c, 1.0, 0.0), vibrate(c, 1.0, 0.03));
        assert_eq!(lit.a(), c.a());
    }

    #[test]
    fn budget_is_one_for_small_and_shrinks() {
        for pulses in 0..=4 {
            for beats in 1..=16 {
                assert_eq!(budget(pulses, beats), 1.0);
            }
        }
        assert!(budget(8, 16) < 1.0);
        assert!(budget(4, 64) < 1.0);
        assert!(budget(16, 1024) < budget(8, 64));
        let least = budget(16, 1024);
        assert!(least > 0.0 && least <= 1.0);
    }
}
