//! The network thread's bookkeeping, without the network: when each of its
//! jobs is next due ([`Deadlines`]), and the session list with the decisions
//! around it ([`Calendar`]): which phase to send, and when a downloaded list
//! has to be saved.

use std::time::Instant;

use crate::{
    input::SessionPhase,
    schedule::{self, Session, CACHED_SESSIONS},
};

/// When each of the network thread's jobs is next due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadlines {
    pub wifi_check: Instant,
    pub heap_log: Instant,
    pub clock: Instant,
    pub calendar: Instant,
    pub phase: Instant,
    pub winner: Instant,
}

impl Deadlines {
    /// Every job due at once.
    pub fn all(now: Instant) -> Self {
        Deadlines {
            wifi_check: now,
            heap_log: now,
            clock: now,
            calendar: now,
            phase: now,
            winner: now,
        }
    }

    /// When the thread has to wake up next.
    pub fn earliest(&self) -> Instant {
        self.wifi_check
            .min(self.heap_log)
            .min(self.clock)
            .min(self.calendar)
            .min(self.phase)
            .min(self.winner)
    }
}

/// The sessions the lamp knows, what of them is saved in flash, and the
/// phase last sent.
#[derive(Debug, Default)]
pub struct Calendar {
    sessions: Vec<Session>,
    saved: Vec<Session>,
    sent_phase: Option<SessionPhase>,
}

impl Calendar {
    /// Starts from the sessions saved in flash: a reboot during a session
    /// (when OpenF1 refuses free requests) still knows what's on.
    pub fn new(saved: Vec<Session>) -> Self {
        Calendar {
            sessions: saved.clone(),
            saved,
            sent_phase: None,
        }
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    /// Swaps in a freshly downloaded list. Returns the part of it that still
    /// matters at `now_unix` when that differs from what flash holds: the
    /// caller saves it and reports back with [`Calendar::saved`].
    pub fn replace(&mut self, sessions: Vec<Session>, now_unix: i64) -> Option<Vec<Session>> {
        let relevant = schedule::still_relevant(&sessions, now_unix, CACHED_SESSIONS);
        self.sessions = sessions;
        (relevant != self.saved).then_some(relevant)
    }

    /// `sessions` are in flash now.
    pub fn saved(&mut self, sessions: Vec<Session>) {
        self.saved = sessions;
    }

    /// The phase at `now_unix`, and whether it differs from the one last
    /// returned as changed, which then counts as sent.
    pub fn phase(&mut self, now_unix: i64, feed_running: bool) -> (SessionPhase, bool) {
        let phase = schedule::phase_at(&self.sessions, now_unix, feed_running);
        let changed = self.sent_phase != Some(phase);
        self.sent_phase = Some(phase);
        (phase, changed)
    }
}

/// `2 d 5 h 48 min`, or `3 h 20 min` when under a day.
pub fn in_words(seconds: i64) -> String {
    let minutes = seconds / 60;
    let (days, hours, minutes) = (minutes / (24 * 60), minutes / 60 % 24, minutes % 60);
    if days > 0 {
        format!("{days} d {hours} h {minutes} min")
    } else {
        format!("{hours} h {minutes} min")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::schedule::{SessionKind, LIVE_GRACE, POST_SESSION};

    const HOUR: i64 = 60 * 60;

    fn race(key: u32, start: i64) -> Session {
        Session {
            key,
            kind: SessionKind::Race,
            start,
            end: start + 2 * HOUR,
        }
    }

    #[test]
    fn the_earliest_deadline_is_the_next_wake_up() {
        let t0 = Instant::now();
        let mut due = Deadlines::all(t0 + Duration::from_secs(60));
        assert_eq!(due.earliest(), t0 + Duration::from_secs(60));
        due.winner = t0 + Duration::from_secs(5);
        assert_eq!(due.earliest(), t0 + Duration::from_secs(5));
    }

    #[test]
    fn the_first_phase_is_always_sent() {
        let mut calendar = Calendar::new(vec![]);
        assert_eq!(calendar.phase(0, false), (SessionPhase::Idle, true));
    }

    #[test]
    fn an_unchanged_phase_is_sent_once() {
        let mut calendar = Calendar::new(vec![race(1, 10 * HOUR)]);
        assert_eq!(calendar.phase(10 * HOUR, false), (SessionPhase::Live, true));
        assert_eq!(
            calendar.phase(10 * HOUR + 60, false),
            (SessionPhase::Live, false)
        );
        let after = 12 * HOUR + LIVE_GRACE;
        assert_eq!(
            calendar.phase(after, false),
            (SessionPhase::PostSession, true)
        );
    }

    #[test]
    fn a_new_list_is_saved_only_when_its_relevant_part_changed() {
        let now = 0;
        let mut calendar = Calendar::new(vec![race(1, 10 * HOUR)]);
        assert_eq!(calendar.replace(vec![race(1, 10 * HOUR)], now), None);

        let moved = vec![race(1, 11 * HOUR)];
        assert_eq!(calendar.replace(moved.clone(), now), Some(moved.clone()));
        calendar.saved(moved.clone());
        assert_eq!(calendar.replace(moved, now), None);
    }

    #[test]
    fn a_failed_save_is_tried_again_with_the_next_list() {
        let mut calendar = Calendar::new(vec![]);
        let list = vec![race(1, 10 * HOUR)];
        assert!(calendar.replace(list.clone(), 0).is_some());
        assert!(calendar.replace(list, 0).is_some());
    }

    #[test]
    fn sessions_that_are_over_are_not_saved() {
        let over = race(1, 0);
        let coming = race(2, 48 * HOUR);
        let now = over.end + LIVE_GRACE + POST_SESSION;
        let mut calendar = Calendar::new(vec![]);
        assert_eq!(
            calendar.replace(vec![over, coming], now),
            Some(vec![coming])
        );
        assert_eq!(calendar.sessions(), [over, coming]);
    }

    #[test]
    fn countdowns_read_in_days_hours_and_minutes() {
        assert_eq!(
            in_words(2 * 24 * HOUR + 5 * HOUR + 48 * 60 + 59),
            "2 d 5 h 48 min"
        );
        assert_eq!(in_words(3 * HOUR + 20 * 60), "3 h 20 min");
        assert_eq!(in_words(59), "0 h 0 min");
    }
}
