//! OpenF1 (`api.openf1.org`): the request URLs and the answers' shapes for the
//! session calendar and a race's result and team colours. The firmware makes
//! the requests; this turns the answers into [`Session`]s and a winner.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Deserializer};

use crate::{
    color::Rgb,
    schedule::{Session, SessionKind},
    weekend::SessionResult,
};

const SESSIONS_URL: &str = "https://api.openf1.org/v1/sessions";
const RESULTS_URL: &str = "https://api.openf1.org/v1/session_result";
const DRIVERS_URL: &str = "https://api.openf1.org/v1/drivers";
/// How far ahead the calendar looks: long enough to always include the next
/// race weekend, short enough to keep the response small (about 7 KB at most,
/// where a whole season is about 48 KB).
pub const CALENDAR_DAYS: i64 = 35;
/// The calendar also looks this far back, so a weekend's finished sessions
/// stay on the page until it's over.
pub const CALENDAR_PAST_DAYS: i64 = 4;
const DAY: i64 = 24 * 60 * 60;

/// The OpenF1 query for sessions starting from the UTC day `CALENDAR_PAST_DAYS`
/// before `now` (Unix seconds) until `CALENDAR_DAYS` after it. Whole days,
/// so a session that is live or just over is still included.
pub fn sessions_url(now: i64) -> String {
    let from = DateTime::from_timestamp(now - CALENDAR_PAST_DAYS * DAY, 0);
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

/// One driver of a session, with their short name and team colour.
pub fn driver_url(session_key: u32, driver: u8) -> String {
    format!("{DRIVERS_URL}?session_key={session_key}&driver_number={driver}")
}

/// `2026-10-04`. Built by hand: chrono's `format` needs its `alloc` feature.
fn date(t: DateTime<Utc>) -> String {
    format!("{:04}-{:02}-{:02}", t.year(), t.month(), t.day())
}

/// One entry of `/v1/sessions`. Every field may be missing or `null`: one
/// odd entry must not fail the whole list, so it's read with defaults and
/// then refused by [`Session::try_from`] (empty kind or time, no key).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SessionDto {
    pub session_key: Option<u32>,
    #[serde(deserialize_with = "null_as_default")]
    pub session_type: String,
    #[serde(deserialize_with = "null_as_default")]
    pub session_name: String,
    #[serde(deserialize_with = "null_as_default")]
    pub date_start: String,
    #[serde(deserialize_with = "null_as_default")]
    pub date_end: String,
    #[serde(deserialize_with = "null_as_default")]
    pub is_cancelled: bool,
    #[serde(deserialize_with = "null_as_default")]
    pub meeting_key: u32,
    #[serde(deserialize_with = "null_as_default")]
    pub country_name: String,
    #[serde(deserialize_with = "null_as_default")]
    pub circuit_short_name: String,
}

/// `#[serde(default)]` covers a missing field, not a `null` one.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, PartialEq)]
pub enum SessionError {
    NoKey,
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
    /// `"VER"`.
    #[serde(default)]
    pub name_acronym: Option<String>,
}

/// A session's P1 for the page, from their line of `/v1/drivers`. `None`
/// without a readable team colour: better to ask again than show a wrong one.
pub fn session_result(driver: &DriverDto) -> Option<SessionResult> {
    Some(SessionResult {
        driver: driver.driver_number,
        name: driver
            .name_acronym
            .clone()
            .unwrap_or_else(|| format!("#{}", driver.driver_number)),
        color: driver.team_colour.as_deref()?.parse().ok()?,
    })
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
            key: dto.session_key.ok_or(SessionError::NoKey)?,
            kind,
            start: unix_seconds(&dto.date_start)?,
            end: unix_seconds(&dto.date_end)?,
            meeting: dto.meeting_key,
            name: dto.session_name,
            country: dto.country_name,
            circuit: dto.circuit_short_name,
            cancelled: dto.is_cancelled,
        })
    }
}

/// Every session, cancelled ones included, converted and sorted by start.
/// Sessions that fail to convert are skipped, so one odd entry can't stop the
/// whole calendar from loading.
pub fn all_sessions(dtos: Vec<SessionDto>) -> Vec<Session> {
    let mut sessions: Vec<Session> = dtos
        .into_iter()
        .filter_map(|dto| Session::try_from(dto).ok())
        .collect();
    sessions.sort_by_key(|s| s.start);
    sessions
}

/// The sessions the lamp can use: not cancelled, converted, sorted by start.
pub fn usable_sessions(dtos: Vec<SessionDto>) -> Vec<Session> {
    usable(&all_sessions(dtos))
}

