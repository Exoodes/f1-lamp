use std::{
    sync::mpsc::{SendError, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use f1_core::{
    color::Rgb,
    input::{Input, RaceEvent, TrackFlag},
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

/// Sends only race events: settings are saved to flash, so sending them here
/// would overwrite the user's; the phase comes from the scheduler's fake race.
fn play(tx: &SyncSender<Input>) -> Result<(), SendError<Input>> {
    loop {
        for (event, hold) in SCRIPT {
            tx.send(Input::Race {
                event,
                received: Instant::now(),
            })?;
            thread::sleep(hold);
        }

        thread::sleep(Duration::from_secs(10));
    }
}
