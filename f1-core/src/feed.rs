use crate::color::Rgb;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum SessionState {
    Inactive,
    Started,
    Aborted,
    Finished,
    Finalised,
    Ends,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionStatus {
    pub status: Option<SessionState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackCode {
    AllClear,
    Yellow,
    ScDeployed,
    Red,
    VscDeployed,
    VscEnding,
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TrackStatus {
    pub status: Option<String>,
    pub message: Option<String>,
}

impl TrackStatus {
    pub fn code(&self) -> Option<TrackCode> {
        self.status.as_deref().map(|s| match s {
            "1" => TrackCode::AllClear,
            "2" => TrackCode::Yellow,
            "4" => TrackCode::ScDeployed,
            "5" => TrackCode::Red,
            "6" => TrackCode::VscDeployed,
            "7" => TrackCode::VscEnding,
            _ => TrackCode::Unknown,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DriverPatch {
    pub team_colour: Option<String>,
    pub tla: Option<String>,
}

// TODO(probe): the live feed may add `"_kf": true`, which a u8 key rejects.
pub type DriverList = HashMap<u8, DriverPatch>;

pub fn update_team_colours(colours: &mut HashMap<u8, Rgb>, patch: &DriverList) {
    for (number, driver) in patch {
        if let Some(rgb) = driver.team_colour.as_deref().and_then(|c| c.parse().ok()) {
            colours.insert(*number, rgb);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RaceControl {
    pub messages: Option<Messages>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Messages {
    List(Vec<RcMessage>),
    Map(HashMap<String, RcMessage>),
}

impl Messages {
    pub fn into_vec(self) -> Vec<RcMessage> {
        match self {
            Messages::List(v) => v,
            Messages::Map(m) => {
                let mut indexed: Vec<(u32, RcMessage)> = m
                    .into_iter()
                    .map(|(key, msg)| (key.parse().unwrap_or(u32::MAX), msg))
                    .collect();
                indexed.sort_by_key(|(index, _)| *index);
                indexed.into_iter().map(|(_, msg)| msg).collect()
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RcMessage {
    pub utc: Option<String>,
    pub lap: Option<u32>,
    pub category: Option<String>,
    pub flag: Option<String>,
    pub scope: Option<String>,
    pub sector: Option<u8>,
    pub racing_number: Option<String>, // a string in the JSON: "77"
    pub status: Option<String>,
    pub mode: Option<String>,
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(json: &str) -> Option<SessionState> {
        serde_json::from_str::<SessionStatus>(json).unwrap().status
    }

    fn track(json: &str) -> Option<TrackCode> {
        serde_json::from_str::<TrackStatus>(json).unwrap().code()
    }

    fn messages(json: &str) -> Vec<RcMessage> {
        serde_json::from_str::<RaceControl>(json)
            .unwrap()
            .messages
            .unwrap()
            .into_vec()
    }

    fn drivers(json: &str) -> DriverList {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn session_status_reads_every_known_state() {
        let cases = [
            ("Inactive", SessionState::Inactive),
            ("Started", SessionState::Started),
            ("Aborted", SessionState::Aborted),
            ("Finished", SessionState::Finished),
            ("Finalised", SessionState::Finalised),
            ("Ends", SessionState::Ends),
        ];
        for (text, state) in cases {
            let json = format!(r#"{{"Status":"{text}","Started":"Started"}}"#);
            assert_eq!(session(&json), Some(state), "{text}");
        }
    }

    #[test]
    fn session_status_unknown_value_parses_as_unknown() {
        assert_eq!(
            session(r#"{"Status":"Banana"}"#),
            Some(SessionState::Unknown)
        );
    }

    #[test]
    fn session_status_without_status_is_none() {
        assert_eq!(session(r#"{"Started":"Started"}"#), None);
    }

    #[test]
    fn track_status_codes_map_to_variants() {
        let cases = [
            ("1", TrackCode::AllClear),
            ("2", TrackCode::Yellow),
            ("4", TrackCode::ScDeployed),
            ("5", TrackCode::Red),
            ("6", TrackCode::VscDeployed),
            ("7", TrackCode::VscEnding),
        ];
        for (code, expected) in cases {
            let json = format!(r#"{{"Status":"{code}","Message":"x"}}"#);
            assert_eq!(track(&json), Some(expected), "code {code}");
        }
    }

    #[test]
    fn track_status_code_3_is_unknown() {
        assert_eq!(track(r#"{"Status":"3"}"#), Some(TrackCode::Unknown));
    }

    #[test]
    fn track_status_code_is_read_from_status_not_message() {
        assert_eq!(
            track(r#"{"Status":"5","Message":"AllClear"}"#),
            Some(TrackCode::Red)
        );
    }

    #[test]
    fn track_status_without_status_has_no_code() {
        assert_eq!(track(r#"{"Message":"Yellow"}"#), None);
    }

    #[test]
    fn race_control_reads_messages_as_list() {
        let m = messages(r#"{"Messages":[{"Category":"Flag","Flag":"GREEN","Lap":1}]}"#);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].category.as_deref(), Some("Flag"));
        assert_eq!(m[0].flag.as_deref(), Some("GREEN"));
        assert_eq!(m[0].lap, Some(1));
    }

    #[test]
    fn race_control_reads_messages_as_indexed_object() {
        let m = messages(
            r#"{"Messages":{"34":{"Category":"SafetyCar","Status":"DEPLOYED","Mode":"SAFETY CAR"}}}"#,
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].category.as_deref(), Some("SafetyCar"));
        assert_eq!(m[0].status.as_deref(), Some("DEPLOYED"));
        assert_eq!(m[0].mode.as_deref(), Some("SAFETY CAR"));
    }

    #[test]
    fn race_control_index_object_keeps_numeric_order() {
        let m = messages(r#"{"Messages":{"10":{"Lap":10},"9":{"Lap":9}}}"#);
        let laps: Vec<_> = m.iter().map(|m| m.lap).collect();
        assert_eq!(laps, [Some(9), Some(10)]);
    }

    #[test]
    fn race_control_message_without_flag_has_none() {
        let m = messages(r#"{"Messages":{"2":{"Category":"Other","Message":"INCIDENT"}}}"#);
        assert_eq!(m[0].flag, None);
        assert_eq!(m[0].message.as_deref(), Some("INCIDENT"));
    }

    #[test]
    fn race_control_reads_driver_and_sector() {
        let m = messages(
            r#"{"Messages":{"5":{"Flag":"BLUE","Scope":"Driver","RacingNumber":"77","Sector":3}}}"#,
        );
        assert_eq!(m[0].racing_number.as_deref(), Some("77"));
        assert_eq!(m[0].sector, Some(3));
        assert_eq!(m[0].scope.as_deref(), Some("Driver"));
    }

    #[test]
    fn race_control_message_text_may_contain_escapes() {
        let m = messages(r#"{"Messages":[{"Message":"CAR 11 \"PER\" – NOTED"}]}"#);
        assert_eq!(m[0].message.as_deref(), Some("CAR 11 \"PER\" – NOTED"));
    }

    #[test]
    fn race_control_without_messages_is_none() {
        let rc: RaceControl = serde_json::from_str("{}").unwrap();
        assert!(rc.messages.is_none());
    }

    #[test]
    fn driver_list_keys_become_driver_numbers() {
        let d = drivers(r#"{"12":{"Tla":"ANT","TeamColour":"00D7B6"}}"#);
        assert_eq!(d[&12].tla.as_deref(), Some("ANT"));
    }

    #[test]
    fn team_colours_are_read_from_hex_without_hash() {
        let mut colours = HashMap::new();
        update_team_colours(&mut colours, &drivers(r#"{"12":{"TeamColour":"00D7B6"}}"#));
        assert_eq!(colours[&12], Rgb::new(0x00, 0xd7, 0xb6));
    }

    #[test]
    fn patch_without_colour_keeps_existing_colour() {
        let mut colours = HashMap::from([(44, Rgb::RED)]);
        update_team_colours(&mut colours, &drivers(r#"{"44":{"Line":5}}"#));
        assert_eq!(colours[&44], Rgb::RED);
    }

    #[test]
    fn new_colour_replaces_old_one() {
        let mut colours = HashMap::from([(44, Rgb::RED)]);
        update_team_colours(&mut colours, &drivers(r#"{"44":{"TeamColour":"0000FF"}}"#));
        assert_eq!(colours[&44], Rgb::BLUE);
    }

    #[test]
    fn driver_with_unreadable_colour_is_skipped() {
        let mut colours = HashMap::new();
        update_team_colours(
            &mut colours,
            &drivers(r#"{"1":{"TeamColour":"zzz"},"2":{"TeamColour":"FF0000"}}"#),
        );
        assert!(!colours.contains_key(&1));
        assert_eq!(colours[&2], Rgb::RED);
    }
}
