//! OpenF1 (`api.openf1.org`): the request URLs and the answers' shapes for the
//! session calendar and a race's result and team colours. The firmware makes
//! the requests; this turns the answers into [`Session`]s and a winner.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Utc};
use serde::Deserialize;

use crate::{
    color::Rgb,
    schedule::{Session, SessionKind},
};

const SESSIONS_URL: &str = "https://api.openf1.org/v1/sessions";
const RESULTS_URL: &str = "https://api.openf1.org/v1/session_result";
const DRIVERS_URL: &str = "https://api.openf1.org/v1/drivers";
/// How far ahead the calendar looks: long enough to always include the next
/// race weekend, short enough to keep the response small (about 7 KB at most,
/// where a whole season is about 48 KB).
pub const CALENDAR_DAYS: i64 = 35;
const DAY: i64 = 24 * 60 * 60;

/// The OpenF1 query for sessions starting from the UTC day of `now` (Unix
/// seconds) until `CALENDAR_DAYS` later. Whole days, so a session that is
/// live or just over today is still included.
pub fn sessions_url(now: i64) -> String {
    let from = DateTime::from_timestamp(now, 0);
    let to = DateTime::from_timestamp(now + CALENDAR_DAYS * DAY, 0);
    match (from, to) {
        (Some(from), Some(to)) => format!(
            "{SESSIONS_URL}?date_start>={}&date_start<{}",
            date(from),
            date(to)
        ),
        // Only for absurd times; asking for everything is the safe fallback.
        _ => SESSIONS_URL.to_string(),
    }
}

/// The winner of a session: only position 1, so the response stays tiny.
pub fn results_url(session_key: u32) -> String {
    format!("{RESULTS_URL}?session_key={session_key}&position=1")
}

/// Every driver in a session, with their team colour (about 8 KB).
pub fn drivers_url(session_key: u32) -> String {
    format!("{DRIVERS_URL}?session_key={session_key}")
}

/// `2026-10-04`. Built by hand: chrono's `format` needs its `alloc` feature.
fn date(t: DateTime<Utc>) -> String {
    format!("{:04}-{:02}-{:02}", t.year(), t.month(), t.day())
}

#[derive(Debug, Deserialize)]
pub struct SessionDto {
    pub session_key: u32,
    pub session_type: String,
    pub session_name: String,
    pub date_start: String,
    pub date_end: String,
    #[serde(default)]
    pub is_cancelled: bool,
}

#[derive(Debug, PartialEq)]
pub enum SessionError {
    BadTime(String),
    UnknownKind(String),
}

/// One line of `/v1/session_result`.
#[derive(Debug, Deserialize)]
pub struct ResultDto {
    /// `null` for drivers who weren't classified (retired early).
    pub position: Option<u8>,
    pub driver_number: u8,
}

/// One line of `/v1/drivers`.
#[derive(Debug, Deserialize)]
pub struct DriverDto {
    pub driver_number: u8,
    /// Hex without `#`, e.g. `"00D7B6"`.
    pub team_colour: Option<String>,
}

/// The driver number of whoever finished first.
pub fn winner(results: &[ResultDto]) -> Option<u8> {
    results
        .iter()
        .find(|r| r.position == Some(1))
        .map(|r| r.driver_number)
}

/// Driver number → team colour. Drivers with a missing or unreadable colour
/// are skipped.
pub fn team_colours(drivers: Vec<DriverDto>) -> HashMap<u8, Rgb> {
    drivers
        .into_iter()
        // `?` inside the closure: a `None` anywhere skips this driver.
        .filter_map(|d| Some((d.driver_number, d.team_colour?.parse().ok()?)))
        .collect()
}

impl TryFrom<SessionDto> for Session {
    type Error = SessionError;
    fn try_from(dto: SessionDto) -> Result<Self, Self::Error> {
        let kind = match (dto.session_type.as_str(), dto.session_name.as_str()) {
            ("Race", "Race") => SessionKind::Race,
            ("Race", "Sprint") => SessionKind::Sprint,
            ("Qualifying", "Qualifying") => SessionKind::Qualifying,
            ("Qualifying", "Sprint Qualifying") => SessionKind::SprintQualifying,
            ("Practice", _) => SessionKind::Practice,
            _ => {
                return Err(SessionError::UnknownKind(format!(
                    "{} / {}",
                    dto.session_type, dto.session_name
                )))
            }
        };

        Ok(Session {
            key: dto.session_key,
            kind,
            start: unix_seconds(&dto.date_start)?,
            end: unix_seconds(&dto.date_end)?,
        })
    }
}

