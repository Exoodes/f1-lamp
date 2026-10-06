//! How the firmware uses the chip: thread stacks, the LED output, the input
//! queue and the flash keys, in one place. Timeouts and intervals stay next
//! to the code that uses them, with their reasons.

/// Stack sizes, in bytes, of the threads and tasks the firmware starts. The
/// main task's (the render loop) is `CONFIG_ESP_MAIN_TASK_STACK_SIZE` in
/// sdkconfig.defaults.
pub mod stack {
    /// WiFi, SNTP, the calendar and winner downloads (TLS handshakes, JSON).
    pub const NET: usize = 16 * 1024;
    /// Negotiate (TLS handshakes) and parsing the feed's frames.
    pub const LIVE: usize = 12 * 1024;
    /// Each web request handler, OTA uploads included.
    pub const WEB: usize = 10 * 1024;
    /// The WebSocket client's own task: it only copies bytes into a channel.
    /// An `i32` because that's what `esp_websocket_client_config_t` takes.
    pub const WS_TASK: i32 = 6 * 1024;
    /// Replay builds: reading and stepping through the embedded session.
    #[cfg(feature = "player")]
    pub const REPLAY: usize = 8 * 1024;
}

/// The LED ring: 23 WS2812s (`f1_core::frame::NUM_LEDS`) with their data line
/// on GPIO2, the XIAO's D0. GPIO2 is a strapping pin of the ESP32-C3: nothing
/// on the data line may pull it low while the chip starts. (The pin itself
/// is taken in main.rs: a pin is a type there, not a value.)
pub mod led {
    /// The ws2812 driver needs the RMT clock undivided (80 MHz ticks).
    pub const RMT_CLOCK_DIVIDER: u8 = 1;
    /// One RMT memory block holds just 2 LEDs' worth of signal, so the driver
    /// refills it ~20 times per frame; WiFi can delay a refill, which breaks
    /// the frame and makes LEDs flicker. Channel 0 borrows the blocks of the
    /// unused channels 1-3, giving each refill 4x more time.
    pub const RMT_MEM_BLOCKS: u8 = 4;
}

/// Inputs waiting for the render loop, which takes them all every frame
/// (20 ms). When it's full, threads wait and the web server answers 503.
pub const INPUT_QUEUE: usize = 32;

/// Everything the firmware keeps in NVS. All of it shares one namespace, so
/// the keys are listed together where a clash would show. NVS limits
/// namespaces and keys to 15 characters.
pub mod nvs {
    pub const NAMESPACE: &str = "f1";
    /// `Settings` as JSON (`storage.rs`).
    pub const SETTINGS: &str = "settings";
    /// The cached sessions as JSON (`storage.rs`).
    pub const SCHEDULE: &str = "schedule";
    /// The F1TV token, as a blob (`token_store.rs`).
    pub const F1TV_TOKEN: &str = "f1tv_token";
    /// How often the firmware on trial has started; absent when none is
    /// (`ota.rs`).
    pub const OTA_TRIAL: &str = "ota_trial";
    /// Set before switching back, so the firmware that runs next can say why
    /// (`ota.rs`).
    pub const OTA_FELL_BACK: &str = "ota_fell_back";
}
