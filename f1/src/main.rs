use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use f1_core::{controller::Controller, frame::Frame, input::Input};
use io::led::LedOutput;
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

#[cfg(feature = "fake")]
mod fake;
mod io;
mod net;

const FRAME_INTERVAL: Duration = Duration::from_millis(20);

#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_psk: &'static str,
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();

    esp_idf_svc::log::EspLogger::initialize_default();
    if CONFIG.wifi_ssid.is_empty() {
        log::warn!("WiFi SSID is empty: is f1/cfg.toml missing, or is its table not named [f1]?");
    }
    log::info!("SSID: {}", CONFIG.wifi_ssid);

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    #[allow(deprecated)]
    let driver = Ws2812Esp32Rmt::new(peripherals.rmt.channel0, peripherals.pins.gpio2)?;

    let mut leds = LedOutput::new(driver);
    leds.write(&Frame::new())?;

    let (tx, rx) = mpsc::sync_channel::<Input>(32);
    net::spawn(peripherals.modem, sys_loop.clone(), nvs.clone(), tx.clone())?;
    #[cfg(feature = "fake")]
    {
        fake::spawn(tx.clone())?;
        log::info!("fake race started");
    }

    let mut controller = Controller::new(Instant::now());
    let mut frame = Frame::new();
    let mut last_layer = "";

    loop {
        let now = Instant::now();

        while let Ok(input) = rx.try_recv() {
            log::info!("input: {input:?}");
            controller.apply(input, now);
        }

        let layer = controller.render(now, &mut frame);
        if layer != last_layer {
            log::info!("layer: {layer}");
            last_layer = layer;
        }
        if let Err(e) = leds.write(&frame) {
            log::warn!("{e}");
        }

        thread::sleep(FRAME_INTERVAL);
    }
}
