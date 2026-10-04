//! Plays the archived 2026 Italian GP race through the replayer, the same way
//! the lamp does, with a fake clock.

use std::time::{Duration, Instant};

use f1_core::{
    input::{Input, RaceEvent, SessionPhase, TrackFlag},
    replay::{Replayer, Step},
    timeline::Stream,
};

const FILES: [(Stream, &str); 6] = [
    (
        Stream::SessionInfo,
        include_str!("data/2026-italy/race/SessionInfo.jsonStream"),
    ),
    (
        Stream::TrackStatus,
        include_str!("data/2026-italy/race/TrackStatus.jsonStream"),
    ),
    (
        Stream::SessionStatus,
        include_str!("data/2026-italy/race/SessionStatus.jsonStream"),
    ),
    (
        Stream::RaceControl,
        include_str!("data/2026-italy/race/RaceControlMessages.jsonStream"),
    ),
    (
        Stream::DriverList,
        include_str!("data/2026-italy/race/DriverList.jsonStream"),
    ),
    (
        Stream::TopThree,
        include_str!("data/2026-italy/race/TopThree.jsonStream"),
    ),
];

fn hms(h: u64, m: u64, s: u64) -> Duration {
    Duration::from_secs((h * 60 + m) * 60 + s)
}

/// Plays until finished, moving the clock forward by each wait, like the
/// replay thread. Returns every input and the real time it took.
fn play_to_end(speed: u32) -> (Vec<Input>, Duration) {
    let t0 = Instant::now();
    let mut now = t0;
    let mut replayer = Replayer::new(&FILES, speed, now);
    replayer.play(now);
    let mut inputs = Vec::new();
    loop {
        match replayer.step(now) {
            Step::Send(more) => inputs.extend(more),
            Step::Wait(wait) => now += wait,
            Step::Finished => return (inputs, now - t0),
            Step::Paused => panic!("paused while playing"),
            Step::Skipped(e) => panic!("{e}"),
        }
    }
}

fn flags(inputs: &[Input]) -> Vec<TrackFlag> {
    inputs
        .iter()
        .filter_map(|i| match i {
            Input::Race {
                event: RaceEvent::TrackFlag(f),
                ..
            } => Some(*f),
            _ => None,
        })
        .collect()
}

#[test]
fn replay_starts_ten_seconds_before_lights_out() {
    let r = Replayer::new(&FILES, 60, Instant::now());
    let status = r.status(Instant::now());
    assert_eq!(status.lights_out_ms, Some(3_415_761));
    assert_eq!(status.position_ms, 3_405_761);
}

#[test]
fn race_at_60x_takes_under_two_minutes() {
    let (_, took) = play_to_end(60);
    // 00:56:45 to 02:55:19 of feed is 118.6 min; at 60x that's 118.6 s.
    assert!(
        took > Duration::from_secs(115) && took < Duration::from_secs(120),
        "{took:?}"
    );
}

#[test]
fn replayed_race_gives_the_same_flags_as_the_track_state_test() {
    use TrackFlag::*;
    let (inputs, _) = play_to_end(60);
    assert_eq!(
        flags(&inputs),
        [
            Green,
            Yellow,
            DoubleYellow,
            SafetyCar,
            Red,
            Green,
            Yellow,
            Green,
            Yellow,
            VirtualSafetyCar,
            Green
        ]
    );
}

#[test]
fn replayed_race_goes_pre_session_live_post_session() {
    let (inputs, _) = play_to_end(60);
    let phases: Vec<SessionPhase> = inputs
        .iter()
        .filter_map(|i| match i {
            Input::Phase(p) => Some(*p),
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        [
            SessionPhase::PreSession,
            SessionPhase::Live,
            SessionPhase::PostSession
        ]
    );
}

#[test]
fn replayed_race_has_start_lights_and_chequered_flag_once() {
    let (inputs, _) = play_to_end(60);
    let count = |wanted: RaceEvent| {
        inputs
            .iter()
            .filter(|i| matches!(i, Input::Race { event, .. } if *event == wanted))
            .count()
    };
    assert_eq!(count(RaceEvent::StartLights), 1);
    assert_eq!(count(RaceEvent::ChequeredFlag), 1);
}

#[test]
fn jump_into_the_red_flag_shows_red_at_once() {
    let t0 = Instant::now();
    let mut r = Replayer::new(&FILES, 60, t0);
    r.seek(hms(1, 15, 0), t0);
    let Step::Send(inputs) = r.step(t0) else {
        panic!("expected inputs")
    };
    assert_eq!(inputs[0], Input::Phase(SessionPhase::Live));
    assert_eq!(flags(&inputs), [TrackFlag::Red]);
}

#[test]
fn antonelli_wins_in_mercedes_teal_after_the_chequered_flag() {
    let (inputs, _) = play_to_end(60);
    let events: Vec<RaceEvent> = inputs
        .iter()
        .filter_map(|i| match i {
            Input::Race { event, .. } => Some(*event),
            _ => None,
        })
        .collect();
    let chequered = events
        .iter()
        .position(|e| *e == RaceEvent::ChequeredFlag)
        .unwrap();
    assert_eq!(
        events[chequered + 1..],
        [RaceEvent::Winner {
            driver: 12,
            team_color: "#00d7b6".parse().unwrap(),
        }]
    );
}

#[test]
fn winner_comes_while_still_live() {
    let (inputs, _) = play_to_end(60);
    let winner = inputs
        .iter()
        .position(|i| {
            matches!(
                i,
                Input::Race {
                    event: RaceEvent::Winner { .. },
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(
        inputs[winner + 1..],
        [Input::Phase(SessionPhase::PostSession)]
    );
}

#[test]
fn jump_past_the_finish_doesnt_show_the_winner_again() {
    let t0 = Instant::now();
    let mut r = Replayer::new(&FILES, 60, t0);
    r.seek(hms(2, 50, 0), t0);
    let Step::Send(inputs) = r.step(t0) else {
        panic!("expected inputs")
    };
    assert!(!inputs.iter().any(|i| matches!(
        i,
        Input::Race {
            event: RaceEvent::Winner { .. },
            ..
        }
    )));
}
