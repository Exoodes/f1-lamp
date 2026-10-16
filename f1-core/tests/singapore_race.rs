//! Plays the 2026 Singapore GP race, captured live by the probe
//! (`f1-probe/captures/2026-10-11_13-40-12`) from before the start to
//! `Finalised`, through the replayer with a fake clock. The streams are
//! trimmed to the lines the lamp can use (`Stream::is_relevant`).

use std::time::{Duration, Instant};

use f1_core::{
    feed::Stream,
    input::{Input, RaceEvent, SessionPhase, TrackFlag},
    replay::{Replayer, Step},
};

const FILES: [(Stream, &str); 12] = [
    (
        Stream::SessionInfo,
        include_str!("data/2026-singapore/race/SessionInfo.jsonStream"),
    ),
    (
        Stream::TrackStatus,
        include_str!("data/2026-singapore/race/TrackStatus.jsonStream"),
    ),
    (
        Stream::SessionStatus,
        include_str!("data/2026-singapore/race/SessionStatus.jsonStream"),
    ),
    (
        Stream::RaceControl,
        include_str!("data/2026-singapore/race/RaceControlMessages.jsonStream"),
    ),
    (
        Stream::DriverList,
        include_str!("data/2026-singapore/race/DriverList.jsonStream"),
    ),
    (
        Stream::TopThree,
        include_str!("data/2026-singapore/race/TopThree.jsonStream"),
    ),
    (
        Stream::PitLane,
        include_str!("data/2026-singapore/race/PitLaneTimeCollection.jsonStream"),
    ),
    (
        Stream::TimingStats,
        include_str!("data/2026-singapore/race/TimingStats.jsonStream"),
    ),
    (
        Stream::LapCount,
        include_str!("data/2026-singapore/race/LapCount.jsonStream"),
    ),
    (
        Stream::Clock,
        include_str!("data/2026-singapore/race/ExtrapolatedClock.jsonStream"),
    ),
    // Only its lines about the pit lane and stopped or retired cars (#23
    // is marked retired after the flag).
    (
        Stream::TimingData,
        include_str!("data/2026-singapore/race/TimingData.jsonStream"),
    ),
    // Only its lines with positions.
    (
        Stream::DriverRaceInfo,
        include_str!("data/2026-singapore/race/DriverRaceInfo.jsonStream"),
    ),
];

/// Every input of the whole race, with the replay position it came at.
fn inputs() -> Vec<(Duration, Input)> {
    let mut now = Instant::now();
    let mut replayer = Replayer::new(&FILES, 200, now);
    replayer.play(now);
    let mut all = Vec::new();
    loop {
        match replayer.step(now) {
            Step::Send(inputs) => {
                let at = Duration::from_millis(replayer.status(now).position_ms);
                all.extend(inputs.into_iter().map(|i| (at, i)));
            }
            Step::Wait(wait) => now += wait,
            Step::Finished => return all,
            Step::Paused => panic!("paused while playing"),
            Step::Skipped(e) => panic!("{e}"),
        }
    }
}

