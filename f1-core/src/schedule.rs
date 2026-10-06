use serde::{Deserialize, Serialize};

use crate::input::SessionPhase;

/// Pre-session starts this long before the scheduled start.
pub const PRE_SESSION: i64 = 30 * 60;
/// Still live this long after the scheduled end: sessions overrun (red flags, delays).
pub const LIVE_GRACE: i64 = 30 * 60;
/// Post-session (the winner window) lasts this long after live ends.
pub const POST_SESSION: i64 = 60 * 60;
/// How many sessions the cache keeps; NVS entries have size limits.
pub const CACHED_SESSIONS: usize = 12;
/// The most bytes the cached schedule's JSON may take: the firmware reads it
/// into a buffer this big, and a longer entry couldn't be read back. A test
/// below checks that `CACHED_SESSIONS` sessions fit with room to spare.
pub const MAX_JSON: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKind {
    Practice,
    Qualifying,
    SprintQualifying,
    Sprint,
    Race,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub key: u32,
    pub kind: SessionKind,
    /// Unix seconds, UTC.
    pub start: i64,
    /// Unix seconds, UTC.
    pub end: i64,
}

impl SessionKind {
    /// Whether the session ends with a winner, and so a post-session window.
    pub fn has_winner(self) -> bool {
        matches!(self, SessionKind::Race | SessionKind::Sprint)
    }
}

