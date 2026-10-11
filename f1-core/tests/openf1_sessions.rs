//! Parses the saved OpenF1 sessions for the 2026 season
//! (`/v1/sessions?year=2026`, downloaded 2026-10-02).

use f1_core::{
    input::SessionPhase,
    openf1::{usable_sessions, SessionDto},
    schedule::{
        current_session, next_session, phase_at, Session, SessionKind, LIVE_GRACE, POST_SESSION,
    },
    settings::SessionToggles,
    weekend::{weekend, Results, SessionState},
};

const SESSIONS: &str = include_str!("data/sessions.json");

/// Italian GP 2026, the race.
const MONZA_RACE: u32 = 11361;

fn dtos() -> Vec<SessionDto> {
    serde_json::from_str(SESSIONS).unwrap()
}

#[test]
fn sessions_fixture_parses_every_record() {
    assert_eq!(dtos().len(), 131);
}

#[test]
fn cancelled_sessions_are_dropped_from_the_fixture() {
    let cancelled = dtos().iter().filter(|d| d.is_cancelled).count();
    assert_eq!(cancelled, 10);
    assert_eq!(usable_sessions(dtos()).len(), 121);
}

#[test]
fn every_session_in_the_fixture_converts() {
    for dto in dtos() {
        let key = dto.session_key;
        assert!(Session::try_from(dto).is_ok(), "session {key}");
    }
}

#[test]
fn italian_gp_race_has_known_start_and_end() {
    let sessions = usable_sessions(dtos());
    let race = sessions.iter().find(|s| s.key == MONZA_RACE).unwrap();
    assert_eq!(race.kind, SessionKind::Race);
    assert_eq!(race.start, 1_788_699_600); // 2026-09-06 13:00 UTC
    assert_eq!(race.end, 1_788_706_800); // 2026-09-06 15:00 UTC
}

#[test]
fn fixture_has_every_kind_of_session() {
    let sessions = usable_sessions(dtos());
    for kind in [
        SessionKind::Practice,
        SessionKind::Qualifying,
        SessionKind::SprintQualifying,
        SessionKind::Sprint,
        SessionKind::Race,
    ] {
        assert!(sessions.iter().any(|s| s.kind == kind), "{kind:?}");
    }
}

#[test]
fn usable_sessions_are_in_start_order() {
    let sessions = usable_sessions(dtos());
    assert!(sessions
        .windows(2)
        .all(|pair| pair[0].start <= pair[1].start));
}

#[test]
fn every_session_ends_after_it_starts() {
    for s in usable_sessions(dtos()) {
        assert!(s.end > s.start, "session {}", s.key);
    }
}

#[test]
fn monza_race_day_goes_through_every_phase() {
    let sessions = usable_sessions(dtos());
    let race = sessions
        .iter()
        .find(|s| s.key == MONZA_RACE)
        .unwrap()
        .clone();
    let at = |now| phase_at(&sessions, now, false);

    assert_eq!(at(race.start - 60), SessionPhase::PreSession);
    assert_eq!(at(race.start + 60), SessionPhase::Live);
    assert_eq!(at(race.end + LIVE_GRACE + 60), SessionPhase::PostSession);
    assert_eq!(
        at(race.end + LIVE_GRACE + POST_SESSION + 60),
        SessionPhase::Idle
    );
}

#[test]
fn monza_race_is_the_current_session_while_live() {
    let sessions = usable_sessions(dtos());
    let (_, current) = current_session(&sessions, 1_788_699_600 + 60, false).unwrap();
    assert_eq!(current.key, MONZA_RACE);
}

#[test]
fn after_monza_the_next_session_is_madrid_practice_1() {
    // Practice is followed too, so Friday's first practice comes first.
    let sessions = usable_sessions(dtos());
    let next = next_session(&sessions, 1_788_706_800).unwrap();
    assert_eq!(next.key, 11362);
    assert_eq!(next.kind, SessionKind::Practice);
    assert_eq!(next.start, 1_789_126_200); // 2026-09-11 11:30 UTC
}

#[test]
fn singapore_race_morning_shows_the_whole_sprint_weekend() {
    // 2026-10-11 08:00 UTC, four hours before the race.
    let w = weekend(
        &usable_sessions(dtos()),
        1_791_705_600,
        SessionToggles::default(),
        &Results::new(),
    )
    .unwrap();
    assert_eq!(
        (w.country.as_str(), w.circuit.as_str()),
        ("Singapore", "Singapore")
    );
    let names: Vec<(&str, SessionState)> = w
        .sessions
        .iter()
        .map(|s| (s.name.as_str(), s.state))
        .collect();
    assert_eq!(
        names,
        [
            ("Practice 1", SessionState::Done),
            ("Sprint Qualifying", SessionState::Done),
            ("Sprint", SessionState::Done),
            ("Qualifying", SessionState::Done),
            ("Race", SessionState::Coming),
        ]
    );
}
