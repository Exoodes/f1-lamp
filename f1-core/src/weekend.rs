//! [`Weekend`]: what the page's schedule shows: the Grand Prix whose session
//! is on or next (or ended less than a day ago), with that weekend's sessions
//! the lamp knows and their results.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{
    color::Rgb,
    schedule::{Session, SessionKind},
    settings::SessionToggles,
};

/// A weekend stays on the page this long after its last session, so the
/// race's result can be seen.
pub const SHOWN_AFTER: i64 = 24 * 60 * 60;
/// OpenF1 is asked for a session's result again and again until this long
/// after its end; after that, once more and then never, so a session that
/// never got one (e.g. abandoned) isn't asked forever. An older session
/// (the lamp was off or flashed since) is still asked that once.
pub const RESULT_GIVE_UP: i64 = 6 * 60 * 60;

/// Who was P1 in a session: the fastest in practice, pole in qualifying, the
/// winner of a race.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionResult {
    pub driver: u8,
    /// `"VER"`.
    pub name: String,
    pub color: Rgb,
}

/// Session key → its result.
pub type Results = HashMap<u32, SessionResult>;

/// Session key → the start it had before it moved (Unix seconds).
pub type Moved = HashMap<u32, i64>;

/// A session whose start changed between two calendar downloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub key: u32,
    pub from: i64,
    pub to: i64,
}

/// Compares a new calendar with the one before it, and keeps in `moved` the
/// first known start of every session that moved: a session moved twice
/// still shows where it started from, and one moved back is no longer
/// moved. Sessions gone from the new calendar are forgotten. Returns this
/// download's moves, for the log.
pub fn track_moves(moved: &mut Moved, old: &[Session], new: &[Session]) -> Vec<Move> {
    moved.retain(|key, _| new.iter().any(|s| s.key == *key));
    let mut moves = Vec::new();
    for session in new {
        let Some(before) = old.iter().find(|s| s.key == session.key) else {
            continue;
        };
        if before.start == session.start {
            continue;
        }
        moves.push(Move {
            key: session.key,
            from: before.start,
            to: session.start,
        });
        let first = *moved.entry(session.key).or_insert(before.start);
        if first == session.start {
            moved.remove(&session.key);
        }
    }
    moves
}

/// What `GET /api/schedule` answers.
#[derive(Debug, PartialEq, Serialize)]
pub struct ScheduleReply {
    /// The lamp's time (Unix seconds), so the page counts down from it and
    /// not from a phone's clock; `None` before the lamp knows the time.
    pub now: Option<i64>,
    pub weekend: Option<Weekend>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Weekend {
    pub country: String,
    pub circuit: String,
    pub sessions: Vec<WeekendSession>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct WeekendSession {
    pub kind: SessionKind,
    pub name: String,
    /// Unix seconds, UTC.
    pub start: i64,
    /// Unix seconds, UTC.
    pub end: i64,
    pub state: SessionState,
    /// Switched off in the settings: the lamp ignores it.
    pub followed: bool,
    pub result: Option<SessionResult>,
    /// The start it had before it moved (Unix seconds); `None` if it didn't.
    pub moved_from: Option<i64>,
}

/// By the scheduled times: a session running late still says `Done`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Done,
    Live,
    Coming,
}

/// The weekend of the first session that ended less than [`SHOWN_AFTER`]
/// before `now` or hasn't ended, with every session of it in `sessions`
/// (sorted by start), finished ones included. `None` when no session is
/// recent, on or coming.
pub fn weekend(
    sessions: &[Session],
    now: i64,
    follow: SessionToggles,
    results: &Results,
    moved: &Moved,
) -> Option<Weekend> {
    let sessions = weekend_sessions(sessions, now);
    let first = sessions.first()?;
    Some(Weekend {
        country: first.country.clone(),
        circuit: first.circuit.clone(),
        sessions: sessions
            .iter()
            .map(|s| WeekendSession {
                kind: s.kind,
                name: s.label().to_string(),
                start: s.start,
                end: s.end,
                state: state(s, now),
                followed: follow.follows(s.kind),
                result: results.get(&s.key).cloned(),
                moved_from: moved.get(&s.key).copied(),
            })
            .collect(),
    })
}

/// Which result to ask OpenF1 for (see [`result_due`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultDue {
    pub key: u32,
    /// Ended [`RESULT_GIVE_UP`] or more ago: if OpenF1 has no result, give
    /// up on it.
    pub last_try: bool,
}

