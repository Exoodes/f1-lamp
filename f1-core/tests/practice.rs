//! Plays the last half hour of the 2026 Singapore GP's first practice,
//! captured live by the probe (`f1-probe/captures/2026-10-09_11-01-15`),
//! through the replayer with a fake clock. The streams are trimmed to the
//! lines the lamp can use (as `build.rs` does for a replay build).

use std::time::{Duration, Instant};

use f1_core::{
    feed::Stream,
    input::{Input, RaceEvent, SessionPhase, TrackFlag},
    replay::{Replayer, Step},
    tracker::LAST_LAPS_MAX,
};

const FILES: [(Stream, &str); 10] = [
    (
        Stream::SessionInfo,
        include_str!("data/2026-singapore/practice-1/SessionInfo.jsonStream"),
    ),
    (
        Stream::TrackStatus,
        include_str!("data/2026-singapore/practice-1/TrackStatus.jsonStream"),
    ),
    (
        Stream::SessionStatus,
        include_str!("data/2026-singapore/practice-1/SessionStatus.jsonStream"),
    ),
    (
        Stream::RaceControl,
        include_str!("data/2026-singapore/practice-1/RaceControlMessages.jsonStream"),
    ),
    (
        Stream::DriverList,
        include_str!("data/2026-singapore/practice-1/DriverList.jsonStream"),
    ),
    (
        Stream::TopThree,
        include_str!("data/2026-singapore/practice-1/TopThree.jsonStream"),
    ),
    (
        Stream::PitLane,
        include_str!("data/2026-singapore/practice-1/PitLaneTimeCollection.jsonStream"),
    ),
    (
        Stream::TimingStats,
        include_str!("data/2026-singapore/practice-1/TimingStats.jsonStream"),
    ),
    (
        Stream::Clock,
        include_str!("data/2026-singapore/practice-1/ExtrapolatedClock.jsonStream"),
    ),
    (
        Stream::TimingData,
        include_str!("data/2026-singapore/practice-1/TimingData.jsonStream"),
    ),
];

/// Every input of the whole session, with the replay position it came at.
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

#[test]
fn practice_has_no_start_lights() {
    assert!(!events().iter().any(|(_, e)| *e == RaceEvent::StartLights));
}

#[test]
fn the_fastest_driver_shows_after_the_last_laps() {
    let winners: Vec<(Duration, u8)> = events()
        .into_iter()
        .filter_map(|(at, e)| match e {
            RaceEvent::Winner { driver, .. } => Some((at, driver)),
            _ => None,
        })
        .collect();
    // #63 set the last fastest lap; `Finished` came at 00:28:45.079, and a
    // car still out then never showed a finished lap: at most two minutes.
    let [(at, 63)] = winners[..] else {
        panic!("{winners:?}")
    };
    let flag = ms(1_725_079);
    assert!(
        at > flag && at <= flag + LAST_LAPS_MAX + ms(1_500),
        "{at:?}"
    );
}

#[test]
fn the_vsc_ends_through_vsc_ending_to_green() {
    let flags: Vec<(Duration, RaceEvent)> = events()
        .into_iter()
        .filter(|(_, e)| matches!(e, RaceEvent::TrackFlag(_)))
        .collect();
    let vsc = flags
        .iter()
        .position(|(_, e)| *e == RaceEvent::TrackFlag(TrackFlag::VirtualSafetyCar))
        .expect("a VSC");
    assert_eq!(
        flags[vsc..vsc + 3],
        [
            (
                ms(502_775),
                RaceEvent::TrackFlag(TrackFlag::VirtualSafetyCar)
            ),
            (ms(571_957), RaceEvent::TrackFlag(TrackFlag::VscEnding)),
            (ms(586_848), RaceEvent::TrackFlag(TrackFlag::Green)),
        ]
    );
}

#[test]
fn the_chequered_flag_shows_when_practice_finishes() {
    let events = events();
    // `Finished` at 00:28:45.079 clears the flag; race control's CHEQUERED
    // FLAG follows a moment later.
    let cleared = events
        .iter()
        .rfind(|(_, e)| *e == RaceEvent::FlagCleared)
        .map(|(at, _)| *at);
    assert_eq!(cleared, Some(ms(1_725_079)));
    let chequered = events
        .iter()
        .find(|(_, e)| *e == RaceEvent::ChequeredFlag)
        .map(|(at, _)| *at)
        .expect("a chequered flag");
    assert!(
        chequered >= ms(1_725_079) && chequered < ms(1_726_000),
        "{chequered:?}"
    );
}

#[test]
fn practice_is_over_at_finalised_not_at_finished() {
    let phases: Vec<(Duration, SessionPhase)> = inputs()
        .into_iter()
        .filter_map(|(at, i)| match i {
            Input::Phase(p) => Some((at, p)),
            _ => None,
        })
        .collect();
    // Joined while it ran; `Finalised` at 00:38:34.666.
    assert_eq!(
        phases.last().copied(),
        Some((ms(2_314_707), SessionPhase::PostSession))
    );
    assert!(phases.contains(&(ms(72), SessionPhase::Live)));
}

#[test]
fn only_real_pit_stops_show() {
    // Most practice pit-lane entries have an empty duration or a long one
    // (a car parked between runs); only these are stops.
    let stops: Vec<u8> = events()
        .into_iter()
        .filter_map(|(_, e)| match e {
            RaceEvent::PitStop { driver, .. } => Some(driver),
            _ => None,
        })
        .collect();
    assert_eq!(stops, [31, 44, 43, 30]);
}
