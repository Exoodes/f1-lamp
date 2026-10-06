//! The wait between retries: short after the first failure, doubling after
//! each further one up to a limit, short again after a success. Used for
//! WiFi, the calendar download and the live feed.

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    first: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub const fn new(first: Duration, max: Duration) -> Self {
        Backoff {
            first,
            max,
            next: first,
        }
    }

    /// After a failure: how long to wait before the next try. The wait after
    /// that is twice as long, but never more than `max`.
    pub fn next_wait(&mut self) -> Duration {
        let wait = self.next;
        self.next = (self.next * 2).min(self.max);
        wait
    }

    /// After a success: the next failure waits `first` again.
    pub fn reset(&mut self) {
        self.next = self.first;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn waits_double_up_to_the_limit() {
        let mut b = Backoff::new(secs(5), secs(60));
        let waits: Vec<Duration> = (0..6).map(|_| b.next_wait()).collect();
        assert_eq!(
            waits,
            [secs(5), secs(10), secs(20), secs(40), secs(60), secs(60)]
        );
    }

    #[test]
    fn reset_starts_again_from_the_first_wait() {
        let mut b = Backoff::new(secs(1), secs(30));
        b.next_wait();
        b.next_wait();
        b.reset();
        assert_eq!(b.next_wait(), secs(1));
    }

    #[test]
    fn a_first_wait_above_the_limit_is_used_once_then_capped() {
        let mut b = Backoff::new(secs(10), secs(5));
        assert_eq!(b.next_wait(), secs(10));
        assert_eq!(b.next_wait(), secs(5));
    }
}
