//! Driver events from the feed, each in the driver's team colour:
//!
//! - winner: the leader in TopThree when a race's SessionStatus becomes
//!   `Finished`, or pole once the last part of qualifying is `Finalised`;
//! - knocked out: a driver out of qualifying after Q1 or Q2 (TimingData,
//!   public);
//! - pit stop: a car leaving the pit lane (PitLaneTimeCollection, public);
//! - fastest lap: a new personal best that is the session's best
//!   (TimingStats, public);
//! - overtake: a new entry under a driver in OvertakeSeries (needs an F1TV
//!   token live).
//!
//! The colours come from DriverList: TopThree patches leave `TeamColour` out
//! when it didn't change, e.g. when one Mercedes passes the other.

use std::collections::{HashMap, HashSet};

use crate::{
    color::Rgb,
    feed::{
        update_team_colours, OvertakeSeries, PitLaneTimes, SessionState, TimingData, TimingStats,
        TopThree,
    },
    input::RaceEvent,
    track_state::FeedMessage,
};

/// Used when a driver's team colour never arrived.
const UNKNOWN_TEAM: Rgb = Rgb::WHITE;
/// A pit-lane time longer than this isn't a pit stop: during a red flag every
/// car waits in the pit lane, for half an hour at Monza.
const MAX_PIT_LANE_SECS: f32 = 120.0;
/// Fastest laps from this lap on: lap 1 starts from the grid or the pit
/// lane, so it's never a real lap time. Lap 2 is already a driver's first
/// timed lap in practice and qualifying.
const FIRST_FASTEST_LAP: u32 = 2;
/// Qualifying (and sprint qualifying) ends with its third part.
const LAST_QUALIFYING_PART: u8 = 3;

#[derive(Debug, Default)]
pub struct Tracker {
    /// From SessionInfo; a sprint is of type "Race" too.
    is_race: bool,
    /// From SessionInfo. Qualifying ends each part with `Finished`; only
    /// the last part has a winner: pole.
    is_qualifying: bool,
    /// From SessionData.
    qualifying_part: Option<u8>,
    /// Shown already: the feed may repeat a driver's `KnockedOut`.
    knocked_out: HashSet<u8>,
    /// Racing number in the first line of TopThree.
    leader: Option<u8>,
    colours: HashMap<u8, Rgb>,
    winner_sent: bool,
}

impl Tracker {
    /// Reads one message; returns the events it carries, usually none.
    pub fn apply(&mut self, msg: &FeedMessage) -> Vec<RaceEvent> {
        let mut events = Vec::new();
        match msg {
            FeedMessage::SessionInfo(info) => {
                if let Some(is_race) = info.is_race() {
                    self.is_race = is_race;
                }
                if let Some(is_qualifying) = info.is_qualifying() {
                    self.is_qualifying = is_qualifying;
                }
            }
            FeedMessage::SessionData(data) => {
                if let Some(part) = data.qualifying_part() {
                    self.qualifying_part = Some(part);
                }
            }
            FeedMessage::DriverList(patch) => update_team_colours(&mut self.colours, patch),
            FeedMessage::TopThree(top) => self.read_top_three(top),
            FeedMessage::Session(s)
                if s.status.is_some_and(|state| self.names_the_winner(state))
                    && !self.winner_sent =>
            {
                if let Some(driver) = self.leader {
                    self.winner_sent = true;
                    events.push(RaceEvent::Winner {
                        driver,
                        team_color: self.colour(driver),
                    });
                }
            }
            FeedMessage::PitLane(pit) => self.pit_stops(pit, &mut events),
            FeedMessage::TimingStats(stats) => self.fastest_laps(stats, &mut events),
            FeedMessage::Overtakes(series) => self.overtakes(series, &mut events),
            FeedMessage::TimingData(timing) => self.knockouts(timing, &mut events),
            _ => {}
        }
        events
    }

    /// Whether the session reaching `state` makes TopThree's leader the
    /// winner. A race is `Finished` when the leader takes the flag. Qualifying
    /// is `Finished` when the clock runs out, but drivers still complete their
    /// last laps, which often take pole (Monza 2026: 70 s later, a new
    /// leader); it's `Finalised` once they have.
    fn names_the_winner(&self, state: SessionState) -> bool {
        match state {
            SessionState::Finished => self.is_race,
            SessionState::Finalised => {
                self.is_qualifying && self.qualifying_part == Some(LAST_QUALIFYING_PART)
            }
            _ => false,
        }
    }

