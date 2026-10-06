//! The wall clock once SNTP has set it: Unix time, the local minute of the day
//! for night mode, and the log's time stamps.

use std::time::{SystemTime, UNIX_EPOCH};

use esp_idf_svc::sys;

/// Central European time as a POSIX TZ string: UTC+1 (CET), and UTC+2 (CEST)
/// from 02:00 on the last Sunday of March (`M3.5.0`) to 03:00 on the last
/// Sunday of October (`M10.5.0/3`). Only night mode and the log's times are
/// local; the schedule and the feed work in UTC.
const TIMEZONE: &str = "CET-1CEST,M3.5.0,M10.5.0/3";

const FIRST_VALID_YEAR: i32 = 2025;
/// 2025-01-01 00:00 UTC. Before SNTP syncs, the chip thinks it's 1970.
const FIRST_VALID_UNIX: i64 = 1_735_689_600;

/// Unix seconds (UTC) once SNTP has set the clock; `None` before that.
pub fn unix_now() -> Option<i64> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let seconds = i64::try_from(seconds).ok()?;
    (seconds >= FIRST_VALID_UNIX).then_some(seconds)
}

/// Makes local time Central European (night mode and the log's times).
pub fn set_timezone() {
    std::env::set_var("TZ", TIMEZONE);
    // SAFETY: `tzset` takes no arguments; it reads `TZ`, set just above.
    unsafe { sys::tzset() };
}

/// Minutes since local midnight once SNTP has set the clock.
pub fn minute_of_day() -> Option<u16> {
    let tm = local_time()?;
    u16::try_from(tm.tm_hour * 60 + tm.tm_min).ok()
}

/// The time of day for log lines on the web page: local `07:31:02` once
/// SNTP has set the clock, before that the time since boot (`+3.2s`).
pub fn log_stamp() -> String {
    match local_time() {
        Some(tm) => format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
        None => {
            // SAFETY: reads the microseconds since boot; no preconditions.
            let micros = unsafe { sys::esp_timer_get_time() };
            format!("+{:.1}s", micros as f64 / 1e6)
        }
    }
}

/// The local date and time once SNTP has set the clock; `None` before that.
fn local_time() -> Option<sys::tm> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let c_sec = sys::time_t::try_from(seconds).ok()?;
    let mut tm = sys::tm::default();
    // SAFETY: both pointers are to locals that live through the call, and
    // `localtime_r` (unlike `localtime`) writes only to `tm`, so any thread
    // may call it.
    let result = unsafe { sys::localtime_r(&c_sec, &mut tm) };
    (!result.is_null() && tm.tm_year + 1900 >= FIRST_VALID_YEAR).then_some(tm)
}
