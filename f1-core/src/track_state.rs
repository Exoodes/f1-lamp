use std::collections::HashSet;

use crate::{
    feed::{
        DriverList, RcMessage, SessionInfo, SessionState, SessionStatus, TopThree, TrackCode,
        TrackStatus,
    },
    input::{RaceEvent, TrackFlag},
};

#[derive(Debug)]
pub enum FeedMessage {
    SessionInfo(SessionInfo),
    Track(TrackStatus),
    Session(SessionStatus),
    RaceControl(RcMessage),
    DriverList(DriverList),
    TopThree(TopThree),
}

#[derive(Debug, Default)]
pub struct TrackState {
    session: Option<SessionState>,
    track: Option<TrackCode>,
    double_yellow: HashSet<u8>,
    shown: Option<TrackFlag>,
    started: bool,
}

impl TrackState {
    pub fn apply(&mut self, msg: FeedMessage) -> Vec<RaceEvent> {
        let mut events = Vec::new();

        match msg {
            FeedMessage::Track(t) => {
                if let Some(code) = t.code() {
                    self.track = Some(code);
                }
            }
            FeedMessage::Session(s) => {
                if let Some(state) = s.status {
                    self.session = Some(state);
                    if state == SessionState::Started && !self.started {
                        events.push(RaceEvent::StartLights);
                        self.started = true;
                    }
                }
            }
            FeedMessage::RaceControl(m) => {
                let key = (m.category.as_deref(), m.flag.as_deref(), m.scope.as_deref());
                match key {
                    (Some("Flag"), Some("DOUBLE YELLOW"), Some("Sector")) => {
                        if let Some(sector) = m.sector {
                            self.double_yellow.insert(sector);
                        }
                    }
                    (Some("Flag"), Some("YELLOW" | "CLEAR"), Some("Sector")) => {
                        if let Some(sector) = m.sector {
                            self.double_yellow.remove(&sector);
                        }
                    }
                    (Some("Flag"), Some("CLEAR" | "GREEN"), Some("Track")) => {
                        self.double_yellow.clear();
                    }
                    (Some("Flag"), Some("CHEQUERED"), _) => {
                        events.push(RaceEvent::ChequeredFlag);
                    }
                    _ => {}
                }
            }
            FeedMessage::SessionInfo(_) | FeedMessage::DriverList(_) | FeedMessage::TopThree(_) => {
            }
        }

        let new = self.derive();
        if new != self.shown {
            if let Some(flag) = new {
                events.push(RaceEvent::TrackFlag(flag));
            }
            self.shown = new;
        }

        events
    }

    pub fn flag(&self) -> Option<TrackFlag> {
        self.shown
    }

