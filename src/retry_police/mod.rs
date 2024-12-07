use tryhard::backoff_strategies::BackoffStrategy;
use std::time::Duration;
use rand::{Rng};

#[derive(Debug, Clone, Copy)]
pub struct FixedBackoffWithJitter {
    delay: Duration,
    jitter_percentage: u8
}

impl FixedBackoffWithJitter {
    /// Create a new `LinearBackoff` with an initial delay.
    pub fn new(initial_delay: Duration, jitter_percentage: u8) -> Self {
        Self {
            delay: initial_delay,
            jitter_percentage
        }
    }
}

impl<'a, E> BackoffStrategy<'a, E> for FixedBackoffWithJitter {
    type Output = Duration;

    fn delay(&mut self, attempt: u32, error: &'a E) -> Self::Output {
        let from = -(self.jitter_percentage as i8);

        let coef_delta = rand::thread_rng().gen_range(from..self.jitter_percentage as i8) as f32;
        let coef = 1.0 + coef_delta / 100.0;
        self.delay.mul_f32(coef)
    }
}