    fn knockouts(&mut self, timing: &TimingData, events: &mut Vec<RaceEvent>) {
        let Some(lines) = &timing.lines else { return };
        for (&driver, line) in lines.iter() {
            if line.knocked_out == Some(true) && self.knocked_out.insert(driver) {
                events.push(RaceEvent::KnockedOut {
                    driver,
                    team_color: self.colour(driver),
                });
            }
        }
    }

    fn colour(&self, driver: u8) -> Rgb {
        self.colours.get(&driver).copied().unwrap_or(UNKNOWN_TEAM)
    }

    fn pit_stops(&self, pit: &PitLaneTimes, events: &mut Vec<RaceEvent>) {
        let Some(times) = &pit.pit_times else { return };
        for (&driver, time) in times.iter() {
            if time.seconds().is_some_and(|s| s <= MAX_PIT_LANE_SECS) {
                events.push(RaceEvent::PitStop {
                    driver,
                    team_color: self.colour(driver),
                });
            }
        }
    }

    fn fastest_laps(&self, stats: &TimingStats, events: &mut Vec<RaceEvent>) {
        let Some(lines) = &stats.lines else { return };
        for (&driver, line) in lines.iter() {
            let Some(best) = &line.personal_best_lap_time else {
                continue;
            };
            // A new time (not just a position shuffled by someone else's),
            // the best of the session, and not a first-lap burst.
            let new_time = best.value.as_deref().is_some_and(|v| !v.is_empty());
            let late_enough = best.lap.is_none_or(|lap| lap >= FIRST_FASTEST_LAP);
            if new_time && best.position == Some(1) && late_enough {
                events.push(RaceEvent::FastestLap {
                    driver,
                    team_color: self.colour(driver),
                });
            }
        }
    }

    fn overtakes(&self, series: &OvertakeSeries, events: &mut Vec<RaceEvent>) {
        let Some(drivers) = &series.overtakes else {
            return;
        };
        for (&driver, entries) in drivers.iter() {
            // Each entry is one overtake by `driver`.
            for _ in 0..entries.len() {
                events.push(RaceEvent::Overtake {
                    driver,
                    team_color: self.colour(driver),
                });
            }
        }
    }