fn events() -> Vec<(Duration, RaceEvent)> {
    inputs()
        .into_iter()
        .filter_map(|(at, i)| match i {
            Input::Race { event, .. } => Some((at, event)),
            _ => None,
        })
        .collect()
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// When `wanted` first came.
fn first(events: &[(Duration, RaceEvent)], wanted: RaceEvent) -> Duration {
    events
        .iter()
        .find(|(_, e)| *e == wanted)
        .map(|(at, _)| *at)
        .unwrap_or_else(|| panic!("no {wanted:?}"))
}

fn fastest_laps(events: &[(Duration, RaceEvent)]) -> Vec<(Duration, u8)> {
    events
        .iter()
        .filter_map(|(at, e)| match e {
            RaceEvent::FastestLap { driver, .. } => Some((*at, *driver)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_car_classified_as_retired_after_the_flag_doesnt_show() {
    // #23 was marked retired 83 s after `Finished`.
    assert!(!events()
        .iter()
        .any(|(_, e)| matches!(e, RaceEvent::Retired { .. })));
}

#[test]
fn the_start_lights_come_with_lights_out() {
    let events = events();
    // SessionStatus `Started` at 00:23:27.653.
    assert_eq!(first(&events, RaceEvent::StartLights), ms(1_407_653));
    assert_eq!(
        first(&events, RaceEvent::TrackFlag(TrackFlag::Green)),
        ms(1_407_653)
    );
}

#[test]
fn hamilton_beating_his_own_fastest_lap_counts() {
    // Lap 22 took P1 (1:37.170); lap 23 (1:37.075) came without a position,
    // which the lamp missed live on race day.
    let laps = fastest_laps(&events());
    assert!(laps.contains(&(ms(3_617_982), 44)), "{laps:?}");
    assert!(laps.contains(&(ms(3_714_671), 44)), "{laps:?}");
}

#[test]
fn the_race_has_fifteen_fastest_laps_and_twenty_five_pit_stops() {
    let events = events();
    let laps = fastest_laps(&events);
    assert_eq!(laps.len(), 15, "{laps:?}");
    // Laps 35-36, within three minutes: #3, #81, #6.
    let late: Vec<u8> = laps
        .iter()
        .filter(|(at, _)| *at > ms(4_800_000) && *at < ms(5_100_000))
        .map(|(_, driver)| *driver)
        .collect();
    assert_eq!(late, [3, 81, 6]);
    let stops = events
        .iter()
        .filter(|(_, e)| matches!(e, RaceEvent::PitStop { .. }))
        .count();
    assert_eq!(stops, 25);
}

#[test]
fn both_vscs_end_through_vsc_ending() {
    let events = events();
    let vsc: Vec<(Duration, RaceEvent)> = events
        .iter()
        .filter(|(_, e)| {
            matches!(
                e,
                RaceEvent::TrackFlag(TrackFlag::VirtualSafetyCar | TrackFlag::VscEnding)
            )
        })
        .copied()
        .collect();
    assert_eq!(
        vsc,
        [
            (
                ms(2_184_449),
                RaceEvent::TrackFlag(TrackFlag::VirtualSafetyCar)
            ),
            (ms(2_248_608), RaceEvent::TrackFlag(TrackFlag::VscEnding)),
            (
                ms(5_222_533),
                RaceEvent::TrackFlag(TrackFlag::VirtualSafetyCar)
            ),
            (ms(5_341_453), RaceEvent::TrackFlag(TrackFlag::VscEnding)),
        ]
    );
}

#[test]
fn the_race_ends_with_the_last_lap_the_chequered_flag_and_the_winner() {
    let events = events();
    let last_lap = first(&events, RaceEvent::LastLap);
    let chequered = first(&events, RaceEvent::ChequeredFlag);
    let (won, winner) = events
        .iter()
        .find_map(|(at, e)| match e {
            RaceEvent::Winner { driver, .. } => Some((*at, *driver)),
            _ => None,
        })
        .expect("a winner");
    assert!(last_lap < chequered && chequered <= won);
    assert_eq!(winner, 3);
    let phases: Vec<SessionPhase> = inputs()
        .into_iter()
        .filter_map(|(_, i)| match i {
            Input::Phase(p) => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(phases.last(), Some(&SessionPhase::PostSession));
}

fn overtakes(events: &[(Duration, RaceEvent)]) -> Vec<(Duration, u8)> {
    events
        .iter()
        .filter_map(|(at, e)| match e {
            RaceEvent::Overtake { driver, .. } => Some((*at, *driver)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_race_has_seventy_nine_overtakes_on_track() {
    let overtakes = overtakes(&events());
    assert_eq!(overtakes.len(), 79);
    // The first at the start: #55 from P13 to P8 at 00:23:37.393.
    assert_eq!(overtakes[0], (ms(1_417_393), 55));
}

#[test]
fn a_pit_stop_is_no_overtake() {
    // HAM in the pit lane from 00:53:24 to 00:54:16 (end of the first
    // sector): #12, #1 and #81 pass the pit entry, four cars the pit exit,
    // and HAM's stop counted puts him ahead of three: 7 changes of places.
    let during = overtakes(&events())
        .into_iter()
        .filter(|(at, _)| *at >= ms(3_204_000) && *at <= ms(3_257_000))
        .count();
    assert_eq!(during, 0);
}

#[test]
fn a_slow_car_on_track_is_overtaken() {
    // #5 slowed with a problem at 00:35:29 without pitting, and stopped at
    // 00:42:36: passing it counts, as in F1's own count.
    let passed: Vec<u8> = overtakes(&events())
        .into_iter()
        .filter(|(at, _)| *at >= ms(2_129_700) && *at <= ms(2_130_200))
        .map(|(_, driver)| driver)
        .collect();
    assert_eq!(passed, [11, 14, 6]);
}
