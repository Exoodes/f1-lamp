use crate::input::SessionPhase;

/// Pre-session starts this long before the scheduled start.
pub const PRE_SESSION: i64 = 30 * 60;
/// Still live this long after the scheduled end: sessions overrun (red flags, delays).
pub const LIVE_GRACE: i64 = 30 * 60;
/// Post-session (the winner window) lasts this long after live ends.
pub const POST_SESSION: i64 = 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Practice,
    Qualifying,
    SprintQualifying,
    Sprint,
    Race,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    pub key: u32,
    pub kind: SessionKind,
    /// Unix seconds, UTC.
    pub start: i64,
    /// Unix seconds, UTC.
    pub end: i64,
}

impl SessionKind {
    /// Whether the lamp follows this kind of session at all. Practice is left
    /// out so Fridays don't keep the lamp in pre-session status.
    pub fn is_tracked(self) -> bool {
        matches!(
            self,
            SessionKind::Race
                | SessionKind::Sprint
                | SessionKind::Qualifying
                | SessionKind::SprintQualifying
        )
    }
    /// Whether the session ends with a winner, and so a post-session window.
    pub fn has_winner(self) -> bool {
        matches!(self, SessionKind::Race | SessionKind::Sprint)
    }
}

/// The phase this one session puts the lamp in at `now`, if any. Each window
/// includes its first second and excludes its last, so a boundary second
/// belongs to the later phase.
fn phase_of(session: &Session, now: i64) -> Option<SessionPhase> {
    if !session.kind.is_tracked() {
        return None;
    }

    let pre_start = session.start - PRE_SESSION;
    let live_end = session.end + LIVE_GRACE;
    let post_end = live_end + POST_SESSION;

    if now < pre_start {
        None
    } else if now < session.start {
        Some(SessionPhase::PreSession)
    } else if now < live_end {
        Some(SessionPhase::Live)
    } else if now < post_end && session.kind.has_winner() {
        Some(SessionPhase::PostSession)
    } else {
        None
    }
}

/// When sessions overlap, the phase with the higher rank decides.
fn rank(phase: SessionPhase) -> u8 {
    match phase {
        SessionPhase::Live => 3,
        SessionPhase::PreSession => 2,
        SessionPhase::PostSession => 1,
        SessionPhase::Idle => 0,
    }
}

/// The session that decides the lamp's phase at `now`, and that phase; `None`
/// when the lamp is idle. On a tie the later session in the list wins, so
/// with sorted sessions the newer one decides.
pub fn current_session(sessions: &[Session], now: i64) -> Option<(SessionPhase, &Session)> {
    sessions
        .iter()
        .filter_map(|s| phase_of(s, now).map(|p| (p, s)))
        .max_by_key(|(p, _)| rank(*p))
}

/// The lamp's phase at `now`.
pub fn phase_at(sessions: &[Session], now: i64) -> SessionPhase {
    current_session(sessions, now).map_or(SessionPhase::Idle, |(phase, _)| phase)
}

