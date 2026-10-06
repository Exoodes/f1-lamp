use std::{
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use esp_idf_svc::hal::peripherals::Peripherals;
#[allow(deprecated)]
use esp_idf_svc::hal::rmt::{config::TransmitConfig, TxRmtDriver};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use f1_core::{controller::Controller, frame::Frame, heartbeat::Heartbeat, input::Input};
use io::led::LedOutput;
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

mod config;
mod io;
mod net;
#[cfg(feature = "player")]
mod replay;
mod web;

const FRAME_INTERVAL: Duration = Duration::from_millis(20);
/// How often the render loop checks that all threads are still running.
const THREAD_CHECK_EVERY: Duration = Duration::from_secs(1);
/// A thread that hasn't beaten for this long has hung. Well above the
/// longest normal wait: two 15 s HTTPS requests plus the TLS lock, or the
/// live thread's 60 s pause between reconnects.
const THREAD_SILENCE_LIMIT: Duration = Duration::from_secs(180);

// WiFi credentials from f1/cfg.toml, passed in by build.rs.
const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PSK: &str = env!("WIFI_PSK");

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    let booted = Instant::now();

    // The serial log as before, plus the last lines for the web page.
    io::weblog::init();
    if WIFI_SSID.is_empty() {
        log::warn!("WiFi SSID is empty: set wifi_ssid in f1/cfg.toml");
    }
    log::info!("SSID: {WIFI_SSID}");

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    // Before anything that could crash: a new firmware on trial is counted
    // here, and switched back after three boots that never got healthy
    // (a crash, or no health within `HEALTHY_WITHIN`, see the render loop).
    io::ota::check_boot(nvs.clone())?;
    #[cfg(feature = "crash-test")]
    crash_test();

    let mut storage = io::storage::SettingsStore::new(nvs.clone())?;

    // ws2812-esp32-rmt-driver 0.14 only supports the legacy RMT API. The
    // values and why: `config::led`.
    #[allow(deprecated)]
    let driver = {
        let config = TransmitConfig::new()
            .clock_divider(config::led::RMT_CLOCK_DIVIDER)
            .mem_block_num(config::led::RMT_MEM_BLOCKS);
        let tx = TxRmtDriver::new(peripherals.rmt.channel0, peripherals.pins.gpio2, &config)?;
        Ws2812Esp32Rmt::new_with_rmt_driver(tx)?
    };

    let mut leds = LedOutput::new(driver);
    leds.write(&Frame::new())?;

    let mut controller = Controller::new(Instant::now());
    controller.apply(Input::Settings(storage.load()), Instant::now());
    // Written by the render loop, read by the web server.
    let snapshot = Arc::new(Mutex::new(controller.snapshot()));
    let mut last_snapshot = controller.snapshot();

    let (tx, rx) = mpsc::sync_channel::<Input>(config::INPUT_QUEUE);
    // Set by the net thread, read by the live-feed thread.
    let live = Arc::new(io::live::LiveControl::default());
    // Set from the web page, read by the live-feed thread.
    let tokens = Arc::new(io::token_store::TokenKeeper::new(nvs.clone())?);
    let net_beat = Arc::new(Heartbeat::new(Instant::now()));
    let feed_beat = Arc::new(Heartbeat::new(Instant::now()));
    // Every spawned thread with its heartbeat, watched by `check_threads`.
    let threads = [
        Watched {
            name: "net",
            handle: net::spawn(
                peripherals.modem,
                sys_loop.clone(),
                nvs.clone(),
                tx.clone(),
                net::Shared {
                    snapshot: Arc::clone(&snapshot),
                    live: Arc::clone(&live),
                    tokens: Arc::clone(&tokens),
                },
                Arc::clone(&net_beat),
            )?,
            heartbeat: net_beat,
        },
        // A replay plays its own events; the live feed would mix in real ones.
        #[cfg(not(feature = "player"))]
        Watched {
            name: "live",
            handle: io::live::spawn(
                tx.clone(),
                Arc::clone(&live),
                Arc::clone(&tokens),
                Arc::clone(&feed_beat),
            )?,
            heartbeat: feed_beat,
        },
        #[cfg(feature = "player")]
        Watched {
            name: "replay",
            handle: replay::spawn(tx.clone(), Arc::clone(&feed_beat))?,
            heartbeat: feed_beat,
        },
    ];

    let mut frame = Frame::new();
    let mut next_thread_check = Instant::now() + THREAD_CHECK_EVERY;

    loop {
        let now = Instant::now();

        if now >= next_thread_check {
            check_threads(&threads, now);
            // Here, not in the net thread: that thread may be the one stuck.
            if io::ota::trial_overdue(now.duration_since(booted)) {
                log::error!(
                    "ota: new firmware not healthy {} s after boot, restarting",
                    now.duration_since(booted).as_secs()
                );
                restart();
            }
            next_thread_check = now + THREAD_CHECK_EVERY;
        }

        while let Ok(input) = rx.try_recv() {
            log::info!("input: {input:?}");
            controller.apply(input, now);
            if let Input::Settings(s) = input {
                storage.changed(s);
            }
        }

        controller.render(now, &mut frame);
        if let Err(e) = leds.write(&frame) {
            log::warn!("{e}");
        }

        // After render, which ticks, so expired overlays don't show.
        let snap = controller.snapshot();
        if snap != last_snapshot {
            *snapshot.lock().unwrap() = snap;
            match serde_json::to_string(&snap) {
                Ok(json) => log::info!("snapshot: {json}"),
                Err(e) => log::warn!("snapshot not serialisable: {e}"),
            }
            last_snapshot = snap;
        }

        storage.save_if_due(now);
        thread::sleep(FRAME_INTERVAL);
    }
}

/// A spawned thread and the heartbeat it keeps.
struct Watched {
    name: &'static str,
    handle: JoinHandle<()>,
    heartbeat: Arc<Heartbeat>,
}

/// Restarts the chip if any thread has ended, for whatever reason (a panic,
/// an error, or a normal return), or has hung: no heartbeat for
/// `THREAD_SILENCE_LIMIT`. The lamp can't work with a thread missing, and a
/// fresh boot is the simplest way to get it back.
fn check_threads(threads: &[Watched], now: Instant) {
    for thread in threads {
        if thread.handle.is_finished() {
            log::error!("thread {} stopped, restarting", thread.name);
            restart();
        }
        let silent = thread.heartbeat.silent_for(now);
        if silent >= THREAD_SILENCE_LIMIT {
            log::error!(
                "thread {} silent for {} s, restarting",
                thread.name,
                silent.as_secs()
            );
            restart();
        }
    }
}

/// With `--features crash-test`: a firmware that crashes 10 s after every
/// boot, to see the update fallback switch back to the firmware before it.
#[cfg(feature = "crash-test")]
fn crash_test() {
    log::warn!("crash-test: this firmware crashes in 10 s, on every boot");
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(10));
        panic!("crash-test");
    });
}

pub fn restart() -> ! {
    // SAFETY: `esp_restart` takes no arguments and has no preconditions; it
    // reboots the chip and never returns.
    unsafe { esp_idf_svc::sys::esp_restart() }
}
