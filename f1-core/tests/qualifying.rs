//! Plays the archived 2026 Italian GP qualifying through the replayer, the
//! same way the lamp does, with a fake clock. The streams are trimmed to the
//! lines the lamp can use (as `build.rs` does for a replay build).

use std::{collections::HashSet, time::Instant};

use f1_core::{
    color::Rgb,
    feed::Stream,
    input::{Input, RaceEvent},
    replay::{Replayer, Step},
};

const FILES: [(Stream, &str); 9] = [
    (
        Stream::SessionInfo,
        include_str!("data/2026-italy/qualifying/SessionInfo.jsonStream"),
    ),
    (
        Stream::SessionData,
        include_str!("data/2026-italy/qualifying/SessionData.jsonStream"),
    ),
    (
        Stream::TrackStatus,
        include_str!("data/2026-italy/qualifying/TrackStatus.jsonStream"),
    ),
    (
        Stream::SessionStatus,
        include_str!("data/2026-italy/qualifying/SessionStatus.jsonStream"),
    ),
    (
        Stream::RaceControl,
        include_str!("data/2026-italy/qualifying/RaceControlMessages.jsonStream"),
    ),
    (
        Stream::DriverList,
        include_str!("data/2026-italy/qualifying/DriverList.jsonStream"),
    ),
    (
        Stream::TopThree,
        include_str!("data/2026-italy/qualifying/TopThree.jsonStream"),
    ),
    (
        Stream::TimingStats,
        include_str!("data/2026-italy/qualifying/TimingStats.jsonStream"),
    ),
    (
        Stream::TimingData,
        include_str!("data/2026-italy/qualifying/TimingData.jsonStream"),
    ),
];

const OUT_IN_Q1: [u8; 6] = [11, 14, 18, 22, 23, 77];
const OUT_IN_Q2: [u8; 6] = [5, 27, 30, 31, 55, 87];
const POLE: u8 = 10;

/// Every race event of the whole session, in order.
fn events() -> Vec<RaceEvent> {
    let mut now = Instant::now();
    let mut replayer = Replayer::new(&FILES, 200, now);
    replayer.play(now);
    let mut events = Vec::new();
    loop {
        match replayer.step(now) {
            Step::Send(inputs) => events.extend(inputs.into_iter().filter_map(|i| match i {
                Input::Race { event, .. } => Some(event),
                _ => None,
            })),
            Step::Wait(wait) => now += wait,
            Step::Finished => return events,
            Step::Paused => panic!("paused while playing"),
            Step::Skipped(e) => panic!("{e}"),
        }
    }
}

fn position(events: &[RaceEvent], f: impl Fn(&RaceEvent) -> bool) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, e)| f(e))
        .map(|(i, _)| i)
        .collect()
}

fn knocked_out(events: &[RaceEvent]) -> Vec<u8> {
    events
        .iter()
        .filter_map(|e| match e {
            RaceEvent::KnockedOut { driver, .. } => Some(*driver),
            _ => None,
        })
        .collect()
}

#[test]
fn each_part_ends_with_its_chequered_flag() {
    let events = events();
    let chequered = position(&events, |e| *e == RaceEvent::ChequeredFlag);
    assert_eq!(chequered.len(), 3);
}

#[test]
fn qualifying_has_no_start_lights() {
    assert!(!events().contains(&RaceEvent::StartLights));
}

#[test]
fn six_drivers_are_knocked_out_after_q1_and_six_after_q2() {
    let out = knocked_out(&events());
    assert_eq!(out.len(), 12);
    let after_q1: HashSet<u8> = out[..6].iter().copied().collect();
    let after_q2: HashSet<u8> = out[6..].iter().copied().collect();
    assert_eq!(after_q1, HashSet::from(OUT_IN_Q1));
    assert_eq!(after_q2, HashSet::from(OUT_IN_Q2));
}

#[test]
fn knockouts_come_between_their_part_and_the_next_one() {
    let events = events();
    let chequered = position(&events, |e| *e == RaceEvent::ChequeredFlag);
    let knockouts = position(&events, |e| matches!(e, RaceEvent::KnockedOut { .. }));
    assert!(knockouts[..6]
        .iter()
        .all(|&i| chequered[0] < i && i < chequered[1]));
    assert!(knockouts[6..]
        .iter()
        .all(|&i| chequered[1] < i && i < chequered[2]));
}

#[test]
fn each_part_shows_its_fastest_driver_and_pole_comes_last_in_team_colour() {
    let events = events();
    let winners: Vec<(usize, u8, Rgb)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            RaceEvent::Winner { driver, team_color } => Some((i, *driver, *team_color)),
            _ => None,
        })
        .collect();
    // Q1: car 10, Q2: car 12. Car 63 led when Q3's clock ran out; car 10
    // took pole on the last lap.
    let drivers: Vec<u8> = winners.iter().map(|&(_, driver, _)| driver).collect();
    assert_eq!(drivers, [10, 12, POLE]);
    assert!(winners.iter().all(|&(_, _, colour)| colour != Rgb::WHITE));
    // Each one after its own part's chequered flag.
    let chequered = position(&events, |e| *e == RaceEvent::ChequeredFlag);
    for (part, &(at, _, _)) in winners.iter().enumerate() {
        assert!(at > chequered[part], "Q{}", part + 1);
        if let Some(&next) = chequered.get(part + 1) {
            assert!(at < next, "Q{}", part + 1);
        }
    }
}

#[test]
fn fastest_laps_show_from_the_first_timed_lap() {
    let events = events();
    let first_fastest = position(&events, |e| matches!(e, RaceEvent::FastestLap { .. }));
    let first_chequered = position(&events, |e| *e == RaceEvent::ChequeredFlag)[0];
    assert!(!first_fastest.is_empty());
    assert!(first_fastest[0] < first_chequered);
}
