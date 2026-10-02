use std::{
    sync::mpsc::SyncSender,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Context;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop, hal::modem::Modem, nvs::EspDefaultNvsPartition, sntp::EspSntp,
};
use f1_core::input::{Input, NetStatus};

use crate::{
    io::{clock, wifi},
    WIFI_PSK, WIFI_SSID,
};

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

/// How often to check that WiFi is still up.
const WIFI_CHECK_EVERY: Duration = Duration::from_secs(5);
/// How often to log the free heap.
const HEAP_LOG_EVERY: Duration = Duration::from_secs(60);
/// Wait before the first reconnect attempt; doubles after each failure.
const FIRST_RETRY_WAIT: Duration = Duration::from_secs(1);
/// The reconnect wait never grows beyond this.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);
/// How often to send the time once it's known.
const CLOCK_EVERY: Duration = Duration::from_secs(30);
/// How often to look for the first valid time after boot.
const CLOCK_WAITING_EVERY: Duration = Duration::from_secs(1);

/// When each of the thread's jobs is next due.
struct Deadlines {
    wifi_check: Instant,
    heap_log: Instant,
    clock: Instant,
}

impl Deadlines {
    fn earliest(&self) -> Instant {
        self.wifi_check.min(self.heap_log).min(self.clock)
    }
}

/// The thread's body. Returns only on a fatal error.
fn run(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    tx: &SyncSender<Input>,
) -> anyhow::Result<()> {
    let mut status = None;
    report(tx, &mut status, NetStatus::Connecting)?;
    let mut wifi = wifi::create(modem, sys_loop, nvs, WIFI_SSID, WIFI_PSK)?;
    clock::set_timezone();
    // Kept alive here: time only syncs while this exists. Started once WiFi is up.
    let mut sntp: Option<EspSntp<'static>> = None;

    let mut retry_wait = FIRST_RETRY_WAIT;
    let now = Instant::now();
    let mut due = Deadlines {
        wifi_check: now,
        heap_log: now,
        clock: now,
    };

    loop {
        let now = Instant::now();

        if now >= due.wifi_check {
            due.wifi_check = check_wifi(&mut wifi, tx, &mut status, &mut retry_wait)?;
        }
        if sntp.is_none() && wifi.is_up().unwrap_or(false) {
            sntp = Some(EspSntp::new_default().context("start SNTP")?);
            log::info!("SNTP started");
        }
        if now >= due.clock {
            due.clock = match clock::minute_of_day() {
                // Until the first sync nothing is sent, so the core keeps "not night".
                None => now + CLOCK_WAITING_EVERY,
                Some(minute_of_day) => {
                    log::info!("clock: {:02}:{:02}", minute_of_day / 60, minute_of_day % 60);
                    tx.send(Input::Clock { minute_of_day })
                        .context("render loop is gone")?;
                    now + CLOCK_EVERY
                }
            };
        }
        if now >= due.heap_log {
            let (free, min_free) = heap();
            log::info!("heap: {free} B free, {min_free} B lowest since boot");
            due.heap_log = now + HEAP_LOG_EVERY;
        }

        thread::sleep(due.earliest().saturating_duration_since(Instant::now()));
    }
}

/// Reconnects if WiFi is down. Returns when the next check is due: soon when
/// all is well, after a wait that doubles with each failure when it isn't.
fn check_wifi(
    wifi: &mut wifi::Wifi,
    tx: &SyncSender<Input>,
    status: &mut Option<NetStatus>,
    retry_wait: &mut Duration,
) -> anyhow::Result<Instant> {
    if wifi.is_up().unwrap_or(false) {
        report(tx, status, NetStatus::Online)?;
        return Ok(Instant::now() + WIFI_CHECK_EVERY);
    }

    report(tx, status, NetStatus::Connecting)?;
    log::info!("connecting to {WIFI_SSID}");
    match wifi::connect(wifi) {
        Ok(()) => {
            report(tx, status, NetStatus::Online)?;
            *retry_wait = FIRST_RETRY_WAIT;
            Ok(Instant::now() + WIFI_CHECK_EVERY)
        }
        Err(e) => {
            log::warn!("wifi: {e:#}; retrying in {} s", retry_wait.as_secs());
            let next = Instant::now() + *retry_wait;
            *retry_wait = (*retry_wait * 2).min(MAX_RETRY_WAIT);
            Ok(next)
        }
    }
}

/// Sends `new` unless it's the status we last sent.
fn report(
    tx: &SyncSender<Input>,
    last: &mut Option<NetStatus>,
    new: NetStatus,
) -> anyhow::Result<()> {
    if *last == Some(new) {
        return Ok(());
    }
    tx.send(Input::Net(new)).context("render loop is gone")?;
    *last = Some(new);
    Ok(())
}

/// Free heap now, and the lowest it has been since boot, in bytes.
fn heap() -> (u32, u32) {
    // SAFETY: both functions only read ESP-IDF's heap counters; they take no
    // arguments and have no preconditions.
    unsafe {
        (
            esp_idf_svc::sys::esp_get_free_heap_size(),
            esp_idf_svc::sys::esp_get_minimum_free_heap_size(),
        )
    }
}
