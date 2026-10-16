//! The JSON of F1's live timing streams as typed structs: only the fields the
//! lamp uses, all optional, because an update carries only what changed. The
//! live feed and the archive use the same ones. [`Stream`] names each stream
//! and parses its JSON into a [`FeedMessage`].

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

/// A JSON object whose keys are numbers, read as a map sorted by them. The
/// feed sends lists this way so that an update can patch single items:
/// `{"Messages":{"40":{...}}}` adds or changes message 40 only. Driver lists
/// are keyed by car number (`{"81":{...}}`). Keys that aren't numbers are
/// skipped, such as the `_kf` marker in the live feed's first answer.
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
    /// Whether the session has begun: `Inactive` before the first start,
    /// then `Started` or `Finished`, also while `status` is `Inactive`
    /// between qualifying's parts.
    #[serde(default)]
    pub started: Option<SessionState>,
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
    /// What `Status` says; `Message` repeats it in words ("SCDeployed") and
    /// isn't read. The codes: 1 all clear, 2 yellow, 4 safety car, 5 red, 6
    /// virtual safety car, 7 virtual safety car ending. 3 has never been seen
    /// in the feed; it and any new code read as `Unknown`, which `TrackState`
    /// ignores, so the last flag stays.
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

impl RcMessage {
    /// The car the stewards penalised, if this is a penalty:
    /// `FIA STEWARDS: 5 SECOND TIME PENALTY FOR CAR 11 (PER) - ...`.
    /// Investigations and "no further action" never say `PENALTY FOR CAR`.
    pub fn penalised_car(&self) -> Option<u8> {
        let text = self.message.as_deref()?;
        if !text.starts_with("FIA STEWARDS:")
            || text.contains("NO PENALTY")
            || text.contains("SERVED")
        {
            return None;
        }
        let (_, after) = text.split_once(" PENALTY FOR CAR ")?;
        let number: String = after.chars().take_while(char::is_ascii_digit).collect();
        number.parse().ok()
    }
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

/// What kind of session the feed is about, from SessionInfo's `Type`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionType {
    /// A race or a sprint.
    Race,
    /// Qualifying or sprint qualifying.
    Qualifying,
    /// Practice, or any other type (testing): no start lights, no winner.
    Practice,
    /// SessionInfo hasn't said yet.
    #[default]
    Unknown,
}

impl SessionInfo {
    /// `None` for a message without `Type`, which leaves it as it was.
    pub fn session_type(&self) -> Option<SessionType> {
        self.kind.as_deref().map(|kind| match kind {
            "Race" => SessionType::Race,
            "Qualifying" => SessionType::Qualifying,
            _ => SessionType::Practice,
        })
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

/// The race's running order, by car: `{"87":{"Position":"11"},"31":{"Position":"12"}}`
/// when #87 passes #31. Most updates carry only gaps and intervals. It also
/// has `OvertakeState` (2 = gained a place lately, 0 = lost one, 1 = reset
/// about five seconds later), `PitStops` and `IsOut`, which say nothing the
/// positions and TimingData don't. Race only; needs an F1TV token live.
pub type DriverRaceInfo = Numbered<u8, RaceInfoLine>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RaceInfoLine {
    pub position: Option<String>, // a string in the JSON: "11"
}

impl RaceInfoLine {
    pub fn position(&self) -> Option<u8> {
        self.position.as_deref()?.parse().ok()
    }
}

/// A race's laps: the leader's lap and the race's length. An update holds
/// only what changed; a shortened race gets a new `TotalLaps`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LapCount {
    pub current_lap: Option<u32>,
    pub total_laps: Option<u32>,
}

/// The session clock: the time left (`"00:17:42"`) at `Utc`, and whether
/// it's counting down. Sent when it starts or stops, and with the state on
/// connecting.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExtrapolatedClock {
    pub utc: Option<String>,
    pub remaining: Option<String>,
    pub extrapolating: Option<bool>,
}

/// `"01:02:03"` (or `"02:03"`) as milliseconds.
pub fn clock_ms(text: &str) -> Option<u32> {
    let mut seconds: u32 = 0;
    for part in text.split(':') {
        let (whole, fraction) = part.split_once('.').unwrap_or((part, ""));
        seconds = seconds.checked_mul(60)?.checked_add(whole.parse().ok()?)?;
        if !fraction.is_empty() {
            return None;
        }
    }
    seconds.checked_mul(1000)
}

