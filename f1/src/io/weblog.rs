//! The logger: every Rust log line goes to the serial port, and the last few
//! are also kept for the web page's live log (`GET /api/log`).
//!
//! Only `log` crate lines are caught (`f1::...`); ESP-IDF's own C messages
//! (`wifi:`, `esp-tls`) take another path and reach the serial port only.

use std::sync::Mutex;

use esp_idf_svc::log::{EspIdfLogFilter, EspLogger};
use f1_core::logbuf::{LogBuffer, LogPage};
use log::{Log, Metadata, Record};

use crate::io::clock;

/// Enough to bridge the page's one-second polls, even when a burst of lines
/// (a reconnect, a replay jump) comes in between.
const LINES: usize = 40;
/// Longer lines (snapshots) are cut; the serial port still has them whole.
const MAX_LINE: usize = 240;

static BUFFER: Mutex<LogBuffer> = Mutex::new(LogBuffer::new(LINES, MAX_LINE));

struct TeeLogger {
    serial: EspLogger,
}

static LOGGER: TeeLogger = TeeLogger {
    serial: EspLogger::new(EspIdfLogFilter::new()),
};

impl Log for TeeLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.serial.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        self.serial.log(record);
        if !self.serial.enabled(record.metadata()) {
            return;
        }
        // Formatted before taking the lock, which is held only to push.
        // The time is taken now, when the line is logged: the page may only
        // fetch it a second (or, from a background tab, much) later.
        let line = format!(
            "{} {} {}: {}",
            clock::log_stamp(),
            record.level().as_str().chars().next().unwrap_or('?'),
            record.target(),
            record.args()
        );
        // `try_lock`: never wait inside the logger. A line logged while the
        // web server is copying the buffer is lost here, not on the serial
        // port.
        if let Ok(mut buffer) = BUFFER.try_lock() {
            buffer.push(line);
        }
    }

    fn flush(&self) {}
}

/// Installs the logger. Call instead of `EspLogger::initialize_default()`:
/// the serial output is the same.
pub fn init() {
    if log::set_logger(&LOGGER).is_ok() {
        LOGGER.serial.filter().initialize();
    }
}

/// The lines after `after` for the page; see `LogBuffer::since`. Doesn't
/// log anything itself, or every poll would add a line.
pub fn page(after: Option<u64>) -> LogPage {
    BUFFER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .since(after)
}
