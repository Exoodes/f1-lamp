//! Parses every line of the archived 2026 Italian GP race streams into the
//! typed `feed` messages.

use std::{collections::HashMap, time::Duration};

use f1_core::{
    color::Rgb,
    feed::{
        update_team_colours, DriverList, RaceControl, RcMessage, SessionState, SessionStatus,
        TrackCode, TrackStatus,
    },
    stream::parse_line,
};
use serde::de::DeserializeOwned;

const TRACK_STATUS: &str = include_str!("data/2026-italy/race/TrackStatus.jsonStream");
const SESSION_STATUS: &str = include_str!("data/2026-italy/race/SessionStatus.jsonStream");
const RACE_CONTROL: &str = include_str!("data/2026-italy/race/RaceControlMessages.jsonStream");
const DRIVER_LIST: &str = include_str!("data/2026-italy/race/DriverList.jsonStream");

/// Every line of a file as offset + typed message. Panics with the line number.
fn parse_all<T: DeserializeOwned>(file: &str) -> Vec<(Duration, T)> {
    file.lines()
        .enumerate()
        .map(|(i, raw)| {
            let line = parse_line(raw).unwrap_or_else(|e| panic!("line {}: {e}", i + 1));
            let msg = serde_json::from_str(line.json)
                .unwrap_or_else(|e| panic!("line {}: {e}\n{}", i + 1, line.json));
            (line.offset, msg)
        })
        .collect()
}

fn hms(h: u64, m: u64, s: u64, ms: u64) -> Duration {
    Duration::from_millis(((h * 60 + m) * 60 + s) * 1000 + ms)
}

fn race_control_messages() -> Vec<(Duration, RcMessage)> {
    parse_all::<RaceControl>(RACE_CONTROL)
        .into_iter()
        .flat_map(|(offset, rc)| {
            let messages = rc.messages.expect("every line has Messages").into_vec();
            messages.into_iter().map(move |m| (offset, m))
        })
        .collect()
}

fn colours_after_whole_race() -> HashMap<u8, Rgb> {
    let mut colours = HashMap::new();
    for (_, patch) in parse_all::<DriverList>(DRIVER_LIST) {
        update_team_colours(&mut colours, &patch);
    }
    colours
}

#[test]
fn track_status_gives_the_race_sequence_of_codes() {
    use TrackCode::*;
    let codes: Vec<TrackCode> = parse_all::<TrackStatus>(TRACK_STATUS)
        .into_iter()
        .map(|(_, t)| t.code().expect("every line has a Status"))
        .collect();
    assert_eq!(
        codes,
        [
            Yellow,
            AllClear,
            Yellow,
            AllClear,
            Yellow,
            ScDeployed,
            Red,
            AllClear,
            Yellow,
            AllClear,
            Yellow,
            AllClear,
            Yellow,
            VscDeployed,
            VscEnding,
            AllClear,
        ]
    );
}

#[test]
fn track_status_codes_agree_with_their_messages() {
    for (offset, t) in parse_all::<TrackStatus>(TRACK_STATUS) {
        let expected = match t.message.as_deref() {
            Some("AllClear") => TrackCode::AllClear,
            Some("Yellow") => TrackCode::Yellow,
            Some("SCDeployed") => TrackCode::ScDeployed,
            Some("Red") => TrackCode::Red,
            Some("VSCDeployed") => TrackCode::VscDeployed,
            Some("VSCEnding") => TrackCode::VscEnding,
            other => panic!("{offset:?}: unexpected message {other:?}"),
        };
        assert_eq!(t.code(), Some(expected), "{offset:?}");
    }
}

#[test]
fn red_flag_starts_at_1_01_07() {
    let red = parse_all::<TrackStatus>(TRACK_STATUS)
        .into_iter()
        .find(|(_, t)| t.code() == Some(TrackCode::Red))
        .unwrap();
    assert_eq!(red.0, hms(1, 1, 7, 837));
}

#[test]
fn session_status_gives_the_race_sequence_of_states() {
    use SessionState::*;
    let states: Vec<SessionState> = parse_all::<SessionStatus>(SESSION_STATUS)
        .into_iter()
        .map(|(_, s)| s.status.expect("every line has a Status"))
        .collect();
    assert_eq!(
        states,
        [Inactive, Started, Aborted, Started, Finished, Finalised, Ends]
    );
}

#[test]
fn session_finishes_at_2_48_11() {
    let finished = parse_all::<SessionStatus>(SESSION_STATUS)
        .into_iter()
        .find(|(_, s)| s.status == Some(SessionState::Finished))
        .unwrap();
    assert_eq!(finished.0, hms(2, 48, 11, 236));
}

#[test]
fn every_race_control_line_parses_into_154_messages() {
    assert_eq!(race_control_messages().len(), 154);
}

#[test]
fn race_control_categories_are_flag_safety_car_or_other() {
    for (offset, m) in race_control_messages() {
        let category = m.category.as_deref();
        assert!(
            matches!(category, Some("Flag" | "SafetyCar" | "Other")),
            "{offset:?}: {category:?}"
        );
    }
}

#[test]
fn race_control_has_one_chequered_flag_at_2_48_10() {
    let chequered: Vec<Duration> = race_control_messages()
        .into_iter()
        .filter(|(_, m)| m.flag.as_deref() == Some("CHEQUERED"))
        .map(|(offset, _)| offset)
        .collect();
    assert_eq!(chequered, [hms(2, 48, 10, 829)]);
}

#[test]
fn race_control_reports_safety_car_then_vsc_deployed_and_ending() {
    // Kept in a variable so the `&str`s below can borrow from it.
    let messages = race_control_messages();
    let safety_car: Vec<(Option<&str>, Option<&str>)> = messages
        .iter()
        .filter(|(_, m)| m.category.as_deref() == Some("SafetyCar"))
        .map(|(_, m)| (m.mode.as_deref(), m.status.as_deref()))
        .collect();
    assert_eq!(
        safety_car,
        [
            (Some("SAFETY CAR"), Some("DEPLOYED")),
            (Some("VSC"), Some("DEPLOYED")),
            (Some("VSC"), Some("ENDING")),
        ]
    );
}

#[test]
fn every_driver_list_line_parses() {
    assert_eq!(parse_all::<DriverList>(DRIVER_LIST).len(), 149);
}

#[test]
fn driver_list_gives_22_team_colours() {
    assert_eq!(colours_after_whole_race().len(), 22);
}

#[test]
fn eleven_teams_share_the_22_colours() {
    let mut distinct: Vec<Rgb> = colours_after_whole_race().into_values().collect();
    distinct.sort_by_key(|c| (c.r, c.g, c.b));
    distinct.dedup();
    assert_eq!(distinct.len(), 11);
}

#[test]
fn antonelli_is_mercedes_teal() {
    assert_eq!(colours_after_whole_race()[&12], "#00d7b6".parse().unwrap());
}

#[test]
fn piastri_is_mclaren_papaya() {
    assert_eq!(colours_after_whole_race()[&81], "#f47600".parse().unwrap());
}
