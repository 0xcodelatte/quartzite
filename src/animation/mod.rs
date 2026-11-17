//! Spring physics for every animation in the compositor.
//!
//! Like Core Animation's `CASpringAnimation`, a spring is described by its
//! *response* (roughly the period, in seconds) and *damping ratio*
//! (1.0 = no overshoot, < 1.0 = a little bounce). Springs keep their
//! velocity when retargeted mid-flight, which is what makes interrupted
//! animations (e.g. toggling Exposé twice quickly) feel natural.
//!
//! Animations are sampled once per rendered frame, and frames are paced by
//! vblank, so motion is vsync-locked without any timers.

use std::time::Instant;

use smithay::utils::{Logical, Rectangle};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    stiffness: f64,
    damping: f64,
}

impl Spring {
    pub fn new(response: f64, damping_ratio: f64) -> Self {
        let omega = std::f64::consts::TAU / response;
        Self {
            stiffness: omega * omega,
            damping: 2.0 * damping_ratio * omega,
        }
    }

    /// Window open/close: quick with a hint of bounce.
    pub fn snappy() -> Self {
        Self::new(0.32, 0.82)
    }
    /// Layout changes (Exposé, Spaces, zoom): smooth, no overshoot.
    pub fn smooth() -> Self {
        Self::new(0.42, 1.0)
    }
}

#[derive(Debug, Clone)]
pub struct Animated {
    value: f64,
    velocity: f64,
    target: f64,
    spring: Spring,
    last: Option<Instant>,
}

impl Animated {
    pub fn new(value: f64) -> Self {
        Self { value, velocity: 0.0, target: value, spring: Spring::smooth(), last: None }
    }

    /// Start moving toward `target`, keeping the current velocity.
    pub fn to(&mut self, target: f64, spring: Spring) {
        if target != self.target || spring != self.spring {
            self.target = target;
            self.spring = spring;
        }
    }

    /// Jump to `value` with no animation.
    pub fn set(&mut self, value: f64) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    pub fn target(&self) -> f64 {
        self.target
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn is_settled(&self) -> bool {
        self.value == self.target && self.velocity == 0.0
    }

    /// Advance the simulation to `now` and return the new value.
    pub fn tick(&mut self, now: Instant) -> f64 {
        let last = self.last.replace(now).unwrap_or(now);
        if self.is_settled() {
            return self.value;
        }
        // Cap the step so a long stall (e.g. VT switch) doesn't explode.
        let mut dt = now.saturating_duration_since(last).as_secs_f64().min(0.1);
        // Semi-implicit Euler with 1 ms substeps is stable for stiff springs.
        const STEP: f64 = 0.001;
        while dt > 0.0 {
            let h = dt.min(STEP);
            let force = -self.spring.stiffness * (self.value - self.target) - self.spring.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
            dt -= h;
        }
        let scale = (self.target.abs()).max(1.0);
        if (self.value - self.target).abs() < 1e-3 * scale && self.velocity.abs() < 1e-2 * scale {
            self.set(self.target);
        }
        self.value
    }
}

/// A rectangle whose edges spring toward a (possibly moving) target.
#[derive(Debug, Clone)]
pub struct AnimatedRect {
    x: Animated,
    y: Animated,
    w: Animated,
    h: Animated,
}

impl AnimatedRect {
    pub fn new(from: Rectangle<f64, Logical>) -> Self {
        Self {
            x: Animated::new(from.loc.x),
            y: Animated::new(from.loc.y),
            w: Animated::new(from.size.w),
            h: Animated::new(from.size.h),
        }
    }

    pub fn tick(&mut self, target: Rectangle<f64, Logical>, now: Instant) -> Rectangle<f64, Logical> {
        let s = Spring::smooth();
        self.x.to(target.loc.x, s);
        self.y.to(target.loc.y, s);
        self.w.to(target.size.w, s);
        self.h.to(target.size.h, s);
        Rectangle::new(
            (self.x.tick(now), self.y.tick(now)).into(),
            (self.w.tick(now), self.h.tick(now)).into(),
        )
    }

    pub fn is_settled(&self) -> bool {
        self.x.is_settled() && self.y.is_settled() && self.w.is_settled() && self.h.is_settled()
    }
}

pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub fn lerp_rect(a: Rectangle<f64, Logical>, b: Rectangle<f64, Logical>, t: f64) -> Rectangle<f64, Logical> {
    Rectangle::new(
        (lerp(a.loc.x, b.loc.x, t), lerp(a.loc.y, b.loc.y, t)).into(),
        (lerp(a.size.w, b.size.w, t), lerp(a.size.h, b.size.h, t)).into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn run(anim: &mut Animated, secs: f64) -> Vec<f64> {
        let start = Instant::now();
        anim.tick(start);
        (1..=(secs * 60.0) as u32)
            .map(|i| anim.tick(start + Duration::from_secs_f64(i as f64 / 60.0)))
            .collect()
    }

    #[test]
    fn critically_damped_spring_settles_without_overshoot() {
        let mut a = Animated::new(0.0);
        a.to(1.0, Spring::smooth());
        let samples = run(&mut a, 2.0);
        assert!(samples.iter().all(|v| *v <= 1.0 + 1e-9));
        assert!(a.is_settled());
        assert_eq!(a.value(), 1.0);
    }

    #[test]
    fn underdamped_spring_overshoots_then_settles() {
        let mut a = Animated::new(0.0);
        a.to(1.0, Spring::snappy());
        let samples = run(&mut a, 2.0);
        assert!(samples.iter().any(|v| *v > 1.0));
        assert!(a.is_settled());
    }

    #[test]
    fn retargeting_keeps_velocity() {
        let mut a = Animated::new(0.0);
        a.to(1.0, Spring::smooth());
        let start = Instant::now();
        a.tick(start);
        a.tick(start + Duration::from_millis(100));
        a.to(0.0, Spring::smooth());
        let before = a.value();
        a.tick(start + Duration::from_millis(116));
        // Still moving forward for a moment thanks to momentum.
        assert!(a.value() > before);
    }
}