/// The finished session of the weekend shown at `now` whose result to ask
/// OpenF1 for next: one with no result in `results` that wasn't given up on.
pub fn result_due(
    sessions: &[Session],
    now: i64,
    results: &Results,
    given_up: &HashSet<u32>,
) -> Option<ResultDue> {
    weekend_sessions(sessions, now)
        .iter()
        .find(|s| s.end <= now && !results.contains_key(&s.key) && !given_up.contains(&s.key))
        .map(|s| ResultDue {
            key: s.key,
            last_try: now >= s.end + RESULT_GIVE_UP,
        })
}

/// The sessions of the weekend shown at `now`. A session without a weekend
/// (from a calendar saved before weekends were kept) stands alone; the next
/// download fills them in.
fn weekend_sessions(sessions: &[Session], now: i64) -> Vec<&Session> {
    let Some(first) = sessions.iter().find(|s| s.end + SHOWN_AFTER > now) else {
        return Vec::new();
    };
    sessions
        .iter()
        .filter(|s| {
            if first.meeting == 0 {
                s.key == first.key
            } else {
                s.meeting == first.meeting
            }
        })
        .collect()
}

fn state(session: &Session, now: i64) -> SessionState {
    if now >= session.end {
        SessionState::Done
    } else if now >= session.start {
        SessionState::Live
    } else {
        SessionState::Coming
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::SessionKind;

    const ALL: SessionToggles = SessionToggles {
        practice: true,
        qualifying: true,
        sprint_qualifying: true,
        sprint: true,
        race: true,
    };
    const HOUR: i64 = 60 * 60;
    const DAY: i64 = 24 * HOUR;

    fn session(key: u32, meeting: u32, name: &str, start: i64) -> Session {
        let kind = match name {
            "Race" => SessionKind::Race,
            "Qualifying" => SessionKind::Qualifying,
            _ => SessionKind::Practice,
        };
        Session {
            meeting,
            name: name.to_string(),
            country: if meeting == 1 {
                "Singapore"
            } else {
                "United States"
            }
            .to_string(),
            circuit: if meeting == 1 { "Singapore" } else { "Austin" }.to_string(),
            ..Session::new(key, kind, start, start + HOUR)
        }
    }

    /// Singapore: practice Friday, qualifying Saturday, race Sunday; then
    /// Austin a fortnight later.
    fn calendar() -> Vec<Session> {
        vec![
            session(1, 1, "Practice 1", 0),
            session(2, 1, "Qualifying", DAY),
            session(3, 1, "Race", 2 * DAY),
            session(4, 2, "Practice 1", 14 * DAY),
            session(5, 2, "Race", 16 * DAY),
        ]
    }

    fn names_and_states(weekend: &Weekend) -> Vec<(&str, SessionState)> {
        weekend
            .sessions
            .iter()
            .map(|s| (s.name.as_str(), s.state))
            .collect()
    }

    #[test]
    fn before_the_weekend_every_session_is_coming() {
        let w = weekend(&calendar(), -DAY, ALL, &Results::new(), &Moved::new()).unwrap();
        assert_eq!(
            (w.country.as_str(), w.circuit.as_str()),
            ("Singapore", "Singapore")
        );
        assert_eq!(
            names_and_states(&w),
            [
                ("Practice 1", SessionState::Coming),
                ("Qualifying", SessionState::Coming),
                ("Race", SessionState::Coming),
            ]
        );
    }

    #[test]
    fn during_the_weekend_finished_and_live_sessions_show_too() {
        let w = weekend(
            &calendar(),
            DAY + HOUR / 2,
            ALL,
            &Results::new(),
            &Moved::new(),
        )
        .unwrap();
        assert_eq!(
            names_and_states(&w),
            [
                ("Practice 1", SessionState::Done),
                ("Qualifying", SessionState::Live),
                ("Race", SessionState::Coming),
            ]
        );
    }

    #[test]
    fn a_session_is_live_from_its_start_until_its_end() {
        let race = 2 * DAY;
        let state_at = |now| {
            weekend(&calendar(), now, ALL, &Results::new(), &Moved::new())
                .unwrap()
                .sessions[2]
                .state
        };
        assert_eq!(state_at(race - 1), SessionState::Coming);
        assert_eq!(state_at(race), SessionState::Live);
        assert_eq!(state_at(race + HOUR - 1), SessionState::Live);
    }

    #[test]
    fn after_the_race_the_next_weekend_shows() {
        let w = weekend(
            &calendar(),
            3 * DAY + HOUR,
            ALL,
            &Results::new(),
            &Moved::new(),
        )
        .unwrap();
        assert_eq!(w.circuit, "Austin");
        assert_eq!(w.sessions.len(), 2);
    }

    #[test]
    fn nothing_when_no_session_is_coming() {
        assert_eq!(
            weekend(
                &calendar(),
                17 * DAY + HOUR,
                ALL,
                &Results::new(),
                &Moved::new()
            ),
            None
        );
        assert_eq!(weekend(&[], 0, ALL, &Results::new(), &Moved::new()), None);
    }

    #[test]
    fn a_session_saved_without_its_weekend_stands_alone() {
        let old = vec![
            Session::new(1, SessionKind::Qualifying, DAY, DAY + HOUR),
            Session::new(2, SessionKind::Race, 2 * DAY, 2 * DAY + HOUR),
        ];
        let w = weekend(&old, 0, ALL, &Results::new(), &Moved::new()).unwrap();
        assert_eq!(names_and_states(&w), [("Qualifying", SessionState::Coming)]);
        assert_eq!(w.country, "");
    }

    #[test]
    fn sessions_switched_off_show_as_not_followed() {
        let follow = SessionToggles {
            practice: false,
            ..ALL
        };
        let w = weekend(&calendar(), -DAY, follow, &Results::new(), &Moved::new()).unwrap();
        let followed: Vec<bool> = w.sessions.iter().map(|s| s.followed).collect();
        assert_eq!(followed, [false, true, true]);
    }

    #[test]
    fn the_weekend_stays_for_a_day_after_the_race() {
        let race_end = 2 * DAY + HOUR;
        let w = weekend(
            &calendar(),
            race_end + DAY - 1,
            ALL,
            &Results::new(),
            &Moved::new(),
        )
        .unwrap();
        assert_eq!(w.circuit, "Singapore");
        assert_eq!(w.sessions[2].state, SessionState::Done);
    }

    fn verstappen() -> SessionResult {
        SessionResult {
            driver: 1,
            name: "VER".to_string(),
            color: Rgb::new(0x47, 0x81, 0xD7),
        }
    }

    #[test]
    fn results_show_next_to_their_sessions() {
        let results = Results::from([(2, verstappen())]);
        let w = weekend(&calendar(), 2 * DAY - HOUR, ALL, &results, &Moved::new()).unwrap();
        let shown: Vec<Option<&str>> = w
            .sessions
            .iter()
            .map(|s| s.result.as_ref().map(|r| r.name.as_str()))
            .collect();
        assert_eq!(shown, [None, Some("VER"), None]);
    }

    fn due_key(now: i64, results: &Results, given_up: &HashSet<u32>) -> Option<u32> {
        result_due(&calendar(), now, results, given_up).map(|d| d.key)
    }

    #[test]
    fn results_are_asked_for_in_order_once_each_session_ends() {
        let none = HashSet::new();
        let mut results = Results::new();
        assert_eq!(due_key(HOUR - 1, &results, &none), None);
        assert_eq!(due_key(HOUR, &results, &none), Some(1));
        assert_eq!(due_key(2 * HOUR, &results, &none), Some(1));
        results.insert(1, verstappen());
        assert_eq!(due_key(2 * HOUR, &results, &none), None);
        assert_eq!(due_key(DAY + HOUR, &results, &none), Some(2));
    }

    #[test]
    fn older_sessions_are_still_asked_once() {
        // Race morning, after a flash: Friday's and Saturday's results.
        let none = HashSet::new();
        let due = result_due(&calendar(), 2 * DAY - HOUR, &Results::new(), &none).unwrap();
        assert_eq!(
            due,
            ResultDue {
                key: 1,
                last_try: true
            }
        );
        let given_up = HashSet::from([1]);
        assert_eq!(due_key(2 * DAY - HOUR, &Results::new(), &given_up), Some(2));
    }

    #[test]
    fn recent_sessions_are_asked_until_the_give_up_time() {
        let race_end = 2 * DAY + HOUR;
        let results = Results::from([(1, verstappen()), (2, verstappen())]);
        let none = HashSet::new();
        let due = |now| result_due(&calendar(), now, &results, &none).unwrap();
        assert_eq!(
            due(race_end),
            ResultDue {
                key: 3,
                last_try: false
            }
        );
        assert!(!due(race_end + RESULT_GIVE_UP - 1).last_try);
        assert!(due(race_end + RESULT_GIVE_UP).last_try);
        let given_up = HashSet::from([3]);
        assert_eq!(
            due_key(race_end + RESULT_GIVE_UP, &results, &given_up),
            None
        );
    }

    // ---- Moves ----

    fn moved_to(key: u32, start: i64) -> Session {
        Session {
            start,
            ..calendar().into_iter().find(|s| s.key == key).unwrap()
        }
    }

    #[test]
    fn a_session_that_moved_is_reported_and_kept() {
        let mut moved = Moved::new();
        let new = vec![moved_to(2, DAY + HOUR / 2), moved_to(3, 2 * DAY)];
        let moves = track_moves(&mut moved, &calendar(), &new);
        assert_eq!(
            moves,
            [Move {
                key: 2,
                from: DAY,
                to: DAY + HOUR / 2
            }]
        );
        assert_eq!(moved, Moved::from([(2, DAY)]));
    }

    #[test]
    fn a_session_moved_twice_keeps_its_first_start() {
        let mut moved = Moved::new();
        let once = vec![moved_to(2, DAY + HOUR / 2)];
        track_moves(&mut moved, &calendar(), &once);
        let twice = vec![moved_to(2, DAY + HOUR)];
        track_moves(&mut moved, &once, &twice);
        assert_eq!(moved, Moved::from([(2, DAY)]));
    }

    #[test]
    fn a_session_moved_back_is_no_longer_moved() {
        let mut moved = Moved::new();
        let once = vec![moved_to(2, DAY + HOUR / 2)];
        track_moves(&mut moved, &calendar(), &once);
        let back = vec![moved_to(2, DAY)];
        assert_eq!(track_moves(&mut moved, &once, &back).len(), 1);
        assert!(moved.is_empty());
    }

    #[test]
    fn new_and_unchanged_sessions_are_not_moves() {
        let mut moved = Moved::new();
        assert!(track_moves(&mut moved, &[], &calendar()).is_empty());
        assert!(track_moves(&mut moved, &calendar(), &calendar()).is_empty());
        assert!(moved.is_empty());
    }

    #[test]
    fn sessions_gone_from_the_calendar_are_forgotten() {
        let mut moved = Moved::from([(2, DAY), (99, 0)]);
        track_moves(&mut moved, &calendar(), &calendar());
        assert_eq!(moved, Moved::from([(2, DAY)]));
    }

    #[test]
    fn a_moved_session_shows_where_it_was() {
        let moved = Moved::from([(2, DAY - HOUR / 2)]);
        let w = weekend(&calendar(), 0, ALL, &Results::new(), &moved).unwrap();
        let from: Vec<Option<i64>> = w.sessions.iter().map(|s| s.moved_from).collect();
        assert_eq!(from, [None, Some(DAY - HOUR / 2), None]);
    }

    #[test]
    fn the_reply_reads_as_the_page_expects() {
        let reply = ScheduleReply {
            now: Some(10),
            weekend: weekend(&calendar()[2..3], 0, ALL, &Results::new(), &Moved::new()),
        };
        assert_eq!(
            serde_json::to_string(&reply).unwrap(),
            r#"{"now":10,"weekend":{"country":"Singapore","circuit":"Singapore","sessions":[{"kind":"Race","name":"Race","start":172800,"end":176400,"state":"coming","followed":true,"result":null,"moved_from":null}]}}"#
        );
    }
}
