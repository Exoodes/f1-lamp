//! The time-driven rules of an open connection to the live feed, without
//! the network: when to ping, when the connection counts as dead, and when
//! the device's TLS lock must be let go even though `Subscribe` hasn't been
//! answered. The device asks [`LiveTimers::tick`] what's due, tells it what
//! happened ([`data`](LiveTimers::data), [`subscribed`](LiveTimers::subscribed)),
//! and sleeps until [`wake_at`](LiveTimers::wake_at).

use std::time::{Duration, Instant};

/// The server drops clients that stay quiet for about 30 s.
pub const PING_EVERY: Duration = Duration::from_secs(15);
/// Heartbeat and pings arrive every few seconds; this long without anything
/// means the connection is dead even if nobody said so.
pub const SILENCE_LIMIT: Duration = Duration::from_secs(60);
/// The longest the TLS lock is kept after the WebSocket opens, waiting for
/// the answer to `Subscribe`. It normally comes within a second; if it never
/// does (refused, or too large and skipped), holding on would block every
/// other HTTPS request for the whole session.
pub const TLS_HOLD_MAX: Duration = Duration::from_secs(15);

/// What [`LiveTimers::tick`] says is due now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Due {
    /// `Subscribe` wasn't answered in time: let go of the TLS lock now.
    pub release_tls: bool,
    pub ping: bool,
    /// Nothing arrived for `SILENCE_LIMIT`: the connection is dead.
    pub dead: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct LiveTimers {
    next_ping: Instant,
    last_data: Instant,
    /// While the TLS lock is held: when to let go of it at the latest.
    release_tls_at: Option<Instant>,
}

impl LiveTimers {
    /// A connection that opened at `now`, holding the TLS lock.
    pub fn new(now: Instant) -> Self {
        LiveTimers {
            next_ping: now + PING_EVERY,
            last_data: now,
            release_tls_at: Some(now + TLS_HOLD_MAX),
        }
    }

    /// What's due at `now`. Each thing is reported once: the next ping is
    /// counted from `now`, and the TLS lock counts as let go.
    pub fn tick(&mut self, now: Instant) -> Due {
        let release_tls = self.release_tls_at.is_some_and(|at| now >= at);
        if release_tls {
            self.release_tls_at = None;
        }
        let ping = now >= self.next_ping;
        if ping {
            self.next_ping = now + PING_EVERY;
        }
        Due {
            release_tls,
            ping,
            dead: now.saturating_duration_since(self.last_data) > SILENCE_LIMIT,
        }
    }

    /// Something arrived.
    pub fn data(&mut self, now: Instant) {
        self.last_data = now;
    }

    /// `Subscribe` was answered, so the TLS lock was let go: no deadline.
    pub fn subscribed(&mut self) {
        self.release_tls_at = None;
    }

    /// When `tick` next has something to do (the silence check can wait for
    /// the next ping: the limit is far longer than the ping interval).
    pub fn wake_at(&self) -> Instant {
        self.release_tls_at
            .map_or(self.next_ping, |at| at.min(self.next_ping))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn nothing_is_due_right_after_opening() {
        let t0 = Instant::now();
        assert_eq!(LiveTimers::new(t0).tick(t0), Due::default());
    }

    #[test]
    fn pings_go_out_every_interval() {
        let t0 = Instant::now();
        let mut t = LiveTimers::new(t0);
        t.subscribed();
        t.data(t0 + PING_EVERY);
        assert!(t.tick(t0 + PING_EVERY).ping);
        assert!(!t.tick(t0 + PING_EVERY + secs(1)).ping);
        assert!(t.tick(t0 + PING_EVERY * 2).ping);
    }

    #[test]
    fn data_keeps_the_connection_alive() {
        let t0 = Instant::now();
        let mut t = LiveTimers::new(t0);
        t.data(t0 + secs(50));
        assert!(!t.tick(t0 + secs(100)).dead);
        assert!(t.tick(t0 + secs(111)).dead);
    }

    #[test]
    fn silence_alone_is_dead_after_the_limit() {
        let t0 = Instant::now();
        let mut t = LiveTimers::new(t0);
        assert!(!t.tick(t0 + SILENCE_LIMIT).dead);
        assert!(t.tick(t0 + SILENCE_LIMIT + secs(1)).dead);
    }

    #[test]
    fn without_a_subscribe_answer_the_tls_lock_goes_at_the_deadline_once() {
        let t0 = Instant::now();
        let mut t = LiveTimers::new(t0);
        assert!(!t.tick(t0 + TLS_HOLD_MAX - secs(1)).release_tls);
        assert!(t.tick(t0 + TLS_HOLD_MAX).release_tls);
        assert!(!t.tick(t0 + TLS_HOLD_MAX + secs(1)).release_tls);
    }

    #[test]
    fn after_subscribing_there_is_no_tls_deadline() {
        let t0 = Instant::now();
        let mut t = LiveTimers::new(t0);
        t.subscribed();
        assert!(!t.tick(t0 + TLS_HOLD_MAX).release_tls);
    }

    #[test]
    fn it_wakes_for_the_earlier_of_the_tls_deadline_and_the_ping() {
        let t0 = Instant::now();
        let mut t = LiveTimers::new(t0);
        assert_eq!(t.wake_at(), t0 + TLS_HOLD_MAX.min(PING_EVERY));
        t.subscribed();
        assert_eq!(t.wake_at(), t0 + PING_EVERY);
    }

    #[test]
    fn the_silence_limit_is_well_above_the_ping_interval() {
        // `wake_at` relies on it: waking for pings is often enough.
        assert!(SILENCE_LIMIT >= PING_EVERY * 2);
    }
}
