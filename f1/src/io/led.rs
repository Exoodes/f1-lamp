use f1_core::frame::Frame;
use smart_leds::{SmartLedsWrite, RGB8};
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

pub struct LedOutput<'d> {
    driver: Ws2812Esp32Rmt<'d>,
}

impl<'d> LedOutput<'d> {
    pub fn new(driver: Ws2812Esp32Rmt<'d>) -> Self {
        Self { driver }
    }

    pub fn write(&mut self, frame: &Frame) -> anyhow::Result<()> {
        let pixels = frame
            .pixels()
            .iter()
            .map(|c| RGB8::new(c.r / 4, c.g / 4, c.b / 4));
        self.driver
            .write(pixels)
            .map_err(|e| anyhow::anyhow!("LED write failed: {e:?}"))?;
        Ok(())
    }
}
