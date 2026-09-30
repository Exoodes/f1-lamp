use std::{cmp::Ordering, sync::OnceLock};

use crate::{
    color::{to_channel, Rgb},
    frame::Frame,
    input::SessionPhase,
    settings::Settings,
};

const GAMMA: f32 = 2.2;

pub const MAX_BRIGHTNESS: f32 = 0.6;

fn gamma_table() -> &'static [f32; 256] {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(|i| 255.0 * (i as f32 / 255.0).powf(GAMMA)))
}

pub fn correct(color: Rgb, brightness: f32) -> Rgb {
    let table = gamma_table();
    let brightness = brightness.clamp(0.0, MAX_BRIGHTNESS);
    let channel = |c: u8| to_channel(table[usize::from(c)] * brightness);
    Rgb::new(channel(color.r), channel(color.g), channel(color.b))
}

pub fn brightness(settings: &Settings, minute_of_day: Option<u16>, phase: SessionPhase) -> f32 {
    let night = phase != SessionPhase::Live
        && is_night(minute_of_day, settings.night_start, settings.night_end);
    if night {
        settings.night_brightness
    } else {
        settings.global_brightness
    }
}

pub fn post_process(frame: &mut Frame, brightness: f32) {
    for pixel in frame.pixels_mut() {
        *pixel = correct(*pixel, brightness);
    }
}

pub fn is_night(now: Option<u16>, start: Option<u16>, end: Option<u16>) -> bool {
    let (Some(now), Some(start), Some(end)) = (now, start, end) else {
        return false;
    };
    match start.cmp(&end) {
        // e.g. 01:00–05:00
        Ordering::Less => start <= now && now < end,
        // crosses midnight, e.g. 22:00–07:00
        Ordering::Greater => now >= start || now < end,
        // start == end: no window
        Ordering::Equal => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn hm(h: u16, m: u16) -> u16 {
        h * 60 + m
    }

    const NIGHT_START: Option<u16> = Some(hm(22, 0));
    const NIGHT_END: Option<u16> = Some(hm(7, 0));

    const ORANGE: Rgb = Rgb::new(255, 165, 0);

    fn night_settings() -> Settings {
        Settings {
            night_start: NIGHT_START,
            night_end: NIGHT_END,
            global_brightness: 0.5,
            night_brightness: 0.1,
            ..Settings::default()
        }
    }

    #[test]
    fn night_time_uses_night_brightness() {
        let b = brightness(&night_settings(), Some(hm(23, 0)), SessionPhase::Idle);
        assert_eq!(b, 0.1);
    }

    #[test]
    fn daytime_uses_global_brightness() {
        let b = brightness(&night_settings(), Some(hm(12, 0)), SessionPhase::Idle);
        assert_eq!(b, 0.5);
    }

    #[test]
    fn live_session_is_never_dimmed_for_night() {
        let b = brightness(&night_settings(), Some(hm(23, 0)), SessionPhase::Live);
        assert_eq!(b, 0.5);
    }

    #[test]
    fn unknown_time_uses_global_brightness() {
        let b = brightness(&night_settings(), None, SessionPhase::Idle);
        assert_eq!(b, 0.5);
    }

    #[test]
    fn post_process_corrects_every_pixel() {
        let mut frame = Frame::new();
        frame.fill(ORANGE);
        post_process(&mut frame, 0.3);
        assert!(frame.pixels().iter().all(|&p| p == correct(ORANGE, 0.3)));
    }

    #[test]
    fn dim_orange_at_night_brightness_stays_orange() {
        let dim = correct(ORANGE, 0.05);
        assert!(dim.g > 0, "green was lost: {dim:?}");
        assert!(dim.r > dim.g, "no longer orange: {dim:?}");
        assert_eq!(dim.b, 0);
    }

    #[test]
    fn black_stays_black() {
        assert_eq!(correct(Rgb::OFF, MAX_BRIGHTNESS), Rgb::OFF);
    }

    #[test]
    fn full_channel_is_scaled_by_brightness_only() {
        // Gamma leaves 0 and 255 unchanged, so only the brightness applies.
        assert_eq!(correct(Rgb::WHITE, 0.5), Rgb::new(128, 128, 128));
    }

    #[test]
    fn gamma_darkens_mid_values() {
        let mid = correct(Rgb::new(128, 128, 128), 0.5);
        assert!(
            mid.r < 64,
            "gamma should darken 128 to well below half: {mid:?}"
        );
    }

    #[test]
    fn brightness_is_capped_at_max() {
        assert_eq!(
            correct(Rgb::WHITE, 1.0),
            correct(Rgb::WHITE, MAX_BRIGHTNESS)
        );
    }

    #[test]
    fn negative_brightness_is_off() {
        assert_eq!(correct(Rgb::WHITE, -1.0), Rgb::OFF);
    }

    #[test]
    fn late_evening_is_night_in_window_across_midnight() {
        assert!(is_night(Some(hm(23, 0)), NIGHT_START, NIGHT_END));
    }

    #[test]
    fn early_morning_is_night_in_window_across_midnight() {
        assert!(is_night(Some(hm(6, 59)), NIGHT_START, NIGHT_END));
    }

    #[test]
    fn noon_is_not_night() {
        assert!(!is_night(Some(hm(12, 0)), NIGHT_START, NIGHT_END));
    }

    #[test]
    fn window_start_is_inclusive_and_end_is_exclusive() {
        assert!(is_night(Some(hm(22, 0)), NIGHT_START, NIGHT_END));
        assert!(!is_night(Some(hm(7, 0)), NIGHT_START, NIGHT_END));
    }

    #[test]
    fn normal_window_needs_time_between_start_and_end() {
        let (start, end) = (Some(hm(1, 0)), Some(hm(5, 0)));
        assert!(is_night(Some(hm(3, 0)), start, end));
        assert!(!is_night(Some(hm(6, 0)), start, end));
        assert!(!is_night(Some(hm(0, 30)), start, end));
    }

    #[test]
    fn window_with_equal_start_and_end_is_never_night() {
        let t = Some(hm(22, 0));
        assert!(!is_night(t, t, t));
    }

    #[test]
    fn unknown_time_is_never_night() {
        assert!(!is_night(None, NIGHT_START, NIGHT_END));
    }

    #[test]
    fn missing_window_is_never_night() {
        assert!(!is_night(Some(hm(23, 0)), None, NIGHT_END));
        assert!(!is_night(Some(hm(23, 0)), NIGHT_START, None));
    }
}
