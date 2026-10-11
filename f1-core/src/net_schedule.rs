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

/// What [`Calendar::phase`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhaseUpdate {
    pub phase: SessionPhase,
    /// It differs from the phase last returned, which now counts as sent.
    pub changed: bool,
    /// The feed was wanted with the last phase and isn't any more: a session
    /// is over, and the calendar download skipped during it can happen.
    pub session_over: bool,
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

    /// The phase at `now_unix`, compared with the one last returned.
    pub fn phase(&mut self, now_unix: i64, feed_running: bool) -> PhaseUpdate {
        let phase = schedule::phase_at(&self.sessions, now_unix, feed_running);
        let before = self.sent_phase.replace(phase);
        PhaseUpdate {
            phase,
            changed: before != Some(phase),
            session_over: before.is_some_and(schedule::feed_wanted)
                && !schedule::feed_wanted(phase),
        }
    }

    /// Whether the calendar may be downloaded at `now_unix`: not just before
    /// or during a session, when OpenF1 refuses free requests anyway
    /// (HTTP 401, "Live F1 session in progress") and each try would cost a
    /// TLS handshake next to the open feed. Worked out afresh, so it holds
    /// right after a boot too, before any phase was sent.
    pub fn may_download(&self, now_unix: i64, feed_running: bool) -> bool {
        let phase = schedule::phase_at(&self.sessions, now_unix, feed_running);
        !schedule::feed_wanted(phase)
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
    use crate::schedule::{SessionKind, LIVE_GRACE, POST_SESSION, PRE_SESSION};

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

    fn phase_and_change(calendar: &mut Calendar, now: i64) -> (SessionPhase, bool) {
        let update = calendar.phase(now, false);
        (update.phase, update.changed)
    }

    #[test]
    fn the_first_phase_is_always_sent() {
        let mut calendar = Calendar::new(vec![]);
        assert_eq!(
            phase_and_change(&mut calendar, 0),
            (SessionPhase::Idle, true)
        );
    }

    #[test]
    fn an_unchanged_phase_is_sent_once() {
        let mut calendar = Calendar::new(vec![race(1, 10 * HOUR)]);
        assert_eq!(
            phase_and_change(&mut calendar, 10 * HOUR),
            (SessionPhase::Live, true)
        );
        assert_eq!(
            phase_and_change(&mut calendar, 10 * HOUR + 60),
            (SessionPhase::Live, false)
        );
        let after = 12 * HOUR + LIVE_GRACE;
        assert_eq!(
            phase_and_change(&mut calendar, after),
            (SessionPhase::PostSession, true)
        );
    }

    #[test]
    fn no_download_just_before_or_during_a_session() {
        let calendar = Calendar::new(vec![race(1, 10 * HOUR)]);
        assert!(calendar.may_download(10 * HOUR - PRE_SESSION - 1, false));
        assert!(!calendar.may_download(10 * HOUR - PRE_SESSION, false));
        assert!(!calendar.may_download(11 * HOUR, false));
        let live_ends = 12 * HOUR + LIVE_GRACE;
        assert!(!calendar.may_download(live_ends - 1, false));
        assert!(calendar.may_download(live_ends, false));
    }

    #[test]
    fn no_download_while_the_feed_says_the_session_overruns() {
        let calendar = Calendar::new(vec![race(1, 10 * HOUR)]);
        let live_ends = 12 * HOUR + LIVE_GRACE;
        assert!(!calendar.may_download(live_ends + 60, true));
    }

    #[test]
    fn downloads_work_without_any_sessions() {
        assert!(Calendar::new(vec![]).may_download(0, false));
    }

    #[test]
    fn a_session_is_over_once_when_the_feed_stops_being_wanted() {
        let mut calendar = Calendar::new(vec![race(1, 10 * HOUR)]);
        let mut over = |now| calendar.phase(now, false).session_over;
        assert!(!over(0));
        assert!(!over(10 * HOUR - PRE_SESSION));
        assert!(!over(11 * HOUR));
        let live_ends = 12 * HOUR + LIVE_GRACE;
        assert!(over(live_ends));
        assert!(!over(live_ends + 60));
    }

    #[test]
    fn a_boot_after_a_session_is_not_a_session_ending() {
        let mut calendar = Calendar::new(vec![race(1, 10 * HOUR)]);
        let after = 12 * HOUR + LIVE_GRACE;
        assert!(!calendar.phase(after, false).session_over);
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
