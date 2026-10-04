//! Driver events from the feed. For now: the winner, the leader in TopThree
//! at the moment a race's SessionStatus becomes `Finished`.
//!
//! The colour comes from DriverList: TopThree patches leave `TeamColour` out
//! when it didn't change, e.g. when one Mercedes passes the other.

use std::collections::HashMap;

use crate::{
    color::Rgb,
    feed::{update_team_colours, SessionState, TopThree},
    input::RaceEvent,
    track_state::FeedMessage,
};

/// Used when a winner's team colour never arrived.
const UNKNOWN_TEAM: Rgb = Rgb::WHITE;

#[derive(Debug, Default)]
pub struct Tracker {
    /// From SessionInfo. Qualifying also ends parts with `Finished`, so only
    /// a race (or sprint, also of type "Race") has a winner.
    is_race: bool,
    /// Racing number in the first line of TopThree.
    leader: Option<u8>,
    colours: HashMap<u8, Rgb>,
    winner_sent: bool,
}

impl Tracker {
    /// Reads one message; returns the winner when the race has just finished.
    pub fn apply(&mut self, msg: &FeedMessage) -> Option<RaceEvent> {
        match msg {
            FeedMessage::SessionInfo(info) => {
                if let Some(is_race) = info.is_race() {
                    self.is_race = is_race;
                }
            }
            FeedMessage::DriverList(patch) => update_team_colours(&mut self.colours, patch),
            FeedMessage::TopThree(top) => self.read_top_three(top),
            FeedMessage::Session(s)
                if s.status == Some(SessionState::Finished)
                    && self.is_race
                    && !self.winner_sent =>
            {
                let driver = self.leader?;
                self.winner_sent = true;
                return Some(RaceEvent::Winner {
                    driver,
                    team_color: self.colours.get(&driver).copied().unwrap_or(UNKNOWN_TEAM),
                });
            }
            _ => {}
        }
        None
    }

    fn read_top_three(&mut self, top: &TopThree) {
        let Some(lines) = &top.lines else { return };
        for (index, line) in lines.indexed() {
            let Some(number) = line.racing_number.as_deref().and_then(|n| n.parse().ok()) else {
                continue;
            };
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

    /// Parses one JSON message of `stream` and feeds it to the tracker.
    fn feed(t: &mut Tracker, stream: Stream, json: &str) -> Option<RaceEvent> {
        let mut out = None;
        for msg in stream.parse(json).unwrap() {
            out = out.or(t.apply(&msg));
        }
        out
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
}
