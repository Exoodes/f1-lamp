//! Whether the live feed keeps failing, for the lamp to show it. One failed
//! connection is normal (the server drops clients now and then, and the next
//! try works), so only several in a row count; one that subscribes ends it.

/// Failed connection attempts in a row before the feed counts as failing.
/// With the live thread's waits of 5, 10 and 20 s, that's about half a minute.
pub const FAILURES_BEFORE_ALARM: u32 = 3;

#[derive(Debug, Default)]
pub struct FeedHealth {
    failures: u32,
    failing: bool,
}

impl FeedHealth {
    /// A connection attempt failed or a connection broke. `Some(true)` when
    /// the feed now counts as failing and the lamp should be told.
    pub fn failed(&mut self) -> Option<bool> {
        self.failures = self.failures.saturating_add(1);
        self.set(self.failures >= FAILURES_BEFORE_ALARM)
    }

    /// Subscribed: the feed works. `Some(false)` when it was failing before.
    pub fn working(&mut self) -> Option<bool> {
        self.failures = 0;
        self.set(false)
    }

    /// The feed isn't wanted any more: whatever happened, it isn't failing.
    pub fn not_wanted(&mut self) -> Option<bool> {
        self.working()
    }

    /// The new state, if it changed.
    fn set(&mut self, failing: bool) -> Option<bool> {
        (failing != self.failing).then(|| {
            self.failing = failing;
            failing
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_failures_are_not_reported() {
        let mut h = FeedHealth::default();
        for _ in 1..FAILURES_BEFORE_ALARM {
            assert_eq!(h.failed(), None);
        }
    }

    #[test]
    fn enough_failures_in_a_row_are_reported_once() {
        let mut h = FeedHealth::default();
        for _ in 1..FAILURES_BEFORE_ALARM {
            h.failed();
        }
        assert_eq!(h.failed(), Some(true));
        assert_eq!(h.failed(), None);
    }

    #[test]
    fn subscribing_ends_the_alarm() {
        let mut h = FeedHealth::default();
        for _ in 0..FAILURES_BEFORE_ALARM {
            h.failed();
        }
        assert_eq!(h.working(), Some(false));
        assert_eq!(h.working(), None);
    }

    #[test]
    fn a_working_connection_starts_the_count_again() {
        let mut h = FeedHealth::default();
        for _ in 1..FAILURES_BEFORE_ALARM {
            h.failed();
        }
        h.working();
        assert_eq!(h.failed(), None);
    }

    #[test]
    fn not_wanted_ends_the_alarm() {
        let mut h = FeedHealth::default();
        for _ in 0..FAILURES_BEFORE_ALARM {
            h.failed();
        }
        assert_eq!(h.not_wanted(), Some(false));
    }
}