/// The phase this one session puts the lamp in at `now`, if any. Each window
/// includes its first second and excludes its last, so a boundary second
/// belongs to the later phase.
///
/// Every kind of session is followed, practice too: flags and red flags
/// happen there as well, and Friday practice is the first live test of a
/// weekend.
fn phase_of(session: &Session, now: i64) -> Option<SessionPhase> {
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

/// The first session that hasn't started yet. `sessions` must be
/// sorted by start, as `openf1::usable_sessions` returns them.
/// The next moment after `now` at which the phase can change: a session's
/// pre-session start, start, end of live, or end of post-session.
/// Waking up then makes phase changes exact instead of up to a polling
/// interval late, which matters for the start lights.
pub fn next_change(sessions: &[Session], now: i64) -> Option<i64> {
    sessions
        .iter()
        .flat_map(|s| {
            let live_end = s.end + LIVE_GRACE;
            let post_end = s.kind.has_winner().then_some(live_end + POST_SESSION);
            [
                Some(s.start - PRE_SESSION),
                Some(s.start),
                Some(live_end),
                post_end,
            ]
        })
        .flatten()
        .filter(|&t| t > now)
        .min()
}

pub fn next_session(sessions: &[Session], now: i64) -> Option<&Session> {
    sessions.iter().find(|s| s.start > now)
}

/// The sessions that can still affect the lamp at `now` (not fully over,
/// including their post-session window): the soonest `max`, in start order.
/// `sessions` must be sorted by start.
pub fn still_relevant(sessions: &[Session], now: i64, max: usize) -> Vec<Session> {
    sessions
        .iter()
        .filter(|s| s.end + LIVE_GRACE + POST_SESSION > now)
        .take(max)
        .copied()
        .collect()
}

/// A race or sprint whose post-session (winner) window contains `now`. Unlike
/// `phase_at`, another session's pre-session window doesn't hide it.
pub fn awaiting_winner(sessions: &[Session], now: i64) -> Option<&Session> {
    sessions.iter().find(|s| {
        let post_start = s.end + LIVE_GRACE;
        s.kind.has_winner() && post_start <= now && now < post_start + POST_SESSION
    })
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
    fn practice_is_followed_like_qualifying() {
        let practice = [session(1, SessionKind::Practice, START, END)];
        assert_eq!(phase_at(&practice, START - 1), SessionPhase::PreSession);
        assert_eq!(phase_at(&practice, START), SessionPhase::Live);
        assert_eq!(phase_at(&practice, END + LIVE_GRACE), SessionPhase::Idle);
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
    fn live_practice_wins_over_a_later_sessions_pre_session() {
        // Practice still running while the race's pre-session has begun.
        let sessions = [
            session(9, SessionKind::Practice, START - HOUR, START),
            race(),
        ];
        let (phase, s) = current_session(&sessions, START - 10 * MINUTE).unwrap();
        assert_eq!(phase, SessionPhase::Live);
        assert_eq!(s.key, 9);
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
    fn next_session_includes_practice() {
        let sessions = [
            session(1, SessionKind::Practice, START, END),
            session(2, SessionKind::Race, START + 24 * HOUR, END + 24 * HOUR),
        ];
        assert_eq!(next_session(&sessions, 0).map(|s| s.key), Some(1));
    }

    #[test]
    fn no_next_session_after_the_last_one() {
        assert_eq!(next_session(&[race()], START), None);
    }

    // ---- Cache ----

    /// When the race's post-session window is over: 13:30.
    const RACE_OVER: i64 = END + LIVE_GRACE + POST_SESSION;

    #[test]
    fn finished_sessions_are_dropped() {
        assert!(still_relevant(&[race()], RACE_OVER, 10).is_empty());
    }

    #[test]
    fn a_race_in_its_post_session_window_is_kept() {
        assert_eq!(still_relevant(&[race()], RACE_OVER - 1, 10), [race()]);
    }

    #[test]
    fn qualifying_is_kept_until_the_same_time_as_a_race() {
        // Simpler than per-kind rules; a stale qualifying entry costs nothing.
        let quali = session(1, SessionKind::Qualifying, START, END);
        assert_eq!(still_relevant(&[quali], RACE_OVER - 1, 10), [quali]);
    }

    #[test]
    fn upcoming_sessions_are_kept() {
        let later = session(2, SessionKind::Race, START + 24 * HOUR, END + 24 * HOUR);
        assert_eq!(still_relevant(&[race(), later], RACE_OVER, 10), [later]);
    }

    #[test]
    fn at_most_max_sessions_are_kept_the_soonest_first() {
        let sessions = [
            session(1, SessionKind::Qualifying, START, END),
            session(2, SessionKind::Race, START + 24 * HOUR, END + 24 * HOUR),
            session(3, SessionKind::Race, START + 48 * HOUR, END + 48 * HOUR),
        ];
        let keys: Vec<u32> = still_relevant(&sessions, 0, 2)
            .iter()
            .map(|s| s.key)
            .collect();
        assert_eq!(keys, [1, 2]);
    }

    #[test]
    fn empty_list_stays_empty() {
        assert!(still_relevant(&[], START, 10).is_empty());
    }

    #[test]
    fn cached_sessions_survive_a_json_round_trip() {
        let sessions = vec![
            race(),
            session(2, SessionKind::SprintQualifying, START + HOUR, END + HOUR),
        ];
        let json = serde_json::to_string(&sessions).unwrap();
        let back: Vec<Session> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sessions);
    }

    #[test]
    fn a_full_cache_fits_comfortably_in_nvs() {
        // Realistic keys and times, and the longest kind name.
        let sessions: Vec<Session> = (0..CACHED_SESSIONS as u32)
            .map(|i| Session {
                key: 11_379 + i,
                kind: SessionKind::SprintQualifying,
                start: 1_789_221_600 + i64::from(i) * 86_400,
                end: 1_789_225_200 + i64::from(i) * 86_400,
            })
            .collect();
        let json = serde_json::to_string(&sessions).unwrap();
        assert!(json.len() < MAX_JSON * 3 / 4, "{} bytes", json.len());
    }

    // ---- Winner window ----

    /// When the race's winner window opens: 12:30.
    const POST_START: i64 = END + LIVE_GRACE;

    fn awaiting_key(sessions: &[Session], now: i64) -> Option<u32> {
        awaiting_winner(sessions, now).map(|s| s.key)
    }

    #[test]
    fn race_in_post_session_awaits_a_winner() {
        assert_eq!(awaiting_key(&[race()], POST_START), Some(1));
    }

    #[test]
    fn live_race_does_not_await_a_winner_yet() {
        assert_eq!(awaiting_key(&[race()], POST_START - 1), None);
        assert_eq!(awaiting_key(&[race()], START), None);
    }

    #[test]
    fn winner_window_closes_after_post_session() {
        assert_eq!(awaiting_key(&[race()], RACE_OVER - 1), Some(1));
        assert_eq!(awaiting_key(&[race()], RACE_OVER), None);
    }

    #[test]
    fn sprint_awaits_a_winner() {
        let sprint = session(1, SessionKind::Sprint, START, END);
        assert_eq!(awaiting_key(&[sprint], POST_START), Some(1));
    }

    #[test]
    fn qualifying_never_awaits_a_winner() {
        for kind in [SessionKind::Qualifying, SessionKind::SprintQualifying] {
            let quali = session(1, kind, START, END);
            assert_eq!(awaiting_key(&[quali], POST_START), None, "{kind:?}");
        }
    }

    #[test]
    fn awaiting_winner_ignores_a_later_sessions_pre_session() {
        // At 12:45 the race (key 1) is in its winner window, while qualifying
        // at 13:00 is in pre-session. The phase says PreSession, but the
        // winner is still wanted.
        let sessions = [
            race(),
            session(2, SessionKind::Qualifying, 13 * HOUR, 14 * HOUR),
        ];
        let now = 12 * HOUR + 45 * MINUTE;
        assert_eq!(phase_at(&sessions, now), SessionPhase::PreSession);
        assert_eq!(awaiting_key(&sessions, now), Some(1));
    }

    #[test]
    fn empty_list_awaits_no_winner() {
        assert_eq!(awaiting_key(&[], POST_START), None);
    }

    #[test]
    fn next_change_walks_through_every_boundary_of_a_race() {
        let race = [session(1, SessionKind::Race, START, END)];
        let pre = START - PRE_SESSION;
        let live_end = END + LIVE_GRACE;
        let post_end = live_end + POST_SESSION;
        assert_eq!(next_change(&race, 0), Some(pre));
        assert_eq!(next_change(&race, pre), Some(START));
        assert_eq!(next_change(&race, START), Some(live_end));
        assert_eq!(next_change(&race, live_end), Some(post_end));
        assert_eq!(next_change(&race, post_end), None);
    }

    #[test]
    fn phase_really_changes_at_each_boundary() {
        let race = [session(1, SessionKind::Race, START, END)];
        let mut t = 0;
        let mut phases = vec![phase_at(&race, t)];
        while let Some(next) = next_change(&race, t) {
            assert_ne!(
                phase_at(&race, next - 1),
                phase_at(&race, next),
                "at {next}"
            );
            phases.push(phase_at(&race, next));
            t = next;
        }
        assert_eq!(
            phases,
            [
                SessionPhase::Idle,
                SessionPhase::PreSession,
                SessionPhase::Live,
                SessionPhase::PostSession,
                SessionPhase::Idle
            ]
        );
    }

    #[test]
    fn qualifying_has_no_post_session_boundary() {
        let quali = [session(1, SessionKind::Qualifying, START, END)];
        assert_eq!(next_change(&quali, START), Some(END + LIVE_GRACE));
        assert_eq!(next_change(&quali, END + LIVE_GRACE), None);
    }

    #[test]
    fn practice_has_boundaries_but_no_post_session() {
        let practice = [session(1, SessionKind::Practice, START, END)];
        assert_eq!(next_change(&practice, 0), Some(START - PRE_SESSION));
        assert_eq!(next_change(&practice, START), Some(END + LIVE_GRACE));
        assert_eq!(next_change(&practice, END + LIVE_GRACE), None);
    }

    #[test]
    fn next_change_is_the_earliest_over_all_sessions() {
        let weekend = [
            session(1, SessionKind::Qualifying, START, END),
            session(2, SessionKind::Race, START + 24 * HOUR, END + 24 * HOUR),
        ];
        assert_eq!(next_change(&weekend, END), Some(END + LIVE_GRACE));
        assert_eq!(
            next_change(&weekend, END + LIVE_GRACE),
            Some(START + 24 * HOUR - PRE_SESSION)
        );
    }
}
