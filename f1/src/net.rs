use std::{
    sync::mpsc::SyncSender,
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::Context;
use esp_idf_svc::{eventloop::EspSystemEventLoop, hal::modem::Modem, nvs::EspDefaultNvsPartition};

use f1_core::input::{Input, NetStatus};

use crate::{io::wifi, WIFI_PSK, WIFI_SSID};

/// Spawns the network thread, which owns WiFi and reports its status.
pub fn spawn(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    tx: SyncSender<Input>,
) -> anyhow::Result<JoinHandle<()>> {
    let handle = thread::Builder::new()
        .name("net".into())
        .stack_size(16 * 1024)
        .spawn(move || {
            if let Err(e) = run(modem, sys_loop, nvs, &tx) {
                log::error!("network thread stopped: {e:#}");
            }
        })?;
    Ok(handle)
}

/// The thread's body. Returns only on a fatal error.
fn run(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    tx: &SyncSender<Input>,
) -> anyhow::Result<()> {
    tx.send(Input::Net(NetStatus::Connecting))
        .context("render loop is gone")?;
    let mut wifi = wifi::create(modem, sys_loop, nvs, WIFI_SSID, WIFI_PSK)?;
    wifi::connect(&mut wifi)?;
    tx.send(Input::Net(NetStatus::Online))
        .context("render loop is gone")?;
    loop {
        thread::sleep(Duration::from_secs(5));
    }
}

fn report(
    tx: &SyncSender<Input>,
    last: &mut Option<NetStatus>,
    new: NetStatus,
) -> anyhow::Result<()> {
    // if *last == Some(new) → nothing to do, return Ok(())
    // otherwise: send Input::Net(new) (with the same .context), then *last = Some(new)

    if let Some(last) = *last {}

    Input::Net(new);
    Ok(())
}
