use std::time::{SystemTime, UNIX_EPOCH};

use esp_idf_svc::sys;

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

pub fn set_timezone() {
    std::env::set_var("TZ", TIMEZONE);
    unsafe { sys::tzset() };
}

pub fn minute_of_day() -> Option<u16> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let c_sec = sys::time_t::try_from(seconds).ok()?;
    let mut tm = sys::tm::default();
    let result = unsafe { sys::localtime_r(&c_sec, &mut tm) };
    if result.is_null() || tm.tm_year + 1900 < FIRST_VALID_YEAR {
        return None;
    }

    u16::try_from(tm.tm_hour * 60 + tm.tm_min).ok()
}
