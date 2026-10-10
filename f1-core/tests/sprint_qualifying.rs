//! Plays the archived 2026 Singapore GP sprint qualifying through the
//! replayer, with a fake clock. The streams are trimmed to the lines the lamp
//! can use (as `build.rs` does for a replay build).

use std::time::{Duration, Instant};

use f1_core::{
    input::{Input, RaceEvent},
    replay::{Replayer, Step},
    timeline::Stream,
};

const FILES: [(Stream, &str); 8] = [
    (
        Stream::SessionInfo,
        include_str!("data/2026-singapore/sprint-qualifying/SessionInfo.jsonStream"),
    ),
    (
        Stream::SessionData,
        include_str!("data/2026-singapore/sprint-qualifying/SessionData.jsonStream"),
    ),
    (
        Stream::TrackStatus,
        include_str!("data/2026-singapore/sprint-qualifying/TrackStatus.jsonStream"),
    ),
    (
        Stream::SessionStatus,
        include_str!("data/2026-singapore/sprint-qualifying/SessionStatus.jsonStream"),
    ),
    (
        Stream::RaceControl,
        include_str!("data/2026-singapore/sprint-qualifying/RaceControlMessages.jsonStream"),
    ),
    (
        Stream::DriverList,
        include_str!("data/2026-singapore/sprint-qualifying/DriverList.jsonStream"),
    ),
    (
        Stream::TopThree,
        include_str!("data/2026-singapore/sprint-qualifying/TopThree.jsonStream"),
    ),
    (
        Stream::TimingData,
        include_str!("data/2026-singapore/sprint-qualifying/TimingData.jsonStream"),
    ),
];

/// SQ3's chequered flag (SessionStatus `Finished`), as a replay position.
const SQ3_FLAG: Duration = Duration::from_millis(4_710_148); // 01:18:30.148
/// The last car on track at the flag crossed the line at 01:19:58.
const LAST_CAR_DONE: Duration = Duration::from_millis(4_798_033);
/// `Finalised`, the old trigger.
const FINALISED: Duration = Duration::from_millis(5_841_722); // 01:37:21.722
const POLE: u8 = 3;

/// Every race event of the whole session, with the replay position it came at.
fn events() -> Vec<(Duration, RaceEvent)> {
    let mut now = Instant::now();
    let mut replayer = Replayer::new(&FILES, 200, now);
    replayer.play(now);
    let mut events = Vec::new();
    loop {
        match replayer.step(now) {
            Step::Send(inputs) => {
                let at = Duration::from_millis(replayer.status(now).position_ms);
                events.extend(inputs.into_iter().filter_map(|i| match i {
                    Input::Race { event, .. } => Some((at, event)),
                    _ => None,
                }));
            }
            Step::Wait(wait) => now += wait,
            Step::Finished => return events,
            Step::Paused => panic!("paused while playing"),
            Step::Skipped(e) => panic!("{e}"),
        }
    }
}

#[test]
fn pole_comes_when_the_last_car_has_finished_its_lap() {
    let winners: Vec<(Duration, u8)> = events()
        .into_iter()
        .filter_map(|(at, e)| match e {
            RaceEvent::Winner { driver, .. } => Some((at, driver)),
            _ => None,
        })
        .collect();
    let [(at, driver)] = winners[..] else {
        panic!("{winners:?}")
    };
    // Car 16 led at the flag; car 3 took pole 17 s later.
    assert_eq!(driver, POLE);
    assert!(at >= LAST_CAR_DONE, "{at:?}");
    assert!(at < SQ3_FLAG + Duration::from_secs(120), "{at:?}");
    assert!(at < FINALISED);
}
