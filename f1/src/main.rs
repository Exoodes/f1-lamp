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

mod io;
mod net;
#[cfg(feature = "player")]
mod replay;
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

    let mut controller = Controller::new(Instant::now());
    controller.apply(Input::Settings(storage.load()), Instant::now());
    // Written by the render loop, read by the web server.
    let snapshot = Arc::new(Mutex::new(controller.snapshot()));
    let mut last_snapshot = controller.snapshot();

    let (tx, rx) = mpsc::sync_channel::<Input>(32);
    // Set by the net thread, read by the live-feed thread.
    let live = Arc::new(io::live::LiveControl::default());
    // Set from the web page, read by the live-feed thread.
    let tokens = Arc::new(io::token_store::TokenKeeper::new(nvs.clone())?);
    // Every spawned thread, watched by `check_threads`.
    let threads = [
        (
            "net",
            net::spawn(
                peripherals.modem,
                sys_loop.clone(),
                nvs.clone(),
                tx.clone(),
                Arc::clone(&snapshot),
                Arc::clone(&live),
                Arc::clone(&tokens),
            )?,
        ),
        // A replay plays its own events; the live feed would mix in real ones.
        #[cfg(not(feature = "player"))]
        (
            "live",
            io::live::spawn(tx.clone(), Arc::clone(&live), Arc::clone(&tokens))?,
        ),
        #[cfg(feature = "player")]
        ("replay", replay::spawn(tx.clone())?),
    ];

    let mut frame = Frame::new();
    let mut next_thread_check = Instant::now() + THREAD_CHECK_EVERY;

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
