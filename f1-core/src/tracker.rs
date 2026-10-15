//! Driver events from the feed, each in the driver's team colour:
//!
//! - winner: the leader in TopThree when a race's SessionStatus becomes
//!   `Finished`; the fastest driver of each qualifying part (pole after the
//!   last) or of practice, once every car has finished its lap after the
//!   flag (at most [`LAST_LAPS_MAX`] after it);
//! - knocked out: a driver out of qualifying after Q1 or Q2 (TimingData,
//!   public);
//! - retired: a car out of the session while it runs (TimingData, public);
//! - penalty: the stewards' penalty for a car (race control, public);
//! - pit stop: a car leaving the pit lane (PitLaneTimeCollection, public);
//! - fastest lap: a new personal best that is the session's best
//!   (TimingStats, public);
//! - overtake: a new entry under a driver in OvertakeSeries (needs an F1TV
//!   token live).
//!
//! The colours come from DriverList: TopThree patches leave `TeamColour` out
//! when it didn't change, e.g. when one Mercedes passes the other.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use crate::{
    color::Rgb,
    feed::{
        update_team_colours, FeedMessage, OvertakeSeries, PitLaneTimes, SessionState, SessionType,
        TimingData, TimingLine, TimingStats, TopThree,
    },
    input::RaceEvent,
};

/// Used when a driver's team colour never arrived.
const UNKNOWN_TEAM: Rgb = Rgb::WHITE;
/// A pit-lane time longer than this isn't a pit stop: during a red flag every
/// car waits in the pit lane, for half an hour at Monza.
const MAX_PIT_LANE_SECS: f32 = 120.0;
/// After qualifying's last flag, pole waits at most this long for the cars
/// still on track to finish their laps. Singapore 2026 sprint qualifying:
/// all done 88 s after the flag; `Finalised` came after 18 minutes.
pub const LAST_LAPS_MAX: Duration = Duration::from_secs(120);

/// What TimingData last said about a car.
#[derive(Debug, Default)]
struct Car {
    laps: Option<u32>,
    in_pit: bool,
    knocked_out: bool,
    retired: bool,
    stopped: bool,
}

impl Car {
    fn update(&mut self, line: &TimingLine) {
        fn set(field: &mut bool, value: Option<bool>) {
            if let Some(v) = value {
                *field = v;
            }
        }
        if line.number_of_laps.is_some() {
            self.laps = line.number_of_laps;
        }
        set(&mut self.in_pit, line.in_pit);
        set(&mut self.knocked_out, line.knocked_out);
        set(&mut self.retired, line.retired);
        set(&mut self.stopped, line.stopped);
    }

    /// Won't set a time any more: in the pits or out of the session.
    fn off_track(&self) -> bool {
        self.in_pit || self.knocked_out || self.retired || self.stopped
    }
}

/// From qualifying's last flag until pole is known.
#[derive(Debug)]
struct LastLaps {
    /// Feed time of the flag; `None` until the first timestamp, which then
    /// stands in for it.
    flag_at: Option<Duration>,
    /// The cars that may still set a time, with their lap count at the
    /// flag. A car is done when it crosses the line (the count changes) or
    /// leaves the track.
    on_track: HashMap<u8, Option<u32>>,
}

#[derive(Debug, Default)]
pub struct Tracker {
    /// From SessionInfo. Unknown has no winner: better none than a wrong
    /// one. Qualifying ends each part with `Finished`, and each part's
    /// fastest driver shows like pole after the last.
    session_type: SessionType,
    /// From SessionData.
    qualifying_part: Option<u8>,
    /// Shown already: the feed may repeat a driver's `KnockedOut`.
    knocked_out: HashSet<u8>,
    /// Shown already, like `knocked_out`.
    retired: HashSet<u8>,
    /// The session finished and hasn't started again. The feed marks cars
    /// that didn't finish as retired after the flag (Singapore 2026: #23,
    /// 83 s later): not news then, and the winner is showing.
    session_over: bool,
    /// From TimingData updates; the snapshot on connecting is too large to
    /// read, so a car can be missing.
    cars: HashMap<u8, Car>,
    /// The feed's clock, from [`Tracker::tick`].
    now: Option<Duration>,
    last_laps: Option<LastLaps>,
    /// Racing number in the first line of TopThree.
    leader: Option<u8>,
    /// Racing number at `Position: 1` in TimingStats' personal bests.
    fastest_lap_holder: Option<u8>,
    colours: HashMap<u8, Rgb>,
    /// For this session, or this qualifying part.
    winner_sent: bool,
}

