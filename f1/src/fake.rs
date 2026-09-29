use std::{
    sync::mpsc::SyncSender,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use f1_core::input::{Input, RaceEvent, TrackFlag};

/// Each flag and how long it stays before the next one is sent.
const SCRIPT: [(TrackFlag, Duration); 7] = [
    (TrackFlag::Green, Duration::from_secs(5)),
    (TrackFlag::Yellow, Duration::from_secs(6)),
    (TrackFlag::DoubleYellow, Duration::from_secs(4)),
    (TrackFlag::Green, Duration::from_secs(3)),
    (TrackFlag::SafetyCar, Duration::from_secs(6)),
    (TrackFlag::VirtualSafetyCar, Duration::from_secs(6)),
    (TrackFlag::Red, Duration::from_secs(5)),
];

/// Spawns a thread that plays `SCRIPT` into `tx` forever.
pub fn spawn(tx: SyncSender<Input>) -> anyhow::Result<JoinHandle<()>> {
    let handle = thread::Builder::new()
        .name("fake".into())
        .stack_size(4096)
        .spawn(move || loop {
            for (flag, hold) in SCRIPT {
                let input = Input::Race {
                    event: RaceEvent::TrackFlag(flag),
                    received: Instant::now(),
                };
                if tx.send(input).is_err() {
                    log::warn!("render loop is gone, stopping the fake race");
                    return;
                }
                thread::sleep(hold);
            }
        })?;
    Ok(handle)
}
