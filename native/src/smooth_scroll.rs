use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const DURATION: Duration = Duration::from_millis(170);

struct Impulse {
    start: Instant,
    distance: f64,
    applied: f64,
}

#[derive(Default)]
pub struct SmoothWheel {
    position: f64,
    impulses: VecDeque<Impulse>,
}

impl SmoothWheel {
    pub fn reset(&mut self, position: f64) {
        self.position = position;
        self.impulses.clear();
    }

    pub fn active(&self) -> bool {
        !self.impulses.is_empty()
    }

    pub fn push(&mut self, distance: f64, now: Instant, max: f64) {
        self.sample(now);
        if distance == 0. || !distance.is_finite() {
            return;
        }
        if self
            .impulses
            .back()
            .is_some_and(|p| p.distance.signum() != distance.signum())
        {
            self.impulses.clear();
        }
        let pending: f64 = self
            .impulses
            .iter()
            .map(|p| p.distance * (1. - p.applied))
            .sum();
        let target = self.position + pending;
        let distance = if distance > 0. {
            distance.min((max.max(0.) - target).max(0.))
        } else {
            distance.max((-target).min(0.))
        };
        if distance != 0. {
            self.impulses.push_back(Impulse {
                start: now,
                distance,
                applied: 0.,
            });
        }
    }

    pub fn sample(&mut self, now: Instant) -> f64 {
        for pulse in &mut self.impulses {
            let t = (now.saturating_duration_since(pulse.start).as_secs_f64()
                / DURATION.as_secs_f64())
            .min(1.);
            let fraction = 1. - (1. - t).powi(3) * (1. + 3. * t);
            self.position += pulse.distance * (fraction - pulse.applied);
            pulse.applied = fraction;
        }
        self.impulses.retain(|p| p.applied < 1.);
        self.position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-7, "{actual} != {expected}");
    }

    #[test]
    fn burst_preserves_every_fractional_tick_and_finishes_promptly() {
        let start = Instant::now();
        let mut wheel = SmoothWheel::default();
        for tick in 0..100 {
            wheel.push(7.5, start + Duration::from_millis(tick * 2), 10_000.);
        }
        assert!(wheel.sample(start + Duration::from_millis(198)) > 450.);
        close(
            wheel.sample(start + Duration::from_millis(198) + DURATION),
            750.,
        );
        assert!(!wheel.active());
    }

    #[test]
    fn another_tick_does_not_restart_existing_motion() {
        let start = Instant::now();
        let mut wheel = SmoothWheel::default();
        wheel.push(100., start, 10_000.);
        wheel.push(100., start + Duration::from_millis(100), 10_000.);
        close(
            wheel.sample(start + DURATION),
            100. + 100. * (1. - 38_000. / 83_521.),
        );
        close(
            wheel.sample(start + Duration::from_millis(100) + DURATION),
            200.,
        );
    }

    #[test]
    fn trajectory_is_independent_of_frame_rate() {
        let start = Instant::now();
        let mut samples = vec![];
        for hz in [30, 60, 165] {
            let mut wheel = SmoothWheel::default();
            wheel.push(250., start, 10_000.);
            for frame in 1..hz / 10 {
                wheel.sample(start + Duration::from_secs_f64(frame as f64 / hz as f64));
            }
            samples.push(wheel.sample(start + Duration::from_millis(100)));
            close(wheel.sample(start + Duration::from_secs(1)), 250.);
            assert!(!wheel.active());
        }
        for value in samples {
            close(value, 250. * (1. - 16_121. / 83_521.));
        }
    }

    #[test]
    fn consecutive_ticks_have_no_deep_speed_troughs() {
        let start = Instant::now();
        let mut wheel = SmoothWheel::default();
        let (mut previous, mut slowest, mut fastest) = (0., f64::INFINITY, 0_f64);
        for frame in 0..180 {
            let now = start + Duration::from_millis(frame * 6);
            if frame % 10 == 0 {
                wheel.push(120., now, 100_000.);
            }
            let position = wheel.sample(now);
            if frame > 60 {
                slowest = slowest.min(position - previous);
                fastest = fastest.max(position - previous);
            }
            previous = position;
        }
        assert!(
            fastest / slowest < 1.3,
            "speed ratio: {}",
            fastest / slowest
        );
    }

    #[test]
    fn ticks_start_and_finish_without_a_velocity_jump() {
        let start = Instant::now();
        let mut wheel = SmoothWheel::default();
        wheel.push(96., start, 10_000.);
        let step = Duration::from_micros(10);
        assert!(wheel.sample(start + step) / step.as_secs_f64() < 2.);
        let before_end = wheel.sample(start + DURATION - step);
        let end = wheel.sample(start + DURATION);
        assert!((end - before_end) / step.as_secs_f64() < 2.);
        close(end, 96.);
    }

    #[test]
    fn reversal_moves_back_on_the_next_frame() {
        let start = Instant::now();
        let mut wheel = SmoothWheel::default();
        wheel.reset(500.);
        wheel.push(300., start, 10_000.);
        let turn = start + Duration::from_millis(30);
        let position = wheel.sample(turn);
        wheel.push(-60., turn, 10_000.);
        assert!(wheel.sample(turn + Duration::from_millis(6)) < position);
        close(wheel.sample(turn + DURATION), position - 60.);
    }

    #[test]
    fn bounds_have_no_overshoot_or_queued_debt() {
        let start = Instant::now();
        let mut wheel = SmoothWheel::default();
        wheel.push(50_000., start, 1_000.);
        close(wheel.sample(start + DURATION), 1_000.);
        wheel.push(500., start + DURATION, 1_000.);
        assert!(!wheel.active());
        wheel.push(-60., start + DURATION, 1_000.);
        assert!(wheel.sample(start + DURATION + Duration::from_millis(6)) < 1_000.);
        wheel.push(
            -50_000.,
            start + DURATION + Duration::from_millis(7),
            1_000.,
        );
        close(wheel.sample(start + Duration::from_secs(1)), 0.);
    }

    #[test]
    fn repeated_ticks_at_the_limit_do_not_discard_pending_distance() {
        let start = Instant::now();
        for spacing in [137, 591, 1_000, 1_721] {
            let mut wheel = SmoothWheel::default();
            let max = 13_389.6015625;
            for tick in 0..260 {
                wheel.push(60., start + Duration::from_micros(tick * spacing), max);
            }
            close(wheel.sample(start + Duration::from_secs(1)), max);
        }
    }

    #[test]
    fn reset_cancels_pending_motion() {
        let start = Instant::now();
        let mut wheel = SmoothWheel::default();
        wheel.push(500., start, 1_000.);
        wheel.reset(40.);
        close(wheel.sample(start + Duration::from_secs(1)), 40.);
        assert!(!wheel.active());
    }
}
