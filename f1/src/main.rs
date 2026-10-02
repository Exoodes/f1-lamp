use std::{
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use esp_idf_svc::hal::peripherals::Peripherals;
#[allow(deprecated)]
use esp_idf_svc::hal::rmt::{config::TransmitConfig, TxRmtDriver};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use f1_core::{controller::Controller, frame::Frame, input::Input};
use io::led::LedOutput;
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

#[cfg(feature = "fake")]
mod fake;
mod io;
mod net;
mod web;

const FRAME_INTERVAL: Duration = Duration::from_millis(20);
/// How often the render loop checks that all threads are still running.
const THREAD_CHECK_EVERY: Duration = Duration::from_secs(1);

// WiFi credentials from f1/cfg.toml, passed in by build.rs.
const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PSK: &str = env!("WIFI_PSK");

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();

    esp_idf_svc::log::EspLogger::initialize_default();
    if WIFI_SSID.is_empty() {
        log::warn!("WiFi SSID is empty: set wifi_ssid in f1/cfg.toml");
    }
    log::info!("SSID: {WIFI_SSID}");

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let mut storage = io::storage::SettingsStore::new(nvs.clone())?;

    // ws2812-esp32-rmt-driver 0.14 only supports the legacy RMT API.
    // One RMT memory block holds just 2 LEDs' worth of signal, so the driver
    // refills it ~20 times per frame; WiFi can delay a refill, which breaks
    // the frame and makes LEDs flicker. Channel 0 borrows the blocks of the
    // unused channels 1-3, giving each refill 4x more time.
    #[allow(deprecated)]
    let driver = {
        let config = TransmitConfig::new()
            .clock_divider(1) // required by the ws2812 driver
            .mem_block_num(4);
        let tx = TxRmtDriver::new(peripherals.rmt.channel0, peripherals.pins.gpio2, &config)?;
        Ws2812Esp32Rmt::new_with_rmt_driver(tx)?
    };

    let mut leds = LedOutput::new(driver);
    leds.write(&Frame::new())?;

    let (tx, rx) = mpsc::sync_channel::<Input>(32);
    // Every spawned thread, watched by `check_threads`.
    let threads = [
        (
            "net",
            net::spawn(peripherals.modem, sys_loop.clone(), nvs.clone(), tx.clone())?,
        ),
        #[cfg(feature = "fake")]
        ("fake", fake::spawn(tx.clone())?),
    ];
    #[cfg(feature = "fake")]
    log::info!("fake race started");

    let mut controller = Controller::new(Instant::now());
    let mut frame = Frame::new();
    let mut next_thread_check = Instant::now() + THREAD_CHECK_EVERY;

    controller.apply(Input::Settings(storage.load()), Instant::now());
    let snapshot = Arc::new(Mutex::new(controller.snapshot()));
    let mut last_snapshot = controller.snapshot();

    loop {
        let now = Instant::now();

        if now >= next_thread_check {
            check_threads(&threads);
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

/// Restarts the chip if any thread has ended, for whatever reason: a panic,
/// an error, or a normal return. The lamp can't work with a thread missing,
/// and a fresh boot is the simplest way to get it back.
fn check_threads(threads: &[(&str, JoinHandle<()>)]) {
    if let Some((name, _)) = threads.iter().find(|(_, handle)| handle.is_finished()) {
        log::error!("thread {name} stopped, restarting");
        restart();
    }
}

fn restart() -> ! {
    // SAFETY: `esp_restart` takes no arguments and has no preconditions; it
    // reboots the chip and never returns.
    unsafe { esp_idf_svc::sys::esp_restart() }
}
