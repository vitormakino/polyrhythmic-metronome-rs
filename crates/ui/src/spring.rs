//! Critically-tuned spring used for UI animations (stiffness 500, damping 30, mass 1).

const STIFFNESS: f32 = 500.0;
const DAMPING: f32 = 30.0;
const MASS: f32 = 1.0;
/// Largest integration substep, in seconds.
const MAX_SUBSTEP: f32 = 1.0 / 240.0;
const EPSILON: f32 = 1e-3;

/// A damped spring that pulls `value` towards `target`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
}

impl Spring {
    /// A spring at rest at `value`.
    pub fn new(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
        }
    }

    /// Advance by `dt` seconds using semi-implicit Euler with substeps of at
    /// most 1/240 s, so the result barely depends on the frame rate.
    pub fn step(&mut self, dt: f32) {
        if dt <= 0.0 || !dt.is_finite() {
            return;
        }
        let substeps = (dt / MAX_SUBSTEP).ceil().max(1.0);
        let h = dt / substeps;
        for _ in 0..substeps as u32 {
            let force = STIFFNESS * (self.target - self.value) - DAMPING * self.velocity;
            self.velocity += force / MASS * h;
            self.value += self.velocity * h;
        }
    }

    /// True once both the distance to the target and the velocity are below 1e-3.
    pub fn settled(&self) -> bool {
        (self.value - self.target).abs() < EPSILON && self.velocity.abs() < EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rate: f32, seconds: f32) -> (Spring, f32) {
        let mut s = Spring::new(0.0);
        s.target = 1.0;
        let mut peak = 0.0f32;
        let n = (rate * seconds).round() as u32;
        for _ in 0..n {
            s.step(1.0 / rate);
            peak = peak.max(s.value);
        }
        (s, peak)
    }

    #[test]
    fn spring_settles_to_target() {
        // Within 0.3 s the spring is visually at the target (2 %).
        let (s, peak) = run(60.0, 0.3);
        assert!((s.value - 1.0).abs() < 0.02, "value at 0.3 s: {}", s.value);
        assert!(peak < 1.1, "overshoot too large: {peak}");
        // And it is formally settled shortly after.
        let (s, _) = run(60.0, 1.0);
        assert!(s.settled(), "{s:?}");
    }

    #[test]
    fn spring_is_framerate_independent() {
        let (a, _) = run(60.0, 0.5);
        let (b, _) = run(120.0, 0.5);
        assert!(a.value > 0.5, "spring did not move: {}", a.value);
        assert!(
            (a.value - b.value).abs() < 1e-3,
            "{} vs {}",
            a.value,
            b.value
        );
        let (c, _) = run(30.0, 0.5);
        assert!(
            (a.value - c.value).abs() < 1e-3,
            "{} vs {}",
            a.value,
            c.value
        );
    }

    #[test]
    fn rest_spring_is_settled_and_moves_when_retargeted() {
        let mut s = Spring::new(0.5);
        assert!(s.settled());
        s.target = 1.0;
        assert!(!s.settled());
    }
}