/// The first tracked session that hasn't started yet. `sessions` must be
/// sorted by start, as `openf1::usable_sessions` returns them.
pub fn next_session(sessions: &[Session], now: i64) -> Option<&Session> {
    sessions
        .iter()
        .find(|s| s.kind.is_tracked() && s.start > now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    /// The race used by most tests: 10:00 to 12:00 on day zero.
    const START: i64 = 10 * HOUR;
    const END: i64 = 12 * HOUR;

    fn session(key: u32, kind: SessionKind, start: i64, end: i64) -> Session {
        Session {
            key,
            kind,
            start,
            end,
        }
    }

    fn race() -> Session {
        session(1, SessionKind::Race, START, END)
    }

    fn phase_of_race_at(now: i64) -> SessionPhase {
        phase_at(&[race()], now)
    }

    // ---- One session through its windows ----

    #[test]
    fn empty_list_is_idle() {
        assert_eq!(phase_at(&[], START), SessionPhase::Idle);
        assert_eq!(current_session(&[], START), None);
    }

    #[test]
    fn well_before_a_session_is_idle() {
        assert_eq!(phase_of_race_at(START - 2 * HOUR), SessionPhase::Idle);
    }

    #[test]
    fn one_second_before_pre_session_is_idle() {
        assert_eq!(
            phase_of_race_at(START - PRE_SESSION - 1),
            SessionPhase::Idle
        );
    }

    #[test]
    fn pre_session_opens_exactly_30_minutes_before_start() {
        assert_eq!(
            phase_of_race_at(START - PRE_SESSION),
            SessionPhase::PreSession
        );
    }

    #[test]
    fn one_second_before_start_is_pre_session() {
        assert_eq!(phase_of_race_at(START - 1), SessionPhase::PreSession);
    }

    #[test]
    fn exactly_at_start_is_live() {
        assert_eq!(phase_of_race_at(START), SessionPhase::Live);
    }

    #[test]
    fn just_after_the_scheduled_end_is_still_live() {
        assert_eq!(phase_of_race_at(END + 1), SessionPhase::Live);
    }

    #[test]
    fn live_lasts_until_the_grace_period_ends() {
        assert_eq!(phase_of_race_at(END + LIVE_GRACE - 1), SessionPhase::Live);
    }

    #[test]
    fn race_enters_post_session_when_the_grace_period_ends() {
        assert_eq!(
            phase_of_race_at(END + LIVE_GRACE),
            SessionPhase::PostSession
        );
    }

    #[test]
    fn post_session_lasts_for_its_window_then_idle() {
        let post_end = END + LIVE_GRACE + POST_SESSION;
        assert_eq!(phase_of_race_at(post_end - 1), SessionPhase::PostSession);
        assert_eq!(phase_of_race_at(post_end), SessionPhase::Idle);
    }

    // ---- Session kinds ----

    #[test]
    fn sprint_has_a_post_session() {
        let sprint = session(1, SessionKind::Sprint, START, END);
        assert_eq!(
            phase_at(&[sprint], END + LIVE_GRACE),
            SessionPhase::PostSession
        );
    }

    #[test]
    fn qualifying_is_live_but_has_no_post_session() {
        for kind in [SessionKind::Qualifying, SessionKind::SprintQualifying] {
            let quali = [session(1, kind, START, END)];
            assert_eq!(phase_at(&quali, START), SessionPhase::Live, "{kind:?}");
            assert_eq!(
                phase_at(&quali, END + LIVE_GRACE),
                SessionPhase::Idle,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn practice_is_ignored() {
        let practice = [session(1, SessionKind::Practice, START, END)];
        assert_eq!(phase_at(&practice, START - 1), SessionPhase::Idle);
        assert_eq!(phase_at(&practice, START), SessionPhase::Idle);
    }

    #[test]
    fn only_races_and_sprints_have_winners() {
        assert!(SessionKind::Race.has_winner());
        assert!(SessionKind::Sprint.has_winner());
        assert!(!SessionKind::Qualifying.has_winner());
        assert!(!SessionKind::SprintQualifying.has_winner());
        assert!(!SessionKind::Practice.has_winner());
    }

    // ---- Which session decides ----

    #[test]
    fn current_session_reports_the_deciding_session() {
        let sessions = [race()];
        let (phase, s) = current_session(&sessions, START).unwrap();
        assert_eq!(phase, SessionPhase::Live);
        assert_eq!(s.key, 1);
    }

    #[test]
    fn idle_has_no_current_session() {
        assert_eq!(current_session(&[race()], START - 2 * HOUR), None);
    }

    #[test]
    fn live_beats_another_sessions_pre_session() {
        // Qualifying 10:00-11:00 is live until 11:30; the race at 11:15 is
        // in pre-session from 10:45. At 11:05 both apply.
        let sessions = [
            session(1, SessionKind::Qualifying, 10 * HOUR, 11 * HOUR),
            session(2, SessionKind::Race, 11 * HOUR + 15 * MINUTE, 13 * HOUR),
        ];
        let (phase, s) = current_session(&sessions, 11 * HOUR + 5 * MINUTE).unwrap();
        assert_eq!(phase, SessionPhase::Live);
        assert_eq!(s.key, 1);
    }

    #[test]
    fn pre_session_beats_another_sessions_post_session() {
        // The race's winner window runs 12:30-13:30; qualifying at 13:00 is
        // in pre-session from 12:30. At 12:45 both apply.
        let sessions = [
            race(),
            session(2, SessionKind::Qualifying, 13 * HOUR, 14 * HOUR),
        ];
        let (phase, s) = current_session(&sessions, 12 * HOUR + 45 * MINUTE).unwrap();
        assert_eq!(phase, SessionPhase::PreSession);
        assert_eq!(s.key, 2);
    }

    #[test]
    fn when_two_sessions_are_live_the_later_one_decides() {
        // Qualifying 10:00-11:00 is still in its grace period at 11:20,
        // when the race (11:15) has really started.
        let sessions = [
            session(1, SessionKind::Qualifying, 10 * HOUR, 11 * HOUR),
            session(2, SessionKind::Race, 11 * HOUR + 15 * MINUTE, 13 * HOUR),
        ];
        let (phase, s) = current_session(&sessions, 11 * HOUR + 20 * MINUTE).unwrap();
        assert_eq!(phase, SessionPhase::Live);
        assert_eq!(s.key, 2);
    }

    #[test]
    fn untracked_sessions_never_decide() {
        // Practice overlapping a race's pre-session doesn't change anything.
        let sessions = [
            session(9, SessionKind::Practice, START - HOUR, START),
            race(),
        ];
        let (phase, s) = current_session(&sessions, START - 10 * MINUTE).unwrap();
        assert_eq!(phase, SessionPhase::PreSession);
        assert_eq!(s.key, 1);
    }

    // ---- Next session ----

    #[test]
    fn empty_list_has_no_next_session() {
        assert_eq!(next_session(&[], START), None);
    }

    #[test]
    fn next_session_is_the_first_one_not_yet_started() {
        let sessions = [
            session(1, SessionKind::Qualifying, START, END),
            session(2, SessionKind::Race, START + 24 * HOUR, END + 24 * HOUR),
        ];
        assert_eq!(
            next_session(&sessions, START - HOUR).map(|s| s.key),
            Some(1)
        );
    }

    #[test]
    fn a_session_that_has_started_is_not_next() {
        let sessions = [
            session(1, SessionKind::Qualifying, START, END),
            session(2, SessionKind::Race, START + 24 * HOUR, END + 24 * HOUR),
        ];
        assert_eq!(next_session(&sessions, START).map(|s| s.key), Some(2));
    }

    #[test]
    fn next_session_skips_practice() {
        let sessions = [
            session(1, SessionKind::Practice, START, END),
            session(2, SessionKind::Race, START + 24 * HOUR, END + 24 * HOUR),
        ];
        assert_eq!(next_session(&sessions, 0).map(|s| s.key), Some(2));
    }

    #[test]
    fn no_next_session_after_the_last_one() {
        assert_eq!(next_session(&[race()], START), None);
    }
}