/// The sessions the lamp can use: not cancelled, converted, sorted by start.
/// Sessions that fail to convert are skipped, so one odd entry can't stop the
/// whole calendar from loading.
pub fn usable_sessions(dtos: Vec<SessionDto>) -> Vec<Session> {
    let mut sessions: Vec<Session> = dtos
        .into_iter()
        .filter(|dto| !dto.is_cancelled)
        .filter_map(|dto| Session::try_from(dto).ok())
        .collect();
    sessions.sort_by_key(|s| s.start);
    sessions
}

/// An RFC 3339 timestamp such as `2026-09-06T13:00:00+00:00` as Unix seconds.
fn unix_seconds(text: &str) -> Result<i64, SessionError> {
    Ok(DateTime::parse_from_rfc3339(text)
        .map_err(|_| SessionError::BadTime(text.to_string()))?
        .timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-06 13:00 and 15:00 UTC.
    const RACE_START: i64 = 1_788_699_600;
    const RACE_END: i64 = 1_788_706_800;

    fn dto(session_type: &str, session_name: &str) -> SessionDto {
        SessionDto {
            session_key: 11361,
            session_type: session_type.to_string(),
            session_name: session_name.to_string(),
            date_start: "2026-09-06T13:00:00+00:00".to_string(),
            date_end: "2026-09-06T15:00:00+00:00".to_string(),
            is_cancelled: false,
        }
    }

    fn kind_of(session_type: &str, session_name: &str) -> SessionKind {
        Session::try_from(dto(session_type, session_name))
            .unwrap()
            .kind
    }

    #[test]
    fn race_converts_with_unix_times() {
        assert_eq!(
            Session::try_from(dto("Race", "Race")),
            Ok(Session {
                key: 11361,
                kind: SessionKind::Race,
                start: RACE_START,
                end: RACE_END,
            })
        );
    }

    #[test]
    fn sprint_is_told_apart_from_race() {
        assert_eq!(kind_of("Race", "Sprint"), SessionKind::Sprint);
    }

    #[test]
    fn qualifying_converts() {
        assert_eq!(kind_of("Qualifying", "Qualifying"), SessionKind::Qualifying);
    }

    #[test]
    fn sprint_qualifying_is_told_apart_from_qualifying() {
        assert_eq!(
            kind_of("Qualifying", "Sprint Qualifying"),
            SessionKind::SprintQualifying
        );
    }

    #[test]
    fn every_practice_session_counts_as_practice() {
        for name in ["Practice 1", "Practice 2", "Practice 3", "Day 1"] {
            assert_eq!(kind_of("Practice", name), SessionKind::Practice, "{name}");
        }
    }

    #[test]
    fn unknown_session_type_is_an_error() {
        assert_eq!(
            Session::try_from(dto("Shootout", "Shootout")),
            Err(SessionError::UnknownKind("Shootout / Shootout".to_string()))
        );
    }

    #[test]
    fn unknown_race_name_is_an_error() {
        assert!(Session::try_from(dto("Race", "Feature Race")).is_err());
    }

    #[test]
    fn bad_start_time_is_an_error() {
        let mut d = dto("Race", "Race");
        d.date_start = "6 September 2026".to_string();
        assert_eq!(
            Session::try_from(d),
            Err(SessionError::BadTime("6 September 2026".to_string()))
        );
    }

    #[test]
    fn bad_end_time_is_an_error() {
        let mut d = dto("Race", "Race");
        d.date_end = String::new();
        assert_eq!(
            Session::try_from(d),
            Err(SessionError::BadTime(String::new()))
        );
    }

    #[test]
    fn non_utc_offset_is_converted_to_utc() {
        let mut d = dto("Race", "Race");
        d.date_start = "2026-09-06T15:00:00+02:00".to_string();
        assert_eq!(Session::try_from(d).unwrap().start, RACE_START);
    }

    #[test]
    fn cancelled_sessions_are_not_usable() {
        let mut cancelled = dto("Race", "Race");
        cancelled.is_cancelled = true;
        assert!(usable_sessions(vec![cancelled]).is_empty());
    }

    #[test]
    fn sessions_that_fail_to_convert_are_skipped() {
        let sessions = usable_sessions(vec![dto("Shootout", "Shootout"), dto("Race", "Race")]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].kind, SessionKind::Race);
    }

    #[test]
    fn usable_sessions_are_sorted_by_start() {
        let mut later = dto("Race", "Race");
        later.session_key = 2;
        later.date_start = "2026-09-07T13:00:00+00:00".to_string();
        let mut earlier = dto("Qualifying", "Qualifying");
        earlier.session_key = 1;

        let keys: Vec<u32> = usable_sessions(vec![later, earlier])
            .iter()
            .map(|s| s.key)
            .collect();
        assert_eq!(keys, [1, 2]);
    }

    #[test]
    fn missing_is_cancelled_means_not_cancelled() {
        let json = r#"{"session_key":1,"session_type":"Race","session_name":"Race",
            "date_start":"2026-09-06T13:00:00+00:00","date_end":"2026-09-06T15:00:00+00:00"}"#;
        let d: SessionDto = serde_json::from_str(json).unwrap();
        assert!(!d.is_cancelled);
    }

    // ---- sessions_url ----

    /// 2026-10-04 12:00 UTC.
    const OCT_4_NOON: i64 = 1_791_115_200;

    #[test]
    fn sessions_url_covers_the_next_35_days() {
        assert_eq!(
            sessions_url(OCT_4_NOON),
            "https://api.openf1.org/v1/sessions?date_start>=2026-10-04&date_start<2026-11-08"
        );
    }

    #[test]
    fn sessions_url_uses_the_utc_date() {
        // 23:59:59 UTC, already 5 October in Prague: OpenF1's times are UTC.
        assert_eq!(
            sessions_url(1_791_158_399),
            "https://api.openf1.org/v1/sessions?date_start>=2026-10-04&date_start<2026-11-08"
        );
    }

    #[test]
    fn sessions_url_window_crosses_new_year() {
        // 2026-12-21 00:00 UTC; no year filter, so January 2027 is included.
        assert_eq!(
            sessions_url(1_797_811_200),
            "https://api.openf1.org/v1/sessions?date_start>=2026-12-21&date_start<2027-01-25"
        );
    }

    #[test]
    fn sessions_url_pads_month_and_day() {
        // 2026-01-02 00:00 UTC.
        assert_eq!(
            sessions_url(1_767_312_000),
            "https://api.openf1.org/v1/sessions?date_start>=2026-01-02&date_start<2026-02-06"
        );
    }

    #[test]
    fn sessions_url_falls_back_to_no_filter_for_impossible_times() {
        assert_eq!(sessions_url(i64::MAX / 2), SESSIONS_URL);
    }

    // ---- Results and drivers ----

    fn result(position: Option<u8>, driver_number: u8) -> ResultDto {
        ResultDto {
            position,
            driver_number,
        }
    }

    fn driver(driver_number: u8, team_colour: Option<&str>) -> DriverDto {
        DriverDto {
            driver_number,
            team_colour: team_colour.map(str::to_string),
        }
    }

    #[test]
    fn results_url_asks_only_for_the_winner() {
        assert_eq!(
            results_url(11361),
            "https://api.openf1.org/v1/session_result?session_key=11361&position=1"
        );
    }

    #[test]
    fn drivers_url_asks_for_one_session() {
        assert_eq!(
            drivers_url(11361),
            "https://api.openf1.org/v1/drivers?session_key=11361"
        );
    }

    #[test]
    fn winner_is_the_driver_in_position_one() {
        let results = [result(Some(2), 4), result(Some(1), 12), result(Some(3), 63)];
        assert_eq!(winner(&results), Some(12));
    }

    #[test]
    fn no_winner_without_position_one() {
        assert_eq!(winner(&[result(Some(2), 4)]), None);
        assert_eq!(winner(&[]), None);
    }

    #[test]
    fn unclassified_drivers_are_not_winners() {
        assert_eq!(winner(&[result(None, 18)]), None);
    }

    #[test]
    fn null_position_parses() {
        let r: ResultDto = serde_json::from_str(r#"{"position":null,"driver_number":18}"#).unwrap();
        assert_eq!(r.position, None);
    }

    #[test]
    fn team_colour_without_hash_parses() {
        let colours = team_colours(vec![driver(12, Some("00D7B6"))]);
        assert_eq!(colours.get(&12), Some(&Rgb::new(0x00, 0xD7, 0xB6)));
    }

    #[test]
    fn drivers_with_bad_or_missing_colour_are_skipped() {
        let colours = team_colours(vec![
            driver(1, Some("F47600")),
            driver(2, None),
            driver(3, Some("not a colour")),
        ]);
        assert_eq!(colours.len(), 1);
        assert!(colours.contains_key(&1));
    }

    #[test]
    fn a_repeated_driver_keeps_one_colour() {
        // OpenF1 has been seen to return the same driver twice.
        let colours = team_colours(vec![driver(1, Some("F47600")), driver(1, Some("F47600"))]);
        assert_eq!(colours.len(), 1);
    }
}
