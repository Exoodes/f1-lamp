//! A thread's sign of life. The thread calls [`Heartbeat::beat`] every time
//! round its loop; the supervisor asks [`Heartbeat::silent_for`] and restarts
//! the chip when a thread has been quiet too long. That catches a thread that
//! hangs (a lock that never frees, a call that never returns), which the
//! supervisor can't see otherwise: such a thread hasn't ended.

use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

/// Whole seconds since `base`, in an `AtomicU32`: the ESP32-C3 has no 64-bit
/// atomics, and seconds last 136 years.
#[derive(Debug)]
pub struct Heartbeat {
    base: Instant,
    last_secs: AtomicU32,
}

impl Heartbeat {
    /// Counts as a beat at `now`, so a thread that is just starting isn't
    /// silent.
    pub fn new(now: Instant) -> Self {
        Heartbeat {
            base: now,
            last_secs: AtomicU32::new(0),
        }
    }

    pub fn beat(&self, now: Instant) {
        self.last_secs.store(self.secs(now), Ordering::Relaxed);
    }

    /// How long since the last beat, to the second.
    pub fn silent_for(&self, now: Instant) -> Duration {
        let last = self.last_secs.load(Ordering::Relaxed);
        Duration::from_secs(u64::from(self.secs(now).saturating_sub(last)))
    }

    fn secs(&self, now: Instant) -> u32 {
        u32::try_from(now.saturating_duration_since(self.base).as_secs()).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_new_heartbeat_is_not_silent() {
        let t0 = Instant::now();
        assert_eq!(Heartbeat::new(t0).silent_for(t0), Duration::ZERO);
    }

    #[test]
    fn silence_grows_until_the_next_beat() {
        let t0 = Instant::now();
        let h = Heartbeat::new(t0);
        assert_eq!(h.silent_for(t0 + secs(90)), secs(90));
        h.beat(t0 + secs(100));
        assert_eq!(h.silent_for(t0 + secs(130)), secs(30));
    }

    #[test]
    fn a_beat_from_the_past_does_not_make_silence_negative() {
        let t0 = Instant::now();
        let h = Heartbeat::new(t0);
        h.beat(t0 + secs(50));
        assert_eq!(h.silent_for(t0 + secs(20)), Duration::ZERO);
    }

    #[test]
    fn a_time_before_the_base_counts_as_the_base() {
        let t0 = Instant::now() + secs(10);
        let h = Heartbeat::new(t0);
        h.beat(t0 - secs(5));
        assert_eq!(h.silent_for(t0 + secs(3)), secs(3));
    }
}
