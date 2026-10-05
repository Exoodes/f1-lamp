use crate::color::Rgb;
use serde::{
    de::{IgnoredAny, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    marker::PhantomData,
    ops::Deref,
    str::FromStr,
};

#[derive(Debug)]
pub struct Numbered<K, V>(pub BTreeMap<K, V>);

impl<K, V> Deref for Numbered<K, V> {
    type Target = BTreeMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de, K, V> Deserialize<'de> for Numbered<K, V>
where
    K: FromStr + Ord,
    V: Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NumberedVisitor<K, V>(PhantomData<(K, V)>);

        impl<'de, K, V> Visitor<'de> for NumberedVisitor<K, V>
        where
            K: FromStr + Ord,
            V: Deserialize<'de>,
        {
            type Value = Numbered<K, V>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object keyed by numbers")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    match key.parse() {
                        Ok(number) => {
                            entries.insert(number, map.next_value()?);
                        }
                        Err(_) => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(Numbered(entries))
            }
        }

        deserializer.deserialize_map(NumberedVisitor(PhantomData))
    }
}

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

pub type DriverList = Numbered<u8, DriverPatch>;

pub fn update_team_colours(colours: &mut HashMap<u8, Rgb>, patch: &DriverList) {
    for (number, driver) in patch.iter() {
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
    Map(Numbered<u32, RcMessage>),
}

impl Messages {
    pub fn into_vec(self) -> Vec<RcMessage> {
        match self {
            Messages::List(v) => v,
            Messages::Map(m) => m.0.into_values().collect(),
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

#[derive(Debug, Deserialize)]
pub struct SessionInfo {
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "Key")]
    pub key: Option<u32>,
}

impl SessionInfo {
    pub fn is_race(&self) -> Option<bool> {
        self.kind.as_deref().map(|k| k == "Race")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TopThree {
    pub lines: Option<TopLines>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TopLines {
    List(Vec<TopLine>),
    Map(Numbered<usize, TopLine>),
}

impl TopLines {
    pub fn indexed(&self) -> Vec<(usize, &TopLine)> {
        match self {
            TopLines::List(v) => v.iter().enumerate().collect(),
            TopLines::Map(m) => m.iter().map(|(i, line)| (*i, line)).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TopLine {
    pub racing_number: Option<String>,
    pub team_colour: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PitLaneTimes {
    pub pit_times: Option<Numbered<u8, PitLaneTime>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PitLaneTime {
    pub duration: Option<String>,
    pub lap: Option<String>,
}

impl PitLaneTime {
    pub fn seconds(&self) -> Option<f32> {
        self.duration.as_deref()?.parse().ok()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TimingStats {
    pub lines: Option<Numbered<u8, StatsLine>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StatsLine {
    pub personal_best_lap_time: Option<BestLap>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BestLap {
    pub value: Option<String>,
    pub position: Option<u32>,
    pub lap: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OvertakeSeries {
    pub overtakes: Option<Numbered<u8, Overtakes>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Overtakes {
    List(Vec<OvertakeEntry>),
    Map(Numbered<u32, OvertakeEntry>),
}

impl Overtakes {
    pub fn len(&self) -> usize {
        match self {
            Overtakes::List(v) => v.len(),
            Overtakes::Map(m) => m.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OvertakeEntry {
    pub timestamp: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_info_type_race_is_a_race() {
        let info: SessionInfo =
            serde_json::from_str(r#"{"Type":"Race","Name":"Sprint","Key":1}"#).unwrap();
        assert_eq!(info.is_race(), Some(true));
        assert_eq!(info.name.as_deref(), Some("Sprint"));
    }

    #[test]
    fn session_info_qualifying_is_not_a_race() {
        let info: SessionInfo = serde_json::from_str(r#"{"Type":"Qualifying"}"#).unwrap();
        assert_eq!(info.is_race(), Some(false));
    }

    #[test]
    fn top_three_reads_lines_as_list() {
        let t: TopThree = serde_json::from_str(
            r#"{"Withheld":false,"Lines":[{"RacingNumber":"10","TeamColour":"00A1E8"},{"RacingNumber":"63"}]}"#,
        )
        .unwrap();
        let lines = t.lines.unwrap();
        let lines = lines.indexed();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0, 0);
        assert_eq!(lines[0].1.racing_number.as_deref(), Some("10"));
        assert_eq!(lines[1].1.team_colour, None);
    }

    #[test]
    fn top_three_reads_lines_as_indexed_object() {
        let t: TopThree =
            serde_json::from_str(r#"{"Lines":{"0":{"RacingNumber":"12"},"1":{"LapTime":"1:25"}}}"#)
                .unwrap();
        let lines = t.lines.unwrap();
        let mut lines = lines.indexed();
        lines.sort_by_key(|(i, _)| *i);
        assert_eq!(lines[0].1.racing_number.as_deref(), Some("12"));
        assert_eq!(lines[1].1.racing_number, None);
    }

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

    #[test]
    fn driver_list_skips_the_live_feeds_kf_marker() {
        let d = drivers(r#"{"12":{"TeamColour":"00D7B6"},"_kf":true}"#);
        assert_eq!(d.len(), 1);
        assert_eq!(d[&12].team_colour.as_deref(), Some("00D7B6"));
    }

    #[test]
    fn race_control_object_skips_non_numeric_keys() {
        let m = messages(r#"{"Messages":{"_deleted":[3],"4":{"Lap":4}},"_kf":true}"#);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].lap, Some(4));
    }

    #[test]
    fn top_three_object_skips_non_numeric_keys() {
        let t: TopThree =
            serde_json::from_str(r#"{"Lines":{"0":{"RacingNumber":"12"},"_kf":true}}"#).unwrap();
        assert_eq!(t.lines.unwrap().indexed().len(), 1);
    }

    #[test]
    fn numbered_map_rejects_something_that_is_not_an_object() {
        assert!(serde_json::from_str::<DriverList>("[1,2]").is_err());
    }

    #[test]
    fn pit_lane_time_reads_driver_and_seconds_and_skips_deleted() {
        let p: PitLaneTimes = serde_json::from_str(
            r#"{"PitTimes":{"27":{"RacingNumber":"27","Duration":"24.2","Lap":"27"},"_deleted":["55"]}}"#,
        )
        .unwrap();
        let times = p.pit_times.unwrap();
        assert_eq!(times.len(), 1);
        assert_eq!(times[&27].seconds(), Some(24.2));
    }

    #[test]
    fn timing_stats_reads_personal_best_position_and_lap() {
        let t: TimingStats = serde_json::from_str(
            r#"{"Lines":{"12":{"PersonalBestLapTime":{"Lap":13,"Position":1,"Value":"1:25.469"}},"3":{"PersonalBestLapTime":{"Position":2}}}}"#,
        )
        .unwrap();
        let lines = t.lines.unwrap();
        let best = lines[&12].personal_best_lap_time.as_ref().unwrap();
        assert_eq!((best.position, best.lap), (Some(1), Some(13)));
        assert_eq!(best.value.as_deref(), Some("1:25.469"));
        assert_eq!(
            lines[&3].personal_best_lap_time.as_ref().unwrap().value,
            None
        );
    }

    #[test]
    fn overtake_series_reads_both_shapes() {
        let first: OvertakeSeries = serde_json::from_str(
            r#"{"Overtakes":{"81":[{"Timestamp":"2026-09-06T13:03:40.488Z","count":1}]}}"#,
        )
        .unwrap();
        assert_eq!(first.overtakes.unwrap()[&81].len(), 1);
        let update: OvertakeSeries = serde_json::from_str(
            r#"{"Overtakes":{"44":{"5":{"Timestamp":"2026-09-06T13:45:52.666Z","count":3}}}}"#,
        )
        .unwrap();
        assert_eq!(update.overtakes.unwrap()[&44].len(), 1);
    }
}
