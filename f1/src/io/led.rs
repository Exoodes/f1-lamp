#[allow(deprecated)]
use esp_idf_svc::hal::{
    gpio::OutputPin,
    rmt::{config::TransmitConfig, RmtChannel, TxRmtDriver},
};
use f1_core::frame::Frame;
use smart_leds::{SmartLedsWrite, RGB8};
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

use crate::config;

pub struct LedOutput<'d> {
    driver: Ws2812Esp32Rmt<'d>,
}

impl<'d> LedOutput<'d> {
    /// The LED ring on `channel`, sending on `pin`. ws2812-esp32-rmt-driver
    /// 0.14 only supports the legacy RMT API. The values and why:
    /// `config::led`.
    #[allow(deprecated)]
    pub fn new(channel: impl RmtChannel + 'd, pin: impl OutputPin + 'd) -> anyhow::Result<Self> {
        let rmt = TransmitConfig::new()
            .clock_divider(config::led::RMT_CLOCK_DIVIDER)
            .mem_block_num(config::led::RMT_MEM_BLOCKS);
        let tx = TxRmtDriver::new(channel, pin, &rmt)?;
        Ok(Self {
            driver: Ws2812Esp32Rmt::new_with_rmt_driver(tx)?,
        })
    }

    pub fn write(&mut self, frame: &Frame) -> anyhow::Result<()> {
        let pixels = frame.pixels().iter().map(|c| RGB8::new(c.r, c.g, c.b));
        self.driver
            .write(pixels)
            .map_err(|e| anyhow::anyhow!("LED write failed: {e:?}"))?;
        Ok(())
    }
}
