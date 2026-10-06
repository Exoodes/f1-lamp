//! Runs the whole archived 2026 Italian GP race through the track-state
//! machine, with the three streams merged by time as the live feed sends them.

use std::time::Duration;

use f1_core::{
    input::{RaceEvent, TrackFlag},
    timeline::{Stream, Timeline},
    track_state::TrackState,
};

const TRACK_STATUS: &str = include_str!("data/2026-italy/race/TrackStatus.jsonStream");
const SESSION_STATUS: &str = include_str!("data/2026-italy/race/SessionStatus.jsonStream");
const RACE_CONTROL: &str = include_str!("data/2026-italy/race/RaceControlMessages.jsonStream");

/// Every event the race produces, with its offset. The merge is the same
/// `Timeline` the replay on the lamp uses.
fn race_events() -> Vec<(Duration, RaceEvent)> {
    let files = [
        (Stream::TrackStatus, TRACK_STATUS),
        (Stream::SessionStatus, SESSION_STATUS),
        (Stream::RaceControl, RACE_CONTROL),
    ];
    let mut state = TrackState::default();
    let mut events = Vec::new();
    for item in Timeline::new(&files) {
        let (t, msg) = item.unwrap();
        for event in state.apply(msg) {
            events.push((t, event));
        }
    }
    events
}

fn flags() -> Vec<TrackFlag> {
    race_events()
        .into_iter()
        .filter_map(|(_, e)| match e {
            RaceEvent::TrackFlag(f) => Some(f),
            _ => None,
        })
        .collect()
}

fn hms(h: u64, m: u64, s: u64, ms: u64) -> Duration {
    Duration::from_millis(((h * 60 + m) * 60 + s) * 1000 + ms)
}

#[test]
fn whole_race_gives_the_expected_events_at_the_right_times() {
    use RaceEvent::{ChequeredFlag, FlagCleared, StartLights};
    use TrackFlag::*;
    let flag = RaceEvent::TrackFlag;
    assert_eq!(
        race_events(),
        [
            (hms(0, 56, 55, 761), StartLights),
            (hms(0, 56, 55, 761), flag(Green)),
            (hms(0, 59, 57, 947), flag(Yellow)),
            (hms(0, 59, 58, 87), flag(DoubleYellow)),
            (hms(1, 0, 14, 412), flag(SafetyCar)),
            (hms(1, 1, 7, 587), flag(Red)),
            (hms(1, 32, 24, 927), flag(Green)),
            (hms(1, 39, 58, 424), flag(Yellow)),
            (hms(1, 40, 5, 991), flag(Green)),
            (hms(2, 10, 55, 234), flag(Yellow)),
            (hms(2, 11, 9, 379), flag(VirtualSafetyCar)),
            (hms(2, 13, 6, 880), flag(Green)),
            (hms(2, 48, 10, 829), ChequeredFlag),
            (hms(2, 48, 11, 236), FlagCleared),
        ]
    );
}

#[test]
fn every_safety_car_vsc_and_red_ends_in_green() {
    let flags = flags();
    for (i, f) in flags.iter().enumerate() {
        if matches!(
            f,
            TrackFlag::SafetyCar | TrackFlag::VirtualSafetyCar | TrackFlag::Red
        ) {
            // Only neutralisations or a red flag may come before the next green.
            let next_green = flags[i..]
                .iter()
                .position(|f| *f == TrackFlag::Green)
                .unwrap_or_else(|| panic!("{f:?} at {i} never ends in green"));
            assert!(
                flags[i..i + next_green].iter().all(|f| matches!(
                    f,
                    TrackFlag::SafetyCar | TrackFlag::VirtualSafetyCar | TrackFlag::Red
                )),
                "{:?}",
                &flags[i..=i + next_green]
            );
        }
    }
}

#[test]
fn the_race_ends_with_the_chequered_flag_then_no_flag() {
    let events: Vec<RaceEvent> = race_events().into_iter().map(|(_, e)| e).collect();
    assert_eq!(
        events[events.len() - 2..],
        [RaceEvent::ChequeredFlag, RaceEvent::FlagCleared]
    );
}

#[test]
fn start_lights_come_once() {
    let starts = race_events()
        .iter()
        .filter(|(_, e)| *e == RaceEvent::StartLights)
        .count();
    assert_eq!(starts, 1);
}

#[test]
fn no_flag_is_sent_twice_in_a_row() {
    let flags = flags();
    for pair in flags.windows(2) {
        assert_ne!(pair[0], pair[1]);
    }
}

#[test]
fn red_flag_stoppage_stays_red_for_31_minutes() {
    let events = race_events();
    let red = events
        .iter()
        .position(|(_, e)| *e == RaceEvent::TrackFlag(TrackFlag::Red))
        .unwrap();
    let (red_at, _) = events[red];
    let (next_at, next) = events[red + 1];
    assert_eq!(next, RaceEvent::TrackFlag(TrackFlag::Green));
    assert!(next_at - red_at > Duration::from_secs(31 * 60));
}