/// The sessions of `sessions` that aren't cancelled.
pub fn usable(sessions: &[Session]) -> Vec<Session> {
    sessions.iter().filter(|s| !s.cancelled).cloned().collect()
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
            session_key: Some(11361),
            session_type: session_type.to_string(),
            session_name: session_name.to_string(),
            date_start: "2026-09-06T13:00:00+00:00".to_string(),
            date_end: "2026-09-06T15:00:00+00:00".to_string(),
            is_cancelled: false,
            meeting_key: 1_292,
            country_name: "Italy".to_string(),
            circuit_short_name: "Monza".to_string(),
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
                meeting: 1_292,
                name: "Race".to_string(),
                country: "Italy".to_string(),
                circuit: "Monza".to_string(),
                ..Session::new(11361, SessionKind::Race, RACE_START, RACE_END)
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
    fn cancelled_sessions_are_kept_as_cancelled_for_the_page() {
        let mut cancelled = dto("Race", "Race");
        cancelled.is_cancelled = true;
        let all = all_sessions(vec![cancelled, dto("Qualifying", "Qualifying")]);
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|s| s.cancelled));
        assert_eq!(usable(&all).len(), 1);
    }

    #[test]
    fn sessions_that_fail_to_convert_are_skipped() {
        let sessions = usable_sessions(vec![dto("Shootout", "Shootout"), dto("Race", "Race")]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].kind, SessionKind::Race);
    }

    #[test]
    fn a_null_field_skips_only_its_session() {
        let json = r#"[
            {"session_key":1,"session_type":"Race","session_name":"Race",
             "date_start":"2026-09-06T13:00:00+00:00","date_end":"2026-09-06T15:00:00+00:00",
             "meeting_key":null,"country_name":null,"circuit_short_name":null,"is_cancelled":null},
            {"session_key":null,"session_type":"Race","session_name":"Race",
             "date_start":"2026-09-06T13:00:00+00:00","date_end":"2026-09-06T15:00:00+00:00"},
            {"session_key":3,"session_type":null,"session_name":"Race",
             "date_start":"2026-09-06T13:00:00+00:00","date_end":"2026-09-06T15:00:00+00:00"},
            {"session_key":4,"session_type":"Race","session_name":"Race",
             "date_start":null,"date_end":"2026-09-06T15:00:00+00:00"},
            {"session_key":5}
        ]"#;
        let dtos: Vec<SessionDto> = serde_json::from_str(json).unwrap();
        let sessions = all_sessions(dtos);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].key, 1);
        assert_eq!(sessions[0].country, "");
        assert!(!sessions[0].cancelled);
    }

    #[test]
    fn usable_sessions_are_sorted_by_start() {
        let mut later = dto("Race", "Race");
        later.session_key = Some(2);
        later.date_start = "2026-09-07T13:00:00+00:00".to_string();
        let mut earlier = dto("Qualifying", "Qualifying");
        earlier.session_key = Some(1);

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
    fn sessions_url_covers_the_last_4_and_the_next_35_days() {
        assert_eq!(
            sessions_url(OCT_4_NOON),
            "https://api.openf1.org/v1/sessions?date_start>=2026-09-30&date_start<2026-11-08"
        );
    }

    #[test]
    fn sessions_url_uses_the_utc_date() {
        // 23:59:59 UTC, already 5 October in Prague: OpenF1's times are UTC.
        assert_eq!(
            sessions_url(1_791_158_399),
            "https://api.openf1.org/v1/sessions?date_start>=2026-09-30&date_start<2026-11-08"
        );
    }

    #[test]
    fn sessions_url_window_crosses_new_year() {
        // 2026-12-21 00:00 UTC; no year filter, so January 2027 is included.
        assert_eq!(
            sessions_url(1_797_811_200),
            "https://api.openf1.org/v1/sessions?date_start>=2026-12-17&date_start<2027-01-25"
        );
    }

    #[test]
    fn sessions_url_pads_month_and_day() {
        // 2026-01-02 00:00 UTC.
        assert_eq!(
            sessions_url(1_767_312_000),
            "https://api.openf1.org/v1/sessions?date_start>=2025-12-29&date_start<2026-02-06"
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
            name_acronym: None,
        }
    }

    #[test]
    fn driver_url_asks_for_one_driver() {
        assert_eq!(
            driver_url(11388, 1),
            "https://api.openf1.org/v1/drivers?session_key=11388&driver_number=1"
        );
    }

    #[test]
    fn a_result_has_the_short_name_and_the_team_colour() {
        let line = r#"[{"driver_number":16,"name_acronym":"LEC","team_colour":"ED1131","full_name":"Charles LECLERC"}]"#;
        let drivers: Vec<DriverDto> = serde_json::from_str(line).unwrap();
        assert_eq!(
            session_result(&drivers[0]),
            Some(SessionResult {
                driver: 16,
                name: "LEC".to_string(),
                color: Rgb::new(0xED, 0x11, 0x31),
            })
        );
    }

    #[test]
    fn a_result_without_a_name_uses_the_number() {
        let result = session_result(&driver(16, Some("ED1131"))).unwrap();
        assert_eq!(result.name, "#16");
    }

    #[test]
    fn no_result_without_a_team_colour() {
        assert_eq!(session_result(&driver(16, None)), None);
        assert_eq!(session_result(&driver(16, Some("red"))), None);
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
