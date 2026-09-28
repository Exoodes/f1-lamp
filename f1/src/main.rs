use esp_idf_svc::hal::peripherals::Peripherals;
use f1_core::{color::Rgb, frame::Frame};
use io::led::LedOutput;
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

mod io;

fn main() -> anyhow::Result<()> {
    // It is necessary to call this function once. Otherwise, some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("Hello, world!");

    let peripherals = Peripherals::take()?;
    #[allow(deprecated)] // ws2812-esp32-rmt-driver 0.14 only supports the legacy RMT API
    let driver = Ws2812Esp32Rmt::new(peripherals.rmt.channel0, peripherals.pins.gpio2)?;

    let mut leds = LedOutput::new(driver);
    leds.write(&Frame::new())?;

    let mut frame = Frame::new();
    frame.fill(Rgb::RED);
    leds.write(&frame)?;
    log::info!("LEDs written");

    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