impl Tracker {
    /// Moves the feed's clock to `at`: a live message's timestamp, or a
    /// replay's position. Returns pole when the cars' last laps run out of
    /// time.
    pub fn tick(&mut self, at: Duration) -> Option<RaceEvent> {
        let now = self.now.map_or(at, |now| now.max(at));
        self.now = Some(now);
        // A flag before the first timestamp (in the state on connecting)
        // counts from the first one: otherwise the cap never runs out.
        if let Some(last) = &mut self.last_laps {
            last.flag_at.get_or_insert(now);
        }
        self.pole_after_last_laps()
    }

    /// Reads one message; returns the events it carries, usually none.
    pub fn apply(&mut self, msg: &FeedMessage) -> Vec<RaceEvent> {
        let mut events = Vec::new();
        match msg {
            FeedMessage::SessionInfo(info) => {
                if let Some(kind) = info.session_type() {
                    self.session_type = kind;
                }
            }
            FeedMessage::SessionData(data) => {
                if let Some(part) = data.qualifying_part() {
                    if self.qualifying_part != Some(part) {
                        // A new part has a fastest driver of its own.
                        self.winner_sent = false;
                        self.last_laps = None;
                    }
                    self.qualifying_part = Some(part);
                }
            }
            FeedMessage::DriverList(patch) => update_team_colours(&mut self.colours, patch),
            FeedMessage::TopThree(top) => self.read_top_three(top),
            FeedMessage::Session(s) => {
                if let Some(state) = s.status {
                    match state {
                        SessionState::Started => self.session_over = false,
                        SessionState::Finished | SessionState::Finalised | SessionState::Ends => {
                            self.session_over = true;
                        }
                        _ => {}
                    }
                    events.extend(self.session_state(state));
                }
            }
            FeedMessage::PitLane(pit) => self.pit_stops(pit, &mut events),
            FeedMessage::RaceControl(m) => {
                if let Some(driver) = m.penalised_car() {
                    events.push(RaceEvent::Penalty {
                        driver,
                        team_color: self.colour(driver),
                    });
                }
            }
            FeedMessage::TimingStats(stats) => self.fastest_laps(stats, &mut events),
            FeedMessage::Overtakes(series) => self.overtakes(series, &mut events),
            FeedMessage::TimingData(timing) => {
                self.timing(timing, &mut events);
                events.extend(self.pole_after_last_laps());
            }
            _ => {}
        }
        events
    }

    /// The winner, if the session reaching `state` names one. A race is
    /// `Finished` when the leader takes the flag. Qualifying and practice
    /// are `Finished` when the clock runs out, but drivers still complete
    /// their last laps, which often take pole (Monza 2026: 70 s later, a new
    /// leader), so P1 waits for them. There TopThree is ordered by best lap,
    /// so its leader is the fastest driver. `Finalised`, often many minutes
    /// later, is the fallback.
    fn session_state(&mut self, state: SessionState) -> Option<RaceEvent> {
        let is_race = self.session_type == SessionType::Race;
        let after_last_laps = match self.session_type {
            // Each part; a part not known yet has no P1 to show.
            SessionType::Qualifying => self.qualifying_part.is_some(),
            SessionType::Practice => true,
            SessionType::Race | SessionType::Unknown => false,
        };
        match state {
            SessionState::Finished if is_race => self.winner(),
            SessionState::Finished if after_last_laps => {
                self.start_last_laps();
                self.pole_after_last_laps()
            }
            SessionState::Finalised if after_last_laps => self.winner(),
            _ => None,
        }
    }

    /// Qualifying's last flag: notes which cars may still set a time. A car
    /// only DriverList knows of may be on track too.
    fn start_last_laps(&mut self) {
        let numbers: HashSet<u8> = self
            .colours
            .keys()
            .chain(self.cars.keys())
            .copied()
            .collect();
        let on_track = numbers
            .into_iter()
            .filter_map(|n| match self.cars.get(&n) {
                Some(car) if car.off_track() => None,
                car => Some((n, car.and_then(|c| c.laps))),
            })
            .collect();
        self.last_laps = Some(LastLaps {
            flag_at: self.now,
            on_track,
        });
    }

