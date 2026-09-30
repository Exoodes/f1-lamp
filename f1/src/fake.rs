use std::{
    sync::mpsc::SyncSender,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use f1_core::{
    color::Rgb,
    input::{Input, NetStatus, RaceEvent, SessionPhase, TrackFlag},
};

const TEAM: Rgb = Rgb::new(0, 210, 190);

/// Each race event and how long to wait before sending the next one.
const SCRIPT: [(RaceEvent, Duration); 9] = [
    (RaceEvent::StartLights, Duration::from_secs(7)),
    (
        RaceEvent::TrackFlag(TrackFlag::Green),
        Duration::from_secs(3),
    ),
    (
        RaceEvent::FastestLap {
            driver: 44,
            team_color: TEAM,
        },
        Duration::from_secs(4),
    ),
    (
        RaceEvent::TrackFlag(TrackFlag::Yellow),
        Duration::from_secs(4),
    ),
    (
        RaceEvent::TrackFlag(TrackFlag::SafetyCar),
        Duration::from_secs(6),
    ),
    (
        RaceEvent::PitStop {
            driver: 44,
            team_color: TEAM,
        },
        Duration::from_secs(4),
    ),
    (
        RaceEvent::TrackFlag(TrackFlag::VirtualSafetyCar),
        Duration::from_secs(6),
    ),
    (RaceEvent::TrackFlag(TrackFlag::Red), Duration::from_secs(4)),
    (RaceEvent::ChequeredFlag, Duration::from_secs(11)),
];

/// Spawns a thread that reports the lamp as online and live, then plays `SCRIPT` forever.
pub fn spawn(tx: SyncSender<Input>) -> anyhow::Result<JoinHandle<()>> {
    let handle = thread::Builder::new()
        .name("fake".into())
        .stack_size(4096)
        .spawn(move || {
            // Without these the status layer wins and the ring just breathes blue.
            let setup = [
                Input::Net(NetStatus::Online),
                Input::Phase(SessionPhase::Live),
            ];
            if setup.into_iter().any(|input| tx.send(input).is_err()) {
                log::warn!("render loop is gone, stopping the fake race");
                return;
            }

            loop {
                for (event, hold) in SCRIPT {
                    let input = Input::Race {
                        event,
                        received: Instant::now(),
                    };
                    if tx.send(input).is_err() {
                        log::warn!("render loop is gone, stopping the fake race");
                        return;
                    }
                    thread::sleep(hold);
                }
            }
        })?;
    Ok(handle)
}
