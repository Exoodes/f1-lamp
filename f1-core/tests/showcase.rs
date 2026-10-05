//! The hand-written showcase race (`data/showcase/`), which `--features
//! showcase` plays on the lamp: every flag and the winner in 2.5 minutes.

use std::time::{Duration, Instant};

use f1_core::{
    input::{Input, RaceEvent, SessionPhase, TrackFlag},
    replay::{Replayer, Step},
    timeline::{Stream, Timeline},
};

const FILES: [(Stream, &str); 9] = [
    (
        Stream::SessionInfo,
        include_str!("data/showcase/SessionInfo.jsonStream"),
    ),
    (
        Stream::TrackStatus,
        include_str!("data/showcase/TrackStatus.jsonStream"),
    ),
    (
        Stream::SessionStatus,
        include_str!("data/showcase/SessionStatus.jsonStream"),
    ),
    (
        Stream::RaceControl,
        include_str!("data/showcase/RaceControlMessages.jsonStream"),
    ),
    (
        Stream::DriverList,
        include_str!("data/showcase/DriverList.jsonStream"),
    ),
    (
        Stream::TopThree,
        include_str!("data/showcase/TopThree.jsonStream"),
    ),
    (
        Stream::PitLane,
        include_str!("data/showcase/PitLaneTimeCollection.jsonStream"),
    ),
    (
        Stream::TimingStats,
        include_str!("data/showcase/TimingStats.jsonStream"),
    ),
    (
        Stream::Overtakes,
        include_str!("data/showcase/OvertakeSeries.jsonStream"),
    ),
];

fn colour(hex: &str) -> f1_core::color::Rgb {
    hex.parse().unwrap()
}

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s)
}

/// Plays at real time with a fake clock; every input with its offset from
/// the start of the files.
fn play() -> (Vec<(Duration, Input)>, Duration) {
    let t0 = Instant::now();
    let mut now = t0;
    let mut r = Replayer::new(&FILES, 1, now);
    let start = Duration::from_millis(r.status(now).position_ms);
    r.play(now);
    let mut inputs = Vec::new();
    loop {
        match r.step(now) {
            Step::Send(more) => inputs.extend(more.into_iter().map(|i| (start + (now - t0), i))),
            Step::Wait(wait) => now += wait,
            Step::Finished => return (inputs, now - t0),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn every_line_parses() {
    for item in Timeline::new(&FILES) {
        item.unwrap();
    }
}

#[test]
fn showcase_plays_every_flag_and_the_winner_in_order() {
    use RaceEvent::{ChequeredFlag, FastestLap, Overtake, StartLights, Winner};
    use TrackFlag::*;
    let flag = RaceEvent::TrackFlag;
    let events: Vec<(Duration, RaceEvent)> = play()
        .0
        .into_iter()
        .filter_map(|(t, i)| match i {
            // The pit window has a test of its own.
            Input::Race {
                event: RaceEvent::PitStop { .. },
                ..
            } => None,
            Input::Race { event, .. } => Some((t, event)),
            _ => None,
        })
        .collect();
    assert_eq!(
        events,
        [
            (secs(20.0), StartLights),
            (secs(20.0), flag(Green)),
            (secs(35.0), flag(Yellow)),
            (secs(42.0), flag(DoubleYellow)),
            (secs(50.5), flag(Green)),
            // Not the lap-2 best at 30 s: the first timed lap is ignored.
            (
                secs(58.0),
                FastestLap {
                    driver: 16,
                    team_color: colour("#ed1131")
                }
            ),
            (secs(60.5), flag(SafetyCar)),
            (
                secs(72.0),
                Overtake {
                    driver: 16,
                    team_color: colour("#ed1131")
                }
            ),
            (secs(75.0), flag(Green)),
            (secs(85.5), flag(VirtualSafetyCar)),
            (secs(100.0), flag(Green)),
            (
                secs(102.0),
                FastestLap {
                    driver: 12,
                    team_color: colour("#00d7b6")
                }
            ),
            (secs(110.0), flag(Red)),
            (secs(130.0), flag(Green)),
            (
                secs(135.0),
                Overtake {
                    driver: 12,
                    team_color: colour("#00d7b6")
                }
            ),
            (secs(150.0), ChequeredFlag),
            (
                secs(150.5),
                Winner {
                    driver: 12,
                    team_color: "#00d7b6".parse().unwrap(),
                }
            ),
        ]
    );
}

#[test]
fn showcase_goes_pre_session_live_post_session() {
    let phases: Vec<(Duration, SessionPhase)> = play()
        .0
        .into_iter()
        .filter_map(|(t, i)| match i {
            Input::Phase(p) => Some((t, p)),
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        [
            (secs(10.0), SessionPhase::PreSession),
            (secs(20.0), SessionPhase::Live),
            (secs(161.0), SessionPhase::PostSession),
        ]
    );
}

#[test]
fn showcase_takes_two_and_a_half_minutes_at_real_time() {
    let (_, took) = play();
    assert_eq!(took, secs(151.0));
}

#[test]
fn pit_window_under_the_safety_car_shows_every_team_colour() {
    let pit_stops: Vec<(Duration, u8, f1_core::color::Rgb)> = play()
        .0
        .into_iter()
        .filter_map(|(t, i)| match i {
            Input::Race {
                event: RaceEvent::PitStop { driver, team_color },
                ..
            } => Some((t, driver, team_color)),
            _ => None,
        })
        .collect();
    let drivers: Vec<u8> = pit_stops.iter().map(|(_, d, _)| *d).collect();
    assert_eq!(drivers, [81, 12, 16, 3, 14, 10, 23, 22, 5, 31, 77]);

    // The 11 official 2026 colours, one per team.
    let mut colours: Vec<String> = pit_stops.iter().map(|(_, _, c)| c.to_string()).collect();
    colours.sort();
    colours.dedup();
    assert_eq!(colours.len(), 11);

    // All under the safety car (60.5 s to 75 s), 1.2 s apart: each 0.6 s
    // flash is over before the next, so the overlay queue never fills.
    for (t, _, _) in &pit_stops {
        assert!(*t > secs(60.5) && *t < secs(75.0), "{t:?}");
    }
    for pair in pit_stops.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= secs(1.2) - Duration::from_millis(1));
    }
}