    fn derive(&self) -> Option<TrackFlag> {
        use SessionState::{Aborted, Started};

        match (self.session, self.track) {
            (Some(Aborted), _) => Some(TrackFlag::Red),
            (Some(Started), Some(TrackCode::Red)) => Some(TrackFlag::Red),
            (Some(Started), Some(TrackCode::ScDeployed)) => Some(TrackFlag::SafetyCar),
            (Some(Started), Some(TrackCode::VscDeployed | TrackCode::VscEnding)) => {
                Some(TrackFlag::VirtualSafetyCar)
            }
            (Some(Started), Some(TrackCode::Yellow)) if !self.double_yellow.is_empty() => {
                Some(TrackFlag::DoubleYellow)
            }
            (Some(Started), Some(TrackCode::Yellow)) => Some(TrackFlag::Yellow),
            (Some(Started), _) => Some(TrackFlag::Green),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(code: &str) -> FeedMessage {
        FeedMessage::Track(TrackStatus {
            status: Some(code.to_owned()),
            message: None,
        })
    }

    fn session(state: SessionState) -> FeedMessage {
        FeedMessage::Session(SessionStatus {
            status: Some(state),
        })
    }

    fn rc(flag: &str, scope: &str, sector: Option<u8>) -> FeedMessage {
        FeedMessage::RaceControl(RcMessage {
            utc: None,
            lap: None,
            category: Some("Flag".to_owned()),
            flag: Some(flag.to_owned()),
            scope: Some(scope.to_owned()),
            sector,
            racing_number: None,
            status: None,
            mode: None,
            message: None,
        })
    }

    fn flag(f: TrackFlag) -> Vec<RaceEvent> {
        vec![RaceEvent::TrackFlag(f)]
    }

    /// A state that is racing under green.
    fn racing() -> TrackState {
        let mut s = TrackState::default();
        s.apply(track("1"));
        s.apply(session(SessionState::Started));
        s
    }

    #[test]
    fn nothing_is_shown_before_the_start() {
        let mut s = TrackState::default();
        assert!(s.apply(track("2")).is_empty());
        assert!(s.apply(track("1")).is_empty());
        assert!(s.apply(session(SessionState::Inactive)).is_empty());
    }

    #[test]
    fn first_start_sends_start_lights_then_green() {
        let mut s = TrackState::default();
        s.apply(track("1"));
        assert_eq!(
            s.apply(session(SessionState::Started)),
            [
                RaceEvent::StartLights,
                RaceEvent::TrackFlag(TrackFlag::Green)
            ]
        );
    }

    #[test]
    fn start_lights_only_on_first_start() {
        let mut s = racing();
        s.apply(session(SessionState::Aborted));
        assert_eq!(
            s.apply(session(SessionState::Started)),
            flag(TrackFlag::Green)
        );
    }

    #[test]
    fn track_status_codes_become_flags() {
        let cases = [
            ("2", TrackFlag::Yellow),
            ("4", TrackFlag::SafetyCar),
            ("5", TrackFlag::Red),
            ("6", TrackFlag::VirtualSafetyCar),
        ];
        for (code, expected) in cases {
            let mut s = racing();
            assert_eq!(s.apply(track(code)), flag(expected), "code {code}");
        }
    }

    #[test]
    fn same_flag_twice_is_sent_once() {
        let mut s = racing();
        assert_eq!(s.apply(track("2")), flag(TrackFlag::Yellow));
        assert!(s.apply(track("2")).is_empty());
    }

    #[test]
    fn vsc_ending_stays_vsc_until_all_clear() {
        let mut s = racing();
        s.apply(track("6"));
        assert!(s.apply(track("7")).is_empty());
        assert_eq!(s.apply(track("1")), flag(TrackFlag::Green));
    }

    #[test]
    fn unknown_track_code_shows_green() {
        let mut s = racing();
        s.apply(track("2"));
        assert_eq!(s.apply(track("3")), flag(TrackFlag::Green));
    }

    #[test]
    fn track_status_without_code_changes_nothing() {
        let mut s = racing();
        s.apply(track("2"));
        let empty = FeedMessage::Track(TrackStatus {
            status: None,
            message: Some("x".to_owned()),
        });
        assert!(s.apply(empty).is_empty());
    }

    #[test]
    fn sector_yellows_during_yellow_cause_no_event() {
        let mut s = racing();
        s.apply(track("2"));
        assert!(s.apply(rc("YELLOW", "Sector", Some(3))).is_empty());
        assert!(s.apply(rc("CLEAR", "Sector", Some(3))).is_empty());
    }

    #[test]
    fn double_yellow_in_a_sector_upgrades_yellow() {
        let mut s = racing();
        s.apply(track("2"));
        assert_eq!(
            s.apply(rc("DOUBLE YELLOW", "Sector", Some(15))),
            flag(TrackFlag::DoubleYellow)
        );
    }

    #[test]
    fn double_yellow_waits_for_track_status_yellow() {
        let mut s = racing();
        assert!(s.apply(rc("DOUBLE YELLOW", "Sector", Some(15))).is_empty());
        assert_eq!(s.apply(track("2")), flag(TrackFlag::DoubleYellow));
    }

    #[test]
    fn clearing_the_last_double_yellow_sector_goes_back_to_yellow() {
        let mut s = racing();
        s.apply(track("2"));
        s.apply(rc("DOUBLE YELLOW", "Sector", Some(15)));
        s.apply(rc("DOUBLE YELLOW", "Sector", Some(16)));
        assert!(s.apply(rc("CLEAR", "Sector", Some(15))).is_empty());
        assert_eq!(
            s.apply(rc("CLEAR", "Sector", Some(16))),
            flag(TrackFlag::Yellow)
        );
    }

    #[test]
    fn plain_yellow_downgrades_double_yellow_in_that_sector() {
        let mut s = racing();
        s.apply(track("2"));
        s.apply(rc("DOUBLE YELLOW", "Sector", Some(15)));
        assert_eq!(
            s.apply(rc("YELLOW", "Sector", Some(15))),
            flag(TrackFlag::Yellow)
        );
    }

    #[test]
    fn track_clear_ends_double_yellow() {
        let mut s = racing();
        s.apply(track("2"));
        s.apply(rc("DOUBLE YELLOW", "Sector", Some(15)));
        s.apply(rc("DOUBLE YELLOW", "Sector", Some(16)));
        s.apply(rc("CLEAR", "Track", None));
        assert_eq!(s.apply(track("1")), flag(TrackFlag::Green));
        // A new yellow later must not come back as double.
        assert_eq!(s.apply(track("2")), flag(TrackFlag::Yellow));
    }

    #[test]
    fn safety_car_wins_over_double_yellow() {
        let mut s = racing();
        s.apply(track("2"));
        s.apply(rc("DOUBLE YELLOW", "Sector", Some(15)));
        assert_eq!(s.apply(track("4")), flag(TrackFlag::SafetyCar));
    }

    #[test]
    fn aborted_is_red_at_once() {
        let mut s = racing();
        assert_eq!(
            s.apply(session(SessionState::Aborted)),
            flag(TrackFlag::Red)
        );
    }

    #[test]
    fn aborted_stays_red_when_track_status_says_all_clear() {
        let mut s = racing();
        s.apply(session(SessionState::Aborted));
        assert!(s.apply(track("5")).is_empty());
        assert!(s.apply(track("1")).is_empty());
        assert!(s.apply(track("2")).is_empty());
    }

    #[test]
    fn restart_after_red_follows_track_status() {
        let mut s = racing();
        s.apply(session(SessionState::Aborted));
        s.apply(track("4"));
        assert_eq!(
            s.apply(session(SessionState::Started)),
            flag(TrackFlag::SafetyCar)
        );
    }

    #[test]
    fn chequered_flag_is_its_own_event() {
        let mut s = racing();
        assert_eq!(
            s.apply(rc("CHEQUERED", "Track", None)),
            [RaceEvent::ChequeredFlag]
        );
    }

    #[test]
    fn nothing_is_sent_after_the_finish() {
        let mut s = racing();
        s.apply(session(SessionState::Finished));
        assert!(s.apply(track("2")).is_empty());
        assert!(s.apply(track("1")).is_empty());
    }

    #[test]
    fn blue_flags_and_other_messages_change_nothing() {
        let mut s = racing();
        assert!(s.apply(rc("BLUE", "Driver", None)).is_empty());
        let other = FeedMessage::RaceControl(RcMessage {
            utc: None,
            lap: None,
            category: Some("Other".to_owned()),
            flag: None,
            scope: None,
            sector: None,
            racing_number: None,
            status: None,
            mode: None,
            message: Some("TRACK LIMITS".to_owned()),
        });
        assert!(s.apply(other).is_empty());
    }
}
