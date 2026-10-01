use std::{
    sync::mpsc::{SendError, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use f1_core::{
    color::Rgb,
    input::{Input, RaceEvent, SessionPhase, TrackFlag},
    settings::Settings,
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

/// Spawns a thread that plays the fake race forever.
pub fn spawn(tx: SyncSender<Input>) -> anyhow::Result<JoinHandle<()>> {
    let handle = thread::Builder::new()
        .name("fake".into())
        .stack_size(4096)
        .spawn(move || {
            if play(&tx).is_err() {
                log::warn!("render loop is gone, stopping the fake race");
            }
        })?;
    Ok(handle)
}

fn play(tx: &SyncSender<Input>) -> Result<(), SendError<Input>> {
    tx.send(Input::Settings(Settings {
        night_start: Some(22 * 60),
        night_end: Some(7 * 60),
        ..Settings::default()
    }))?;

    loop {
        tx.send(Input::Phase(SessionPhase::Live))?;
        for (event, hold) in SCRIPT {
            tx.send(Input::Race {
                event,
                received: Instant::now(),
            })?;
            thread::sleep(hold);
        }

        tx.send(Input::Phase(SessionPhase::PostSession))?;
        thread::sleep(Duration::from_secs(10));
    }
}