    /// Pole, once every car on track at the last flag is done, or
    /// [`LAST_LAPS_MAX`] after the flag.
    fn pole_after_last_laps(&mut self) -> Option<RaceEvent> {
        let last = self.last_laps.as_ref()?;
        let time_up = matches!(
            (last.flag_at, self.now),
            (Some(flag), Some(now)) if now.saturating_sub(flag) >= LAST_LAPS_MAX
        );
        let all_done = last.on_track.iter().all(|(n, laps_at_flag)| {
            self.cars
                .get(n)
                .is_some_and(|car| car.off_track() || car.laps != *laps_at_flag)
        });
        if !time_up && !all_done {
            return None;
        }
        self.last_laps = None;
        self.winner()
    }

    /// TopThree's leader as the winner, once.
    fn winner(&mut self) -> Option<RaceEvent> {
        if self.winner_sent {
            return None;
        }
        let driver = self.leader?;
        self.winner_sent = true;
        Some(RaceEvent::Winner {
            driver,
            team_color: self.colour(driver),
        })
    }

    /// Knockouts, retirements, and what each car is doing.
    fn timing(&mut self, timing: &TimingData, events: &mut Vec<RaceEvent>) {
        let Some(lines) = &timing.lines else { return };
        for (&driver, line) in lines.iter() {
            self.cars.entry(driver).or_default().update(line);
            let team_color = self.colour(driver);
            if line.knocked_out == Some(true) && self.knocked_out.insert(driver) {
                events.push(RaceEvent::KnockedOut { driver, team_color });
            }
            if line.retired == Some(true) && !self.session_over && self.retired.insert(driver) {
                events.push(RaceEvent::Retired { driver, team_color });
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

    /// A new personal best that is the session's best, on any lap. Usually
    /// it comes with `Position: 1`, but the feed sends a position only when
    /// it changes: the holder beating their own time gets none (Singapore
    /// 2026, HAM lap 23), so the holder is remembered.
    fn fastest_laps(&mut self, stats: &TimingStats, events: &mut Vec<RaceEvent>) {
        let Some(lines) = &stats.lines else { return };
        let bests = || {
            lines
                .iter()
                .filter_map(|(&driver, line)| Some((driver, line.personal_best_lap_time.as_ref()?)))
        };
        let new_holder = bests()
            .find(|(_, best)| best.position == Some(1))
            .map(|(driver, _)| driver);
        for (driver, best) in bests() {
            // A new time, not just a position shuffled by someone else's.
            let new_time = best.value.as_deref().is_some_and(|v| !v.is_empty());
            // A deleted lap also brings a (slower) time, but no `Lap`.
            let holder_improved = best.position.is_none()
                && best.lap.is_some()
                && self.fastest_lap_holder == Some(driver)
                && new_holder.is_none();
            if new_time && (best.position == Some(1) || holder_improved) {
                events.push(RaceEvent::FastestLap {
                    driver,
                    team_color: self.colour(driver),
                });
            }
        }
        if new_holder.is_some() {
            self.fastest_lap_holder = new_holder;
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
    use crate::feed::Stream;

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
    fn a_stewards_penalty_is_a_penalty_in_team_colour() {
        let mut t = race();
        feed(
            &mut t,
            Stream::DriverList,
            r#"{"11":{"TeamColour":"6C98FF"}}"#,
        );
        assert_eq!(
            feed(
                &mut t,
                Stream::RaceControl,
                r#"{"Messages":{"114":{"Category":"Other","Message":"FIA STEWARDS: 5 SECOND TIME PENALTY FOR CAR 11 (PER) - FAILING TO FOLLOW RACE DIRECTORS INSTRUCTIONS"}}}"#
            ),
            Some(RaceEvent::Penalty {
                driver: 11,
                team_color: "#6c98ff".parse().unwrap()
            })
        );
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
    fn fastest_lap_on_lap_one_counts_too() {
        let mut t = race();
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"63":{"PersonalBestLapTime":{"Lap":1,"Position":1,"Value":"1:31.002"}}}}"#
            ),
            Some(RaceEvent::FastestLap {
                driver: 63,
                team_color: "#00d7b6".parse().unwrap()
            })
        );
    }

    /// Singapore 2026 race: HAM's lap 22 takes the fastest lap.
    fn ham_holds_the_fastest_lap() -> Tracker {
        let mut t = race();
        feed(
            &mut t,
            Stream::TimingStats,
            r#"{"Lines":{"3":{"PersonalBestLapTime":{"Position":2}},"44":{"PersonalBestLapTime":{"Lap":22,"Position":1,"Value":"1:37.170"}}}}"#,
        )
        .unwrap();
        t
    }

    #[test]
    fn holder_beating_their_own_time_is_a_fastest_lap() {
        let mut t = ham_holds_the_fastest_lap();
        // Lap 23: still P1, so the feed sends no position.
        assert!(matches!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"44":{"PersonalBestLapTime":{"Lap":23,"Value":"1:37.075"}}}}"#
            ),
            Some(RaceEvent::FastestLap { driver: 44, .. })
        ));
    }

    #[test]
    fn former_holder_improving_behind_the_new_one_is_nothing() {
        let mut t = ham_holds_the_fastest_lap();
        feed(
            &mut t,
            Stream::TimingStats,
            r#"{"Lines":{"16":{"PersonalBestLapTime":{"Lap":27,"Position":1,"Value":"1:36.890"}},"44":{"PersonalBestLapTime":{"Position":2}}}}"#,
        )
        .unwrap();
        // HAM's lap 28 is a personal best, still P2.
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"44":{"PersonalBestLapTime":{"Lap":28,"Value":"1:36.922"}}}}"#
            ),
            None
        );
    }

    #[test]
    fn holder_losing_a_deleted_lap_is_nothing() {
        let mut t = ham_holds_the_fastest_lap();
        assert_eq!(
            feed(
                &mut t,
                Stream::TimingStats,
                r#"{"Lines":{"44":{"PersonalBestLapTime":{"Value":"1:38.620"}}}}"#
            ),
            None
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
    fn each_qualifying_part_shows_its_fastest_driver_after_the_last_laps() {
        let mut t = qualifying();
        for n in 1..=3u8 {
            let laps = u32::from(n) * 10;
            part(&mut t, n);
            timing(
                &mut t,
                &format!(
                    r#"{{"Lines":{{"10":{{"NumberOfLaps":{laps},"InPit":false}},"12":{{"NumberOfLaps":{laps},"InPit":false}}}}}}"#
                ),
            );
            t.tick(secs(1000 * u64::from(n)));
            assert_eq!(finish(&mut t), None, "Q{n} at the flag");
            let done = laps + 1;
            assert_eq!(
                timing(
                    &mut t,
                    &format!(
                        r#"{{"Lines":{{"10":{{"NumberOfLaps":{done}}},"12":{{"NumberOfLaps":{done}}}}}}}"#
                    ),
                ),
                winner(10, "#00a1e8"),
                "Q{n}"
            );
            assert_eq!(finish(&mut t), None, "Q{n} only once");
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

    fn timing(t: &mut Tracker, json: &str) -> Option<RaceEvent> {
        feed(t, Stream::TimingData, json)
    }

    /// Q3 with cars 10 and 12 on track (lap 14) and 5 knocked out, the
    /// clock at 1000 s; then the flag.
    fn q3_flag() -> Tracker {
        let mut t = qualifying();
        part(&mut t, 3);
        timing(
            &mut t,
            r#"{"Lines":{"10":{"NumberOfLaps":14,"InPit":false},"12":{"NumberOfLaps":14,"InPit":false},"5":{"KnockedOut":true,"InPit":true}}}"#,
        );
        t.tick(secs(1000));
        assert_eq!(finish(&mut t), None);
        t
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn pole_comes_once_every_car_on_track_has_crossed_the_line() {
        let mut t = q3_flag();
        assert_eq!(
            timing(&mut t, r#"{"Lines":{"10":{"NumberOfLaps":15}}}"#),
            None
        );
        assert_eq!(
            timing(&mut t, r#"{"Lines":{"12":{"NumberOfLaps":15}}}"#),
            winner(10, "#00a1e8")
        );
        assert_eq!(finalise(&mut t), None, "sent once");
    }

    #[test]
    fn a_car_going_into_the_pits_has_finished() {
        let mut t = q3_flag();
        timing(&mut t, r#"{"Lines":{"10":{"NumberOfLaps":15}}}"#);
        assert_eq!(
            timing(&mut t, r#"{"Lines":{"12":{"InPit":true}}}"#),
            winner(10, "#00a1e8")
        );
    }

    #[test]
    fn a_new_leader_on_the_last_lap_takes_pole() {
        let mut t = q3_flag();
        timing(&mut t, r#"{"Lines":{"10":{"NumberOfLaps":15}}}"#);
        feed(
            &mut t,
            Stream::TopThree,
            r#"{"Lines":{"0":{"RacingNumber":"12"}}}"#,
        );
        assert_eq!(
            timing(&mut t, r#"{"Lines":{"12":{"NumberOfLaps":15}}}"#),
            winner(12, "#00d7b6")
        );
    }

    #[test]
    fn pole_comes_two_minutes_after_the_flag_at_the_latest() {
        let mut t = q3_flag();
        timing(&mut t, r#"{"Lines":{"10":{"NumberOfLaps":15}}}"#);
        assert_eq!(
            t.tick(secs(1000) + LAST_LAPS_MAX - Duration::from_millis(1)),
            None
        );
        assert_eq!(t.tick(secs(1000) + LAST_LAPS_MAX), winner(10, "#00a1e8"));
    }

    #[test]
    fn a_flag_before_any_timestamp_counts_from_the_first_one() {
        // Connected after the flag: the state has no timestamp.
        let mut t = qualifying();
        part(&mut t, 3);
        timing(&mut t, r#"{"Lines":{"10":{"NumberOfLaps":14}}}"#);
        assert_eq!(finish(&mut t), None);
        assert_eq!(t.tick(secs(1000)), None);
        assert_eq!(
            t.tick(secs(1000) + LAST_LAPS_MAX - Duration::from_millis(1)),
            None
        );
        assert_eq!(t.tick(secs(1000) + LAST_LAPS_MAX), winner(10, "#00a1e8"));
    }

    #[test]
    fn a_car_without_timing_data_is_waited_for() {
        // Car 12 is only in DriverList: maybe on track since a reconnect.
        let mut t = qualifying();
        part(&mut t, 3);
        timing(&mut t, r#"{"Lines":{"10":{"NumberOfLaps":14}}}"#);
        t.tick(secs(1000));
        finish(&mut t);
        assert_eq!(
            timing(&mut t, r#"{"Lines":{"10":{"NumberOfLaps":15}}}"#),
            None
        );
        assert!(t.tick(secs(1000) + LAST_LAPS_MAX).is_some());
    }

    #[test]
    fn pole_is_at_the_flag_when_every_car_is_already_in_the_pits() {
        let mut t = qualifying();
        part(&mut t, 3);
        timing(
            &mut t,
            r#"{"Lines":{"10":{"InPit":true},"12":{"InPit":true}}}"#,
        );
        assert_eq!(finish(&mut t), winner(10, "#00a1e8"));
    }

    #[test]
    fn the_end_of_q1_waits_two_minutes_at_most_too() {
        let mut t = qualifying();
        part(&mut t, 1);
        timing(
            &mut t,
            r#"{"Lines":{"10":{"NumberOfLaps":14,"InPit":false}}}"#,
        );
        t.tick(secs(1000));
        assert_eq!(finish(&mut t), None);
        assert_eq!(t.tick(secs(1000) + LAST_LAPS_MAX), winner(10, "#00a1e8"));
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

    fn retire(t: &mut Tracker, driver: u8) -> Option<RaceEvent> {
        timing(
            t,
            &format!(r#"{{"Lines":{{"{driver}":{{"Retired":true,"Stopped":true,"Status":92}}}}}}"#),
        )
    }

    #[test]
    fn a_retirement_comes_once_in_team_colour() {
        let mut t = race();
        feed(&mut t, Stream::SessionStatus, r#"{"Status":"Started"}"#);
        assert_eq!(
            retire(&mut t, 81),
            Some(RaceEvent::Retired {
                driver: 81,
                team_color: "#f47600".parse().unwrap()
            })
        );
        assert_eq!(retire(&mut t, 81), None, "the feed repeats it");
    }

    #[test]
    fn a_car_marked_retired_after_the_flag_is_not_news() {
        let mut t = race();
        feed(&mut t, Stream::SessionStatus, r#"{"Status":"Started"}"#);
        finish(&mut t);
        assert_eq!(retire(&mut t, 81), None);
        // A restart after a red flag runs again.
        feed(&mut t, Stream::SessionStatus, r#"{"Status":"Started"}"#);
        assert!(retire(&mut t, 12).is_some());
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
