//! [`Weekend`]: what the page's schedule shows: the Grand Prix whose session
//! is on or next, with that weekend's sessions the lamp knows.

use serde::Serialize;

use crate::{schedule::Session, settings::SessionToggles};

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
    pub name: String,
    /// Unix seconds, UTC.
    pub start: i64,
    /// Unix seconds, UTC.
    pub end: i64,
    pub state: SessionState,
    /// Switched off in the settings: the lamp ignores it.
    pub followed: bool,
}

/// By the scheduled times: a session running late still says `Done`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Done,
    Live,
    Coming,
}

/// The weekend of the first session not yet over at `now`, with every
/// session of it in `sessions` (sorted by start), finished ones included.
/// `None` when no session is on or coming.
///
/// A session without a weekend (from a calendar saved before weekends were
/// kept) stands alone; the next download fills them in.
pub fn weekend(sessions: &[Session], now: i64, follow: SessionToggles) -> Option<Weekend> {
    let first = sessions.iter().find(|s| s.end > now)?;
    let same_weekend = |s: &&Session| {
        if first.meeting == 0 {
            s.key == first.key
        } else {
            s.meeting == first.meeting
        }
    };
    Some(Weekend {
        country: first.country.clone(),
        circuit: first.circuit.clone(),
        sessions: sessions
            .iter()
            .filter(same_weekend)
            .map(|s| WeekendSession {
                name: s.label().to_string(),
                start: s.start,
                end: s.end,
                state: state(s, now),
                followed: follow.follows(s.kind),
            })
            .collect(),
    })
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
        let w = weekend(&calendar(), -DAY, ALL).unwrap();
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
        let w = weekend(&calendar(), DAY + HOUR / 2, ALL).unwrap();
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
        let state_at = |now| weekend(&calendar(), now, ALL).unwrap().sessions[2].state;
        assert_eq!(state_at(race - 1), SessionState::Coming);
        assert_eq!(state_at(race), SessionState::Live);
        assert_eq!(state_at(race + HOUR - 1), SessionState::Live);
    }

    #[test]
    fn after_the_race_the_next_weekend_shows() {
        let w = weekend(&calendar(), 2 * DAY + HOUR, ALL).unwrap();
        assert_eq!(w.circuit, "Austin");
        assert_eq!(w.sessions.len(), 2);
    }

    #[test]
    fn nothing_when_no_session_is_coming() {
        assert_eq!(weekend(&calendar(), 17 * DAY, ALL), None);
        assert_eq!(weekend(&[], 0, ALL), None);
    }

    #[test]
    fn a_session_saved_without_its_weekend_stands_alone() {
        let old = vec![
            Session::new(1, SessionKind::Qualifying, DAY, DAY + HOUR),
            Session::new(2, SessionKind::Race, 2 * DAY, 2 * DAY + HOUR),
        ];
        let w = weekend(&old, 0, ALL).unwrap();
        assert_eq!(names_and_states(&w), [("Qualifying", SessionState::Coming)]);
        assert_eq!(w.country, "");
    }

    #[test]
    fn sessions_switched_off_show_as_not_followed() {
        let follow = SessionToggles {
            practice: false,
            ..ALL
        };
        let w = weekend(&calendar(), -DAY, follow).unwrap();
        let followed: Vec<bool> = w.sessions.iter().map(|s| s.followed).collect();
        assert_eq!(followed, [false, true, true]);
    }

    #[test]
    fn the_reply_reads_as_the_page_expects() {
        let reply = ScheduleReply {
            now: Some(10),
            weekend: weekend(&calendar()[2..3], 0, ALL),
        };
        assert_eq!(
            serde_json::to_string(&reply).unwrap(),
            r#"{"now":10,"weekend":{"country":"Singapore","circuit":"Singapore","sessions":[{"name":"Race","start":172800,"end":176400,"state":"coming","followed":true}]}}"#
        );
    }
}