    fn read_top_three(&mut self, top: &TopThree) {
        let Some(lines) = &top.lines else { return };
        for (index, line) in lines.indexed() {
            let Some(number) = line.racing_number.as_deref().and_then(|n| n.parse().ok()) else {
                continue;
            };
            // Line 0 is P1; updates name only the lines that changed.
            if index == 0 {
                self.leader = Some(number);
            }
            // A second source of colours, in case DriverList is missing.
            if let Some(rgb) = line.team_colour.as_deref().and_then(|c| c.parse().ok()) {
                self.colours.entry(number).or_insert(rgb);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Stream;

    /// Parses one JSON message of `stream` and feeds it to the tracker;
    /// the first event it caused, if any.
    fn feed(t: &mut Tracker, stream: Stream, json: &str) -> Option<RaceEvent> {
        feed_all(t, stream, json).into_iter().next()
    }

    fn feed_all(t: &mut Tracker, stream: Stream, json: &str) -> Vec<RaceEvent> {
        stream
            .parse(json)
            .unwrap()
            .iter()
            .flat_map(|msg| t.apply(msg))
            .collect()
    }

    fn race() -> Tracker {
        let mut t = Tracker::default();
        feed(
            &mut t,
            Stream::SessionInfo,
            r#"{"Type":"Race","Name":"Race"}"#,
        );
        feed(
            &mut t,
            Stream::DriverList,
            r#"{"12":{"TeamColour":"00D7B6"},"63":{"TeamColour":"00D7B6"},"81":{"TeamColour":"F47600"}}"#,
        );
        t
    }

    fn finish(t: &mut Tracker) -> Option<RaceEvent> {
        feed(t, Stream::SessionStatus, r#"{"Status":"Finished"}"#)
    }

    fn finalise(t: &mut Tracker) -> Option<RaceEvent> {
        feed(t, Stream::SessionStatus, r#"{"Status":"Finalised"}"#)
    }

    fn winner(driver: u8, hex: &str) -> Option<RaceEvent> {
        Some(RaceEvent::Winner {
            driver,
            team_color: hex.parse().unwrap(),
        })
    }

    #[test]
    fn leader_at_finish_is_the_winner_in_team_colour() {
        let mut t = race();
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"81"},{"RacingNumber":"12"}]}"#,
        );
        assert_eq!(finish(&mut t), winner(81, "#f47600"));
    }

    #[test]
    fn leader_change_in_a_patch_without_colour_still_gets_the_team_colour() {
        let mut t = race();
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"63"}]}"#,
        );
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":{"0":{"RacingNumber":"12"},"1":{"RacingNumber":"63"}}}"#,
        );
        assert_eq!(finish(&mut t), winner(12, "#00d7b6"));
    }

    #[test]
    fn patch_without_racing_number_keeps_the_leader() {
        let mut t = race();
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"12"}]}"#,
        );
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":{"0":{"LapTime":"1:25.486"}}}"#,
        );
        assert_eq!(finish(&mut t), winner(12, "#00d7b6"));
    }

    #[test]
    fn second_place_changing_doesnt_change_the_leader() {
        let mut t = race();
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"12"}]}"#,
        );
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":{"1":{"RacingNumber":"81"}}}"#,
        );
        assert_eq!(finish(&mut t), winner(12, "#00d7b6"));
    }

    #[test]
    fn winner_is_sent_once() {
        let mut t = race();
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"12"}]}"#,
        );
        assert!(finish(&mut t).is_some());
        assert_eq!(finish(&mut t), None);
    }

    #[test]
    fn red_flag_stoppage_has_no_winner() {
        let mut t = race();
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"12"}]}"#,
        );
        assert_eq!(
            feed(&mut t, Stream::SessionStatus, r#"{"Status":"Aborted"}"#),
            None
        );
    }

    #[test]
    fn qualifying_has_no_winner() {
        let mut t = Tracker::default();
        feed(&mut t, Stream::SessionInfo, r#"{"Type":"Qualifying"}"#);
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"12"}]}"#,
        );
        assert_eq!(finish(&mut t), None);
    }

    #[test]
    fn sprint_has_a_winner() {
        let mut t = Tracker::default();
        feed(
            &mut t,
            Stream::SessionInfo,
            r#"{"Type":"Race","Name":"Sprint"}"#,
        );
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"4","TeamColour":"F47600"}]}"#,
        );
        assert_eq!(finish(&mut t), winner(4, "#f47600"));
    }

    #[test]
    fn without_session_info_there_is_no_winner() {
        let mut t = Tracker::default();
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"12"}]}"#,
        );
        assert_eq!(finish(&mut t), None);
    }

    #[test]
    fn finish_without_a_leader_sends_nothing_and_a_later_finish_can() {
        let mut t = race();
        assert_eq!(finish(&mut t), None);
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"12"}]}"#,
        );
        assert_eq!(finish(&mut t), winner(12, "#00d7b6"));
    }

    #[test]
    fn top_three_colour_is_used_when_driver_list_is_missing() {
        let mut t = Tracker::default();
        feed(&mut t, Stream::SessionInfo, r#"{"Type":"Race"}"#);
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"44","TeamColour":"ED1131"}]}"#,
        );
        assert_eq!(finish(&mut t), winner(44, "#ed1131"));
    }

    #[test]
    fn unknown_colour_falls_back_to_white() {
        let mut t = Tracker::default();
        feed(&mut t, Stream::SessionInfo, r#"{"Type":"Race"}"#);
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"99"}]}"#,
        );
        assert_eq!(finish(&mut t), winner(99, "#ffffff"));
    }

    #[test]
    fn car_leaving_the_pit_lane_is_a_pit_stop_in_team_colour() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::PitLane,
                r#"{"PitTimes":{"81":{"RacingNumber":"81","Duration":"24.2","Lap":"27"}}}"#
            ),
            Some(RaceEvent::PitStop {
                driver: 81,
                team_color: "#f47600".parse().unwrap()
            })
        );
    }

    #[test]
    fn red_flag_wait_in_the_pit_lane_is_not_a_pit_stop() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::PitLane,
                r#"{"PitTimes":{"63":{"RacingNumber":"63","Duration":"1846.2","Lap":"3"}}}"#
            ),
            None
        );
    }

    #[test]
    fn deleted_pit_lane_entry_is_nothing() {
        let mut t = race();
        assert!(feed_all(
            &mut t,
            Stream::PitLane,
            r#"{"PitTimes":{"_deleted":["81"]}}"#
        )
        .is_empty());
    }

    #[test]
    fn two_cars_leaving_together_are_two_pit_stops() {
        let mut t = race();
        let events = feed_all(
            &mut t,
            Stream::PitLane,
            r#"{"PitTimes":{"12":{"Duration":"24.5"},"63":{"Duration":"25.1"}}}"#,
        );
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn new_session_best_is_a_fastest_lap() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"12":{"PersonalBestLapTime":{"Lap":13,"Position":1,"Value":"1:25.469"}},"3":{"PersonalBestLapTime":{"Position":2}}}}"#
            ),
            Some(RaceEvent::FastestLap {
                driver: 12,
                team_color: "#00d7b6".parse().unwrap()
            })
        );
    }

    #[test]
    fn personal_best_that_is_not_the_session_best_is_nothing() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"81":{"PersonalBestLapTime":{"Lap":13,"Position":2,"Value":"1:25.9"}}}}"#
            ),
            None
        );
    }

    #[test]
    fn position_change_without_a_new_time_is_nothing() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"12":{"PersonalBestLapTime":{"Position":1}}}}"#
            ),
            None
        );
    }

    #[test]
    fn fastest_lap_on_lap_one_is_ignored() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"63":{"PersonalBestLapTime":{"Lap":1,"Position":1,"Value":"1:31.002"}}}}"#
            ),
            None
        );
    }

    #[test]
    fn fastest_lap_counts_from_lap_two() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"63":{"PersonalBestLapTime":{"Lap":2,"Position":1,"Value":"1:26.651"}}}}"#
            ),
            Some(RaceEvent::FastestLap {
                driver: 63,
                team_color: "#00d7b6".parse().unwrap()
            })
        );
    }

    #[test]
    fn empty_best_lap_of_the_first_snapshot_is_nothing() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"1":{"PersonalBestLapTime":{"Value":""}}}}"#
            ),
            None
        );
    }

    #[test]
    fn each_overtake_entry_is_an_overtake() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::Overtakes,
                r#"{"Overtakes":{"81":{"5":{"Timestamp":"2026-09-06T13:45:52.666Z","count":3}}}}"#
            ),
            Some(RaceEvent::Overtake {
                driver: 81,
                team_color: "#f47600".parse().unwrap()
            })
        );
        let first = feed_all(
            &mut t,
            Stream::Overtakes,
            r#"{"Overtakes":{"12":[{"Timestamp":"a","count":1}],"63":[{"Timestamp":"b","count":1}]}}"#,
        );
        assert_eq!(first.len(), 2);
    }

    fn qualifying() -> Tracker {
        let mut t = Tracker::default();
        feed(
            &mut t,
            Stream::SessionInfo,
            r#"{"Type":"Qualifying","Name":"Qualifying"}"#,
        );
        feed(
            &mut t,
            Stream::DriverList,
            r#"{"10":{"TeamColour":"00A1E8"},"12":{"TeamColour":"00D7B6"}}"#,
        );
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"10"}]}"#,
        );
        t
    }

    fn part(t: &mut Tracker, n: u8) {
        feed(
            t,
            Stream::SessionData,
            &format!(r#"{{"Series":{{"{n}":{{"QualifyingPart":{n}}}}}}}"#),
        );
    }

    #[test]
    fn the_end_of_q1_and_q2_has_no_winner() {
        let mut t = qualifying();
        for n in [1, 2] {
            part(&mut t, n);
            assert_eq!(finish(&mut t), None, "Q{n}");
            assert_eq!(finalise(&mut t), None, "Q{n}");
        }
    }

    #[test]
    fn pole_is_the_winner_once_q3_is_finalised() {
        let mut t = qualifying();
        for n in [1, 2, 3] {
            part(&mut t, n);
        }
        assert_eq!(finish(&mut t), None);
        assert_eq!(finalise(&mut t), winner(10, "#00a1e8"));
    }

    #[test]
    fn a_last_lap_after_the_clock_runs_out_still_takes_pole() {
        let mut t = qualifying();
        part(&mut t, 3);
        finish(&mut t);
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":{"0":{"RacingNumber":"12"}}}"#,
        );
        assert_eq!(finalise(&mut t), winner(12, "#00d7b6"));
    }

    #[test]
    fn qualifying_without_a_known_part_has_no_winner() {
        let mut t = qualifying();
        assert_eq!(finalise(&mut t), None);
    }

    #[test]
    fn a_race_has_its_winner_at_finished_not_finalised() {
        let mut t = race();
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":[{"RacingNumber":"12"}]}"#,
        );
        assert_eq!(finish(&mut t), winner(12, "#00d7b6"));
        assert_eq!(finalise(&mut t), None);
    }

    #[test]
    fn knocked_out_drivers_come_once_each_in_team_colour() {
        let mut t = qualifying();
        let knockouts = r#"{"Lines":{"12":{"KnockedOut":true},"10":{"KnockedOut":false}}}"#;
        assert_eq!(
            feed_all(&mut t, Stream::TimingData, knockouts),
            [RaceEvent::KnockedOut {
                driver: 12,
                team_color: "#00d7b6".parse().unwrap()
            }]
        );
        assert!(feed_all(&mut t, Stream::TimingData, knockouts).is_empty());
    }
}
