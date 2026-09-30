use std::{
    sync::mpsc::{SendError, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use f1_core::{
    color::Rgb,
    input::{Input, NetStatus, RaceEvent, SessionPhase, TrackFlag},
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

/// Loops forever: a live race in daytime, then an evening where the lamp dims.
/// Only returns if the render loop has gone away.
fn play(tx: &SyncSender<Input>) -> Result<(), SendError<Input>> {
    // Without `Online` the status layer wins and the ring just breathes blue.
    tx.send(Input::Net(NetStatus::Online))?;
    tx.send(Input::Settings(Settings {
        night_start: Some(22 * 60),
        night_end: Some(7 * 60),
        ..Settings::default()
    }))?;

    loop {
        tx.send(Input::Phase(SessionPhase::Live))?;
        tx.send(Input::Clock {
            minute_of_day: 14 * 60,
        })?;
        for (event, hold) in SCRIPT {
            tx.send(Input::Race {
                event,
                received: Instant::now(),
            })?;
            thread::sleep(hold);
        }

        // After the race: the lamp at day brightness, then night falls.
        tx.send(Input::Phase(SessionPhase::PostSession))?;
        thread::sleep(Duration::from_secs(4));
        tx.send(Input::Clock {
            minute_of_day: 23 * 60,
        })?;
        thread::sleep(Duration::from_secs(6));
    }
}
