//! Which track flag is out, from the feed's session status, track status and
//! race control messages. Reports each change as a [`RaceEvent`], plus the
//! start lights and the chequered flag.

use std::collections::HashSet;

use crate::{
    feed::{FeedMessage, LapCount, RcMessage, SessionState, SessionStatus, SessionType, TrackCode},
    input::{RaceEvent, TrackFlag},
};

#[derive(Debug, Default)]
pub struct TrackState {
    session: Option<SessionState>,
    track: Option<TrackCode>,
    double_yellow: HashSet<u8>,
    shown: Option<TrackFlag>,
    /// The first `Started` came while connected: the start, with or without
    /// start lights. Later ones (a restart after a red flag) aren't starts.
    first_start_seen: bool,
    /// The feed says the session began, maybe before the lamp connected
    /// (`SessionStatus.Started`).
    begun: bool,
    session_type: SessionType,
    /// Stopped by a red flag (`Aborted`) and not restarted yet: before the
    /// restart the session goes `Inactive`, still under the red flag.
    suspended: bool,
    current_lap: Option<u32>,
    total_laps: Option<u32>,
    last_lap_sent: bool,
}

impl TrackState {
    /// Reads one message; returns the events it causes: at most one of its
    /// own (start lights, chequered flag), then a change of flag.
    pub fn apply(&mut self, msg: FeedMessage) -> Vec<RaceEvent> {
        let own = match msg {
            FeedMessage::Track(t) => {
                // A code the lamp doesn't know changes nothing: better the
                // last flag than a green that may be wrong.
                if let Some(code) = t.code().filter(|&c| c != TrackCode::Unknown) {
                    self.track = Some(code);
                }
                None
            }
            FeedMessage::Session(s) => self.apply_session(&s),
            FeedMessage::RaceControl(m) => self.apply_race_control(&m),
            FeedMessage::LapCount(laps) => self.apply_lap_count(&laps),
            // Only for the page's TV clock (see `live`).
            FeedMessage::Clock(_) => None,
            FeedMessage::SessionInfo(info) => {
                if let Some(kind) = info.session_type() {
                    self.session_type = kind;
                }
                None
            }
            FeedMessage::DriverList(_)
            | FeedMessage::TopThree(_)
            | FeedMessage::PitLane(_)
            | FeedMessage::TimingStats(_)
            | FeedMessage::Overtakes(_)
            | FeedMessage::SessionData(_)
            | FeedMessage::TimingData(_) => None,
        };
        own.into_iter().chain(self.flag_change()).collect()
    }

    /// The session's status. Its first `Started` is the start: the start
    /// lights, unless it's known not to be a race.
    fn apply_session(&mut self, s: &SessionStatus) -> Option<RaceEvent> {
        if s.started.is_some_and(|st| st != SessionState::Inactive) {
            self.begun = true;
        }
        let state = s.status?;
        self.session = Some(state);
        match state {
            SessionState::Aborted => self.suspended = true,
            SessionState::Inactive => {}
            _ => self.suspended = false,
        }
        if state != SessionState::Started || self.first_start_seen {
            return None;
        }
        self.first_start_seen = true;
        self.may_be_race().then_some(RaceEvent::StartLights)
    }

    /// The leader starting the last lap, once: the current lap reaches the
    /// race's length, also when the race is shortened to the current lap.
    /// The feed sends 0 of 0 laps for a moment before a race.
    fn apply_lap_count(&mut self, laps: &LapCount) -> Option<RaceEvent> {
        if laps.current_lap.is_some() {
            self.current_lap = laps.current_lap;
        }
        if laps.total_laps.is_some() {
            self.total_laps = laps.total_laps;
        }
        let last = matches!(
            (self.current_lap, self.total_laps),
            (Some(lap), Some(total)) if total > 0 && lap >= total
        );
        let racing = self.session == Some(SessionState::Started) && self.may_be_race();
        if !last || !racing || self.last_lap_sent {
            return None;
        }
        self.last_lap_sent = true;
        Some(RaceEvent::LastLap)
    }

