use chrono::DateTime;
use serde::Deserialize;

use crate::schedule::{Session, SessionKind};

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
}
