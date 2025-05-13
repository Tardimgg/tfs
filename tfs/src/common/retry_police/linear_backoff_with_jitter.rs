use std::ops::Mul;
use std::time::Duration;
use rand::Rng;
use tryhard::backoff_strategies::BackoffStrategy;

#[derive(Debug, Clone, Copy)]
pub struct LinearBackoffWithJitter {
    delay: Duration,
    mult: u32,
    jitter_percentage: u8
}

impl LinearBackoffWithJitter {
    pub fn new(initial_delay: Duration, mult: u32, jitter_percentage: u8) -> Self {
        Self {
            delay: initial_delay,
            mult,
            jitter_percentage
        }
    }
}

impl<'a, E> BackoffStrategy<'a, E> for LinearBackoffWithJitter {
    type Output = Duration;

    fn delay(&mut self, attempt: u32, error: &'a E) -> Self::Output {
        let from = -(self.jitter_percentage as i8);

        let coef_delta = rand::thread_rng().gen_range(from..self.jitter_percentage as i8) as f32;
        let coef = 1.0 + coef_delta / 100.0;

        if self.mult == 0 {
            return if attempt == 1 {
                self.delay.mul_f32(coef)
            } else {
                Duration::from_secs(0)
            }
        }
        self.delay
            .saturating_add(self.delay.saturating_mul((attempt - 1) * (self.mult - 1)))
            .mul_f32(coef)
    }
}