    /// Race control's flags: double yellows by sector (shown while any sector
    /// has one), and the chequered flag.
    fn apply_race_control(&mut self, m: &RcMessage) -> Option<RaceEvent> {
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
            (Some("Flag"), Some("CHEQUERED"), _) => return Some(RaceEvent::ChequeredFlag),
            _ => {}
        }
        None
    }

    /// The flag to show now, if it differs from the one shown: the flag, or
    /// `FlagCleared` when there is none any more.
    fn flag_change(&mut self) -> Option<RaceEvent> {
        let new = self.derive();
        if new == self.shown {
            return None;
        }
        self.shown = new;
        Some(new.map_or(RaceEvent::FlagCleared, RaceEvent::TrackFlag))
    }

    /// A race, or not known yet: the start lights and the last lap would
    /// rather show in a session they don't belong to than be missed in a
    /// race whose SessionInfo didn't come.
    fn may_be_race(&self) -> bool {
        !matches!(
            self.session_type,
            SessionType::Qualifying | SessionType::Practice
        )
    }

    pub fn flag(&self) -> Option<TrackFlag> {
        self.shown
    }

    /// Whether the session still goes on: started and not over. Qualifying
    /// is `Finished` between its parts, so for anything but a race only
    /// `Finalised` and `Ends` mean over.
    pub fn session_running(&self) -> bool {
        use SessionState::{Aborted, Finished, Inactive, Started};

        match self.session {
            Some(Started | Aborted) => true,
            // Between qualifying's parts, or before a restart after a red
            // flag; also when the lamp connected only then.
            Some(Inactive) => self.first_start_seen || self.begun,
            Some(Finished) => !self.may_be_race(),
            _ => false,
        }
    }

    /// The flag the session's state and the track status mean together. A
    /// suspended session (`Aborted`, then `Inactive` until the restart) is
    /// red whatever the track status says. While the session runs the track
    /// status decides, a yellow becoming a double yellow while any sector
    /// has one. Before the start and after the finish there is no flag.
    fn derive(&self) -> Option<TrackFlag> {
        use SessionState::Started;

        match (self.session, self.track) {
            _ if self.suspended => Some(TrackFlag::Red),
            (Some(Started), Some(TrackCode::Red)) => Some(TrackFlag::Red),
            (Some(Started), Some(TrackCode::ScDeployed)) => Some(TrackFlag::SafetyCar),
            (Some(Started), Some(TrackCode::VscDeployed)) => Some(TrackFlag::VirtualSafetyCar),
            (Some(Started), Some(TrackCode::VscEnding)) => Some(TrackFlag::VscEnding),
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
    use crate::feed::{SessionInfo, TrackStatus};

    fn track(code: &str) -> FeedMessage {
        FeedMessage::Track(TrackStatus {
            status: Some(code.to_owned()),
            message: None,
        })
    }

    fn session(state: SessionState) -> FeedMessage {
        FeedMessage::Session(SessionStatus {
            status: Some(state),
            started: None,
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
    fn an_unknown_track_code_keeps_the_last_flag() {
        let mut s = racing();
        s.apply(track("4"));
        assert!(s.apply(track("3")).is_empty());
        assert!(s.apply(track("9")).is_empty());
        assert_eq!(s.flag(), Some(TrackFlag::SafetyCar));
        // A known code afterwards still counts.
        assert_eq!(s.apply(track("1")), flag(TrackFlag::Green));
    }

    #[test]
    fn an_unknown_track_code_before_any_other_still_starts_green() {
        let mut s = TrackState::default();
        s.apply(track("3"));
        assert_eq!(
            s.apply(session(SessionState::Started)),
            [
                RaceEvent::StartLights,
                RaceEvent::TrackFlag(TrackFlag::Green)
            ]
        );
    }

    #[test]
    fn finishing_under_a_safety_car_clears_the_flag() {
        let mut s = racing();
        assert_eq!(s.apply(track("4")), flag(TrackFlag::SafetyCar));
        assert_eq!(
            s.apply(session(SessionState::Finished)),
            [RaceEvent::FlagCleared]
        );
        assert_eq!(s.flag(), None);
    }

    #[test]
    fn a_cleared_flag_is_reported_once() {
        let mut s = racing();
        s.apply(session(SessionState::Finished));
        assert!(s.apply(session(SessionState::Finalised)).is_empty());
        assert!(s.apply(track("1")).is_empty());
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
            ("7", TrackFlag::VscEnding),
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
    fn unknown_track_code_keeps_a_yellow() {
        let mut s = racing();
        s.apply(track("2"));
        assert!(s.apply(track("3")).is_empty());
        assert_eq!(s.flag(), Some(TrackFlag::Yellow));
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
    fn vsc_ending_follows_the_vsc_then_green() {
        let mut s = racing();
        assert_eq!(s.apply(track("6")), flag(TrackFlag::VirtualSafetyCar));
        assert_eq!(s.apply(track("7")), flag(TrackFlag::VscEnding));
        assert_eq!(s.apply(track("1")), flag(TrackFlag::Green));
    }

    #[test]
    fn red_stays_until_the_restart_through_inactive() {
        // Singapore 2026 sprint qualifying: Aborted, all clear on track,
        // Inactive ("SQ1 WILL RESUME AT 20:46"), then Started.
        let mut s = racing();
        s.apply(session(SessionState::Aborted));
        s.apply(track("1"));
        assert!(s.apply(session(SessionState::Inactive)).is_empty());
        assert_eq!(s.flag(), Some(TrackFlag::Red));
        assert_eq!(
            s.apply(session(SessionState::Started)),
            flag(TrackFlag::Green)
        );
    }

    #[test]
    fn inactive_between_qualifying_parts_has_no_flag() {
        let mut s = racing();
        s.apply(session(SessionState::Finished));
        s.apply(session(SessionState::Inactive));
        assert_eq!(s.flag(), None);
    }

    #[test]
    fn a_session_ended_under_red_has_no_flag() {
        let mut s = racing();
        s.apply(session(SessionState::Aborted));
        s.apply(session(SessionState::Inactive));
        assert_eq!(
            s.apply(session(SessionState::Finished)),
            [RaceEvent::FlagCleared]
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

    fn laps(current: Option<u32>, total: Option<u32>) -> FeedMessage {
        FeedMessage::LapCount(LapCount {
            current_lap: current,
            total_laps: total,
        })
    }

    #[test]
    fn the_last_lap_comes_once_when_the_leader_starts_it() {
        let mut s = racing();
        assert!(s.apply(laps(Some(1), Some(53))).is_empty());
        assert!(s.apply(laps(Some(52), None)).is_empty());
        assert_eq!(s.apply(laps(Some(53), None)), [RaceEvent::LastLap]);
        assert!(s.apply(laps(Some(53), None)).is_empty());
    }

    #[test]
    fn a_shortened_race_can_start_its_last_lap_at_once() {
        // Singapore 2026 sprint: 21 laps became 20 after a delayed start.
        let mut s = racing();
        s.apply(laps(Some(19), Some(21)));
        assert_eq!(s.apply(laps(None, Some(19))), [RaceEvent::LastLap]);
    }

    #[test]
    fn zero_of_zero_laps_before_a_race_is_not_the_last_lap() {
        let mut s = racing();
        assert!(s.apply(laps(Some(0), Some(0))).is_empty());
    }

    #[test]
    fn no_last_lap_before_the_start_or_outside_a_race() {
        let mut s = TrackState::default();
        assert!(s.apply(laps(Some(1), Some(1))).is_empty());
        let mut s = TrackState::default();
        s.apply(info("Practice"));
        s.apply(session(SessionState::Started));
        assert!(s.apply(laps(Some(1), Some(1))).is_empty());
    }

    fn info(kind: &str) -> FeedMessage {
        FeedMessage::SessionInfo(SessionInfo {
            kind: Some(kind.to_owned()),
            name: None,
            key: None,
        })
    }

    #[test]
    fn a_start_without_session_info_has_start_lights() {
        // Better start lights in a session they don't belong to than none
        // in a race whose SessionInfo didn't come.
        let mut s = TrackState::default();
        assert!(s
            .apply(session(SessionState::Started))
            .contains(&RaceEvent::StartLights));
    }

    #[test]
    fn race_start_has_start_lights() {
        let mut s = TrackState::default();
        s.apply(info("Race"));
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
    fn a_session_runs_from_the_start_until_it_is_over() {
        let mut s = TrackState::default();
        s.apply(info("Race"));
        assert!(!s.session_running());
        s.apply(session(SessionState::Inactive));
        assert!(!s.session_running());
        s.apply(session(SessionState::Started));
        assert!(s.session_running());
        s.apply(session(SessionState::Aborted));
        assert!(s.session_running());
        s.apply(session(SessionState::Finished));
        assert!(!s.session_running());
    }

    #[test]
    fn a_red_flag_counts_as_running_even_when_joined_during_it() {
        let mut s = TrackState::default();
        s.apply(session(SessionState::Aborted));
        assert!(s.session_running());
    }

    #[test]
    fn qualifying_runs_between_its_parts_until_finalised() {
        let mut s = TrackState::default();
        s.apply(info("Qualifying"));
        s.apply(session(SessionState::Started));
        s.apply(session(SessionState::Finished));
        assert!(s.session_running());
        s.apply(session(SessionState::Inactive));
        assert!(s.session_running());
        s.apply(session(SessionState::Finalised));
        assert!(!s.session_running());
    }

    #[test]
    fn qualifying_joined_between_its_parts_is_running() {
        // Monza 2026, between Q1 and Q2.
        let mut s = TrackState::default();
        s.apply(info("Qualifying"));
        let between: SessionStatus =
            serde_json::from_str(r#"{"Status":"Inactive","Started":"Finished"}"#).unwrap();
        s.apply(FeedMessage::Session(between));
        assert!(s.session_running());
    }

    #[test]
    fn a_session_joined_before_its_start_is_not_running() {
        let mut s = TrackState::default();
        s.apply(info("Qualifying"));
        let before: SessionStatus =
            serde_json::from_str(r#"{"Status":"Inactive","Started":"Inactive"}"#).unwrap();
        s.apply(FeedMessage::Session(before));
        assert!(!s.session_running());
    }

    #[test]
    fn qualifying_and_practice_start_without_start_lights() {
        for kind in ["Qualifying", "Practice"] {
            let mut s = TrackState::default();
            s.apply(info(kind));
            s.apply(track("1"));
            assert_eq!(
                s.apply(session(SessionState::Started)),
                flag(TrackFlag::Green),
                "{kind}"
            );
        }
    }
}