/// The session's parts and status changes. Only the qualifying part is read:
/// `Series` gets an entry with `QualifyingPart` 1, 2 and 3 before each part.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionData {
    pub series: Option<Series>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Series {
    List(Vec<SeriesEntry>),
    Map(Numbered<u32, SeriesEntry>),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeriesEntry {
    pub qualifying_part: Option<u8>,
}

impl SessionData {
    /// The latest qualifying part this message names.
    pub fn qualifying_part(&self) -> Option<u8> {
        let parts = |entries: &mut dyn Iterator<Item = &SeriesEntry>| {
            entries.filter_map(|e| e.qualifying_part).max()
        };
        match self.series.as_ref()? {
            Series::List(v) => parts(&mut v.iter()),
            Series::Map(m) => parts(&mut m.values()),
        }
    }
}

/// The timing screen. Read: who is knocked out of qualifying (the feed sets
/// `KnockedOut` for a part's slowest drivers when the next part is
/// announced, about six minutes after its chequered flag), and what a car is
/// doing: crossing the line (`NumberOfLaps`), in the pits or just out of them,
/// or out of the session.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TimingData {
    pub lines: Option<Numbered<u8, TimingLine>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TimingLine {
    pub knocked_out: Option<bool>,
    pub number_of_laps: Option<u32>,
    pub in_pit: Option<bool>,
    /// Out of the pit lane, until the end of the first sector.
    pub pit_out: Option<bool>,
    pub retired: Option<bool>,
    pub stopped: Option<bool>,
}

/// One parsed message of any stream the lamp reads.
#[derive(Debug)]
pub enum FeedMessage {
    SessionInfo(SessionInfo),
    Track(TrackStatus),
    Session(SessionStatus),
    RaceControl(RcMessage),
    DriverList(DriverList),
    TopThree(TopThree),
    PitLane(PitLaneTimes),
    TimingStats(TimingStats),
    RaceInfo(DriverRaceInfo),
    SessionData(SessionData),
    TimingData(TimingData),
    LapCount(LapCount),
    Clock(ExtrapolatedClock),
}

/// Which stream a file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    SessionInfo,
    TrackStatus,
    SessionStatus,
    RaceControl,
    DriverList,
    TopThree,
    /// Public: a car's time in the pit lane, sent as it leaves.
    PitLane,
    /// Public: personal bests; the fastest lap has position 1.
    TimingStats,
    /// Needs an F1TV token live: the running order, for overtakes.
    DriverRaceInfo,
    /// Public: which qualifying part runs.
    SessionData,
    /// Public: the timing screen; who is knocked out of qualifying.
    TimingData,
    /// Public: a race's current lap and its length.
    LapCount,
    /// Public: the session clock, when it starts and stops.
    Clock,
}

impl Stream {
    pub const ALL: [Stream; 13] = [
        Stream::SessionInfo,
        Stream::TrackStatus,
        Stream::SessionStatus,
        Stream::RaceControl,
        Stream::DriverList,
        Stream::TopThree,
        Stream::PitLane,
        Stream::TimingStats,
        Stream::DriverRaceInfo,
        Stream::SessionData,
        Stream::TimingData,
        Stream::LapCount,
        Stream::Clock,
    ];

    /// The name F1 uses, in the archive file names and the live feed.
    pub fn name(self) -> &'static str {
        match self {
            Stream::SessionInfo => "SessionInfo",
            Stream::TrackStatus => "TrackStatus",
            Stream::SessionStatus => "SessionStatus",
            Stream::RaceControl => "RaceControlMessages",
            Stream::DriverList => "DriverList",
            Stream::TopThree => "TopThree",
            Stream::PitLane => "PitLaneTimeCollection",
            Stream::TimingStats => "TimingStats",
            Stream::DriverRaceInfo => "DriverRaceInfo",
            Stream::SessionData => "SessionData",
            Stream::TimingData => "TimingData",
            Stream::LapCount => "LapCount",
            Stream::Clock => "ExtrapolatedClock",
        }
    }

    pub fn from_name(name: &str) -> Option<Stream> {
        Stream::ALL.into_iter().find(|s| s.name() == name)
    }

    /// Whether a line can carry anything the lamp uses, checked without
    /// parsing. Most TopThree lines only update gaps and lap times, and most
    /// DriverList lines only the running order; the live connection skips
    /// them unparsed, and test fixtures keep only the relevant lines.
    pub fn is_relevant(self, json: &str) -> bool {
        match self {
            Stream::TopThree => json.contains("\"RacingNumber\""),
            Stream::DriverList => json.contains("\"TeamColour\""),
            // Most pit-lane lines only delete finished entries.
            Stream::PitLane => json.contains("\"Duration\""),
            // Most lines are sector times and speeds; a new best lap has a value.
            Stream::TimingStats => {
                json.contains("\"PersonalBestLapTime\":{") && json.contains("\"Value\":\"")
            }
            // About 28,000 lines a race, nearly all gaps and intervals; about
            // 200 change the order.
            Stream::DriverRaceInfo => json.contains("\"Position\""),
            // Most lines are status changes.
            Stream::SessionData => json.contains("\"QualifyingPart\""),
            // About 9000 lines a qualifying, mostly sector times. Read are
            // knockouts and, for pole, cars crossing the line, entering the
            // pits or dropping out: about 360 lines. Leaving the pit lane
            // (`PitOut`) matters for overtakes.
            Stream::TimingData => [
                "\"KnockedOut\":true",
                "\"NumberOfLaps\"",
                "\"InPit\"",
                "\"PitOut\"",
                "\"Retired\"",
                "\"Stopped\"",
            ]
            .iter()
            .any(|key| json.contains(key)),
            _ => true,
        }
    }

    /// One JSON message of this stream as feed messages. A race control line
    /// can hold several messages; the other streams always give one.
    pub fn parse(self, json: &str) -> Result<Vec<FeedMessage>, serde_json::Error> {
        Ok(match self {
            Stream::SessionInfo => vec![FeedMessage::SessionInfo(serde_json::from_str::<
                SessionInfo,
            >(json)?)],
            Stream::TrackStatus => vec![FeedMessage::Track(serde_json::from_str::<TrackStatus>(
                json,
            )?)],
            Stream::SessionStatus => vec![FeedMessage::Session(serde_json::from_str::<
                SessionStatus,
            >(json)?)],
            Stream::RaceControl => serde_json::from_str::<RaceControl>(json)?
                .messages
                .map(|m| m.into_vec())
                .unwrap_or_default()
                .into_iter()
                .map(FeedMessage::RaceControl)
                .collect(),
            Stream::DriverList => vec![FeedMessage::DriverList(
                serde_json::from_str::<DriverList>(json)?,
            )],
            Stream::TopThree => vec![FeedMessage::TopThree(serde_json::from_str::<TopThree>(
                json,
            )?)],
            Stream::PitLane => vec![FeedMessage::PitLane(serde_json::from_str::<PitLaneTimes>(
                json,
            )?)],
            Stream::TimingStats => vec![FeedMessage::TimingStats(serde_json::from_str::<
                TimingStats,
            >(json)?)],
            Stream::DriverRaceInfo => vec![FeedMessage::RaceInfo(serde_json::from_str::<
                DriverRaceInfo,
            >(json)?)],
            Stream::SessionData => vec![FeedMessage::SessionData(serde_json::from_str::<
                SessionData,
            >(json)?)],
            Stream::TimingData => vec![FeedMessage::TimingData(
                serde_json::from_str::<TimingData>(json)?,
            )],
            Stream::LapCount => vec![FeedMessage::LapCount(serde_json::from_str::<LapCount>(
                json,
            )?)],
            Stream::Clock => vec![FeedMessage::Clock(
                serde_json::from_str::<ExtrapolatedClock>(json)?,
            )],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn penalised(text: &str) -> Option<u8> {
        RcMessage {
            utc: None,
            lap: None,
            category: Some("Other".to_owned()),
            flag: None,
            scope: None,
            sector: None,
            racing_number: None,
            status: None,
            mode: None,
            message: Some(text.to_owned()),
        }
        .penalised_car()
    }

    #[test]
    fn a_stewards_penalty_names_the_car() {
        // Monza 2026, lap 30.
        assert_eq!(
            penalised("FIA STEWARDS: 5 SECOND TIME PENALTY FOR CAR 11 (PER) - FAILING TO FOLLOW RACE DIRECTORS INSTRUCTIONS – ESCAPE ROAD INSTRUCTIONS"),
            Some(11)
        );
        assert_eq!(
            penalised(
                "FIA STEWARDS: DRIVE THROUGH PENALTY FOR CAR 1 (NOR) - SPEEDING IN THE PIT LANE"
            ),
            Some(1)
        );
    }

    #[test]
    fn investigations_and_other_messages_are_no_penalty() {
        for text in [
            "FIA STEWARDS: TURN 1 INCIDENT INVOLVING CAR 11 (PER) UNDER INVESTIGATION - FAILING TO FOLLOW RACE DIRECTORS INSTRUCTIONS",
            "FIA STEWARDS: INCIDENT INVOLVING CAR 63 (RUS) NO FURTHER ACTION - YELLOW FLAG INFRINGEMENT (16:17:30)",
            "FIA STEWARDS: INCIDENT INVOLVING CARS 81 (PIA) AND 27 (HUL) REVIEWED NO PENALTY",
            "FIA STEWARDS: 5 SECOND TIME PENALTY FOR CAR 11 (PER) SERVED",
            "BLACK AND WHITE FLAG FOR CAR 87 (BEA) - TRACK LIMITS",
            "CAR 1 (NOR) TIME 2:06.772 DELETED - TRACK LIMITS AT TURN 12 LAP 4 16:43:25",
        ] {
            assert_eq!(penalised(text), None, "{text}");
        }
    }

    #[test]
    fn session_info_type_race_is_a_race() {
        let info: SessionInfo =
            serde_json::from_str(r#"{"Type":"Race","Name":"Sprint","Key":1}"#).unwrap();
        assert_eq!(info.session_type(), Some(SessionType::Race));
        assert_eq!(info.name.as_deref(), Some("Sprint"));
    }

    #[test]
    fn session_info_types_are_told_apart() {
        let kind = |json: &str| {
            serde_json::from_str::<SessionInfo>(json)
                .unwrap()
                .session_type()
        };
        assert_eq!(
            kind(r#"{"Type":"Qualifying"}"#),
            Some(SessionType::Qualifying)
        );
        assert_eq!(kind(r#"{"Type":"Practice"}"#), Some(SessionType::Practice));
        // Anything else behaves like practice: no start lights, no winner.
        assert_eq!(kind(r#"{"Type":"Testing"}"#), Some(SessionType::Practice));
        // A message without a type changes nothing.
        assert_eq!(kind(r#"{"Key":1}"#), None);
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
    fn driver_race_info_reads_positions_by_car() {
        let info: DriverRaceInfo = serde_json::from_str(
            r#"{"87":{"Position":"11","Interval":"","OvertakeState":2},"31":{"Position":"12"},"16":{"Gap":"+0.048"},"_kf":true}"#,
        )
        .unwrap();
        assert_eq!(info.len(), 3);
        assert_eq!(info[&87].position(), Some(11));
        assert_eq!(info[&31].position(), Some(12));
        assert_eq!(info[&16].position(), None);
    }

    #[test]
    fn only_driver_race_info_lines_with_positions_are_relevant() {
        let s = Stream::DriverRaceInfo;
        assert!(s.is_relevant(r#"{"87":{"Position":"11"},"31":{"Position":"12"}}"#));
        assert!(!s.is_relevant(r#"{"16":{"Gap":"+0.048","Interval":"+0.048"}}"#));
        assert!(!s.is_relevant(r#"{"87":{"OvertakeState":1}}"#));
    }

    #[test]
    fn session_data_gives_the_latest_qualifying_part_in_both_shapes() {
        let first: SessionData = serde_json::from_str(
            r#"{"Series":[{"Utc":"t","QualifyingPart":1},{"Utc":"t","QualifyingPart":2}],"StatusSeries":[]}"#,
        )
        .unwrap();
        assert_eq!(first.qualifying_part(), Some(2));
        let update: SessionData =
            serde_json::from_str(r#"{"Series":{"2":{"Utc":"t","QualifyingPart":3}}}"#).unwrap();
        assert_eq!(update.qualifying_part(), Some(3));
    }

    #[test]
    fn session_data_without_parts_has_none() {
        let status: SessionData =
            serde_json::from_str(r#"{"StatusSeries":{"4":{"TrackStatus":"Yellow"}}}"#).unwrap();
        assert_eq!(status.qualifying_part(), None);
        let race: SessionData = serde_json::from_str(r#"{"Series":[]}"#).unwrap();
        assert_eq!(race.qualifying_part(), None);
    }

    #[test]
    fn timing_data_reads_knocked_out_by_car() {
        let t: TimingData = serde_json::from_str(
            r#"{"SessionPart":2,"Lines":{"22":{"KnockedOut":true,"Position":"17"},"63":{"Position":"1"}},"_kf":true}"#,
        )
        .unwrap();
        let lines = t.lines.unwrap();
        assert_eq!(lines[&22].knocked_out, Some(true));
        assert_eq!(lines[&63].knocked_out, None);
    }

    #[test]
    fn timing_data_reads_laps_and_where_a_car_is() {
        let t: TimingData = serde_json::from_str(
            r#"{"Lines":{"3":{"NumberOfLaps":15,"Sectors":{"2":{"Value":"28.1"}}},"44":{"InPit":true,"NumberOfPitStops":4},"12":{"InPit":false,"PitOut":true},"16":{"Retired":true,"Stopped":true}}}"#,
        )
        .unwrap();
        let lines = t.lines.unwrap();
        assert_eq!(lines[&3].number_of_laps, Some(15));
        assert_eq!(lines[&44].in_pit, Some(true));
        assert_eq!(lines[&12].pit_out, Some(true));
        assert_eq!(lines[&16].retired, Some(true));
        assert_eq!(lines[&16].stopped, Some(true));
    }

    #[test]
    fn clock_times_read_as_milliseconds() {
        assert_eq!(clock_ms("00:17:42"), Some((17 * 60 + 42) * 1000));
        assert_eq!(clock_ms("01:00:00"), Some(3_600_000));
        assert_eq!(clock_ms("02:03"), Some(123_000));
        assert_eq!(clock_ms("soon"), None);
        assert_eq!(clock_ms(""), None);
    }
}
