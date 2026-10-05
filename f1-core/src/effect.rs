use std::f32::consts::TAU;
use std::time::Duration;

use crate::{
    color::Rgb,
    frame::{Frame, NUM_LEDS},
};

const BREATHE_FLOOR: f32 = 0.1;
const FLASH_PERIOD_MS: u32 = 300;
const CHEQUERED_SWAP_MS: u32 = 500;
/// Chequered pattern: one lit pixel, then three dark ones.
const CHEQUERED_SPACING: usize = 4;
const START_LIGHTS: usize = 5;
const START_LIGHT_INTERVAL_MS: u64 = 1000;
const START_LIGHTS_DARK_MS: u64 = 1000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Solid(Rgb),
    Blink {
        color: Rgb,
        period_ms: u32,
        times: Option<u8>,
    },
    Chase {
        color: Rgb,
        leds_per_sec: f32,
        tail: u8,
    },
    Breathe {
        color: Rgb,
        period_ms: u32,
    },
    Chequered {
        duration_ms: u32,
    },
    StartLights {
        hold_ms: u32,
    },
}

impl Effect {
    pub fn render(&self, elapsed: Duration, frame: &mut Frame) {
        match self {
            Effect::Solid(color) => handle_solid(frame, *color),
            Effect::Blink {
                color,
                period_ms,
                times,
            } => handle_blink(
                frame,
                *color,
                u128::from(*period_ms).max(1),
                *times,
                elapsed,
            ),
            Effect::Chase {
                color,
                leds_per_sec,
                tail,
            } => handle_chase(frame, elapsed, *color, *leds_per_sec, *tail),
            Effect::Breathe { color, period_ms } => {
                handle_breathe(frame, elapsed, *color, u128::from(*period_ms).max(1))
            }
            Effect::Chequered { duration_ms } => {
                handle_chequered(frame, u128::from(*duration_ms), elapsed)
            }
            Effect::StartLights { hold_ms } => handle_start_lights(frame, *hold_ms, elapsed),
        }
    }

    pub fn duration(&self) -> Option<Duration> {
        match self {
            Effect::Solid(_) => None,
            Effect::Blink {
                period_ms, times, ..
            } => times.map(|n| Duration::from_millis(u64::from(*period_ms).max(1) * u64::from(n))),
            Effect::Chase { .. } => None,
            Effect::Breathe { .. } => None,
            Effect::Chequered { duration_ms } => {
                Some(Duration::from_millis(u64::from(*duration_ms)))
            }
            Effect::StartLights { hold_ms } => Some(Duration::from_millis(
                lights_out_ms(*hold_ms) + START_LIGHTS_DARK_MS,
            )),
        }
    }

    /// How long after the start lights begin they go out: the moment that
    /// has to line up with lights out on TV. `None` for other effects.
    pub fn lights_out_after(&self) -> Option<Duration> {
        match self {
            Effect::StartLights { hold_ms } => Some(Duration::from_millis(lights_out_ms(*hold_ms))),
            _ => None,
        }
    }

    pub const fn flash(color: Rgb, times: u8) -> Effect {
        Effect::Blink {
            color,
            period_ms: FLASH_PERIOD_MS,
            times: Some(times),
        }
    }
}

fn handle_solid(frame: &mut Frame, color: Rgb) {
    frame.fill(color)
}

fn handle_blink(
    frame: &mut Frame,
    color: Rgb,
    period_ms: u128,
    times: Option<u8>,
    elapsed: Duration,
) {
    let finished = times.is_some_and(|n| elapsed.as_millis() >= period_ms * u128::from(n));
    let phase = ms_into_period(elapsed, period_ms);
    if !finished && phase < (period_ms / 2) {
        frame.fill(color);
    } else {
        frame.fill(Rgb::OFF);
    }
}

fn handle_chase(frame: &mut Frame, elapsed: Duration, color: Rgb, leds_per_sec: f32, tail: u8) {
    let head_position = (elapsed.as_secs_f32() * leds_per_sec) as usize % NUM_LEDS;
    let tail = usize::from(tail).min(NUM_LEDS - 1);

    frame.fill(Rgb::OFF);

    for i in 0..=tail {
        let position = (head_position + NUM_LEDS - i) % NUM_LEDS;
        let factor = 1.0 - i as f32 / (tail as f32 + 1.0);
        frame.set(position, color.scale(factor));
    }
}

fn handle_breathe(frame: &mut Frame, elapsed: Duration, color: Rgb, period_ms: u128) {
    let phase = ms_into_period(elapsed, period_ms) as f32 / period_ms as f32;
    let value = (1.0 - (TAU * phase).cos()) / 2.0;
    let brightness = BREATHE_FLOOR + (1.0 - BREATHE_FLOOR) * value;
    frame.fill(color.scale(brightness));
}

fn handle_chequered(frame: &mut Frame, duration_ms: u128, elapsed: Duration) {
    let elapsed_ms = elapsed.as_millis();
    if elapsed_ms >= duration_ms {
        frame.fill(Rgb::OFF);
        return;
    }

    // Two patterns half a spacing apart, swapping every half second: each lit
    // pixel of one sits in the middle of a gap of the other.
    let swapped = (elapsed_ms / u128::from(CHEQUERED_SWAP_MS)) % 2 == 1;
    let offset = if swapped { CHEQUERED_SPACING / 2 } else { 0 };
    for i in 0..NUM_LEDS {
        let is_white = i % CHEQUERED_SPACING == offset;
        frame.set(i, if is_white { Rgb::WHITE } else { Rgb::OFF });
    }
}

fn handle_start_lights(frame: &mut Frame, hold_ms: u32, elapsed: Duration) {
    let lit = start_lights_lit(hold_ms, elapsed);
    for i in 0..NUM_LEDS {
        let color = if segment_of(i) < lit {
            Rgb::RED
        } else {
            Rgb::OFF
        };
        frame.set(i, color);
    }
}

fn start_lights_lit(hold_ms: u32, elapsed: Duration) -> usize {
    let elapsed_ms = elapsed.as_millis();
    if elapsed_ms >= u128::from(lights_out_ms(hold_ms)) {
        return 0;
    }
    let lit = elapsed_ms / u128::from(START_LIGHT_INTERVAL_MS) + 1;
    lit.min(START_LIGHTS as u128) as usize
}

fn lights_out_ms(hold_ms: u32) -> u64 {
    (START_LIGHTS as u64 - 1) * START_LIGHT_INTERVAL_MS + u64::from(hold_ms)
}

fn segment_of(i: usize) -> usize {
    i * START_LIGHTS / NUM_LEDS
}

fn ms_into_period(elapsed: Duration, period_ms: u128) -> u128 {
    elapsed.as_millis() % period_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLINK: Effect = Effect::Blink {
        color: Rgb::RED,
        period_ms: 1000,
        times: None,
    };

    fn render_at(effect: Effect, ms: u64) -> Frame {
        let mut frame = Frame::new();
        effect.render(Duration::from_millis(ms), &mut frame);
        frame
    }

    fn all_pixels_are(frame: &Frame, color: Rgb) -> bool {
        frame.pixels().iter().all(|&c| c == color)
    }

    #[test]
    fn solid_fills_every_pixel() {
        let frame = render_at(Effect::Solid(Rgb::GREEN), 0);
        assert!(all_pixels_are(&frame, Rgb::GREEN));
    }

    #[test]
    fn blink_is_on_at_start() {
        let frame = render_at(BLINK, 0);
        assert!(all_pixels_are(&frame, Rgb::RED));
    }

    #[test]
    fn blink_is_off_just_after_half_period() {
        let frame = render_at(BLINK, 501);
        assert!(all_pixels_are(&frame, Rgb::OFF));
    }

    #[test]
    fn blink_is_on_again_just_after_full_period() {
        let frame = render_at(BLINK, 1001);
        assert!(all_pixels_are(&frame, Rgb::RED));
    }

    #[test]
    fn blink_turns_off_a_previously_lit_frame() {
        let mut frame = Frame::new();
        frame.fill(Rgb::WHITE);
        BLINK.render(Duration::from_millis(600), &mut frame);
        assert!(all_pixels_are(&frame, Rgb::OFF));
    }

    #[test]
    fn blink_with_zero_period_does_not_panic() {
        let effect = Effect::Blink {
            color: Rgb::RED,
            period_ms: 0,
            times: None,
        };
        render_at(effect, 1234);
    }

    // 10 LEDs per second: one step every 100 ms, a full lap of 23 LEDs in 2300 ms.
    const CHASE: Effect = Effect::Chase {
        color: Rgb::RED,
        leds_per_sec: 10.0,
        tail: 3,
    };

    fn chase(leds_per_sec: f32, tail: u8) -> Effect {
        Effect::Chase {
            color: Rgb::RED,
            leds_per_sec,
            tail,
        }
    }

    fn lit_count(frame: &Frame) -> usize {
        frame.pixels().iter().filter(|&&c| c != Rgb::OFF).count()
    }

    #[test]
    fn chase_head_is_at_zero_at_start() {
        let frame = render_at(CHASE, 0);
        assert_eq!(frame.pixels()[0], Rgb::RED);
    }

    #[test]
    fn chase_head_moves_one_led_after_one_step() {
        let frame = render_at(CHASE, 101);
        assert_eq!(frame.pixels()[1], Rgb::RED);
        assert_ne!(frame.pixels()[0], Rgb::RED);
    }

    #[test]
    fn chase_head_wraps_to_zero_after_full_lap() {
        let before_wrap = render_at(CHASE, 2201);
        assert_eq!(before_wrap.pixels()[NUM_LEDS - 1], Rgb::RED);

        let after_wrap = render_at(CHASE, 2301);
        assert_eq!(after_wrap.pixels()[0], Rgb::RED);
    }

    #[test]
    fn chase_tail_fades_behind_head_across_the_wrap() {
        // Head at LED 0, so the tail sits at the far end of the strip.
        let frame = render_at(CHASE, 0);
        let p = frame.pixels();
        assert_eq!(p[NUM_LEDS - 1], Rgb::RED.scale(0.75));
        assert_eq!(p[NUM_LEDS - 2], Rgb::RED.scale(0.5));
        assert_eq!(p[NUM_LEDS - 3], Rgb::RED.scale(0.25));
        assert_eq!(p[NUM_LEDS - 4], Rgb::OFF);
    }

    #[test]
    fn chase_lights_only_head_and_tail_and_clears_the_rest() {
        let mut frame = Frame::new();
        frame.fill(Rgb::WHITE);
        CHASE.render(Duration::from_millis(1234), &mut frame);
        assert_eq!(lit_count(&frame), 4);
    }

    #[test]
    fn chase_with_zero_tail_lights_only_head() {
        let frame = render_at(chase(10.0, 0), 0);
        assert_eq!(lit_count(&frame), 1);
        assert_eq!(frame.pixels()[0], Rgb::RED);
    }

    #[test]
    fn chase_tail_longer_than_strip_never_overwrites_head() {
        let frame = render_at(chase(10.0, 200), 0);
        assert_eq!(frame.pixels()[0], Rgb::RED);
    }

    #[test]
    fn chase_with_zero_speed_stays_at_zero() {
        let frame = render_at(chase(0.0, 3), 5000);
        assert_eq!(frame.pixels()[0], Rgb::RED);
    }

    // Dimmest at 0 ms, brightest at 1000 ms, dimmest again at 2000 ms.
    const BREATHE: Effect = Effect::Breathe {
        color: Rgb::RED,
        period_ms: 2000,
    };

    #[test]
    fn breathe_is_at_floor_at_start() {
        let frame = render_at(BREATHE, 0);
        assert!(all_pixels_are(&frame, Rgb::RED.scale(BREATHE_FLOOR)));
    }

    #[test]
    fn breathe_is_full_at_half_period() {
        let frame = render_at(BREATHE, 1000);
        assert!(all_pixels_are(&frame, Rgb::RED));
    }

    #[test]
    fn breathe_is_back_at_floor_after_full_period() {
        let frame = render_at(BREATHE, 2000);
        assert!(all_pixels_are(&frame, Rgb::RED.scale(BREATHE_FLOOR)));
    }

    #[test]
    fn breathe_at_quarter_period_is_between_floor_and_full() {
        let frame = render_at(BREATHE, 500);
        let red = frame.pixels()[0].r;
        assert!(red > Rgb::RED.scale(BREATHE_FLOOR).r);
        assert!(red < Rgb::RED.r);
    }

    #[test]
    fn breathe_never_goes_dark() {
        let floor = Rgb::RED.scale(BREATHE_FLOOR).r;
        for ms in (0..2000).step_by(10) {
            let frame = render_at(BREATHE, ms);
            assert!(frame.pixels()[0].r >= floor, "too dark at {ms} ms");
        }
    }

    #[test]
    fn breathe_with_zero_period_does_not_panic() {
        let effect = Effect::Breathe {
            color: Rgb::RED,
            period_ms: 0,
        };
        render_at(effect, 1234);
    }

    // 3 cycles of 300 ms: on 0–149, off 150–299, ... finished at 900 ms.
    const FLASH: Effect = Effect::flash(Rgb::RED, 3);

    #[test]
    fn flash_is_on_at_start() {
        assert!(all_pixels_are(&render_at(FLASH, 0), Rgb::RED));
    }

    #[test]
    fn flash_is_off_in_second_half_of_a_cycle() {
        assert!(all_pixels_are(&render_at(FLASH, 151), Rgb::OFF));
    }

    #[test]
    fn flash_is_on_again_in_next_cycle() {
        assert!(all_pixels_are(&render_at(FLASH, 301), Rgb::RED));
    }

    #[test]
    fn flash_stays_off_after_last_cycle() {
        // 1000 ms would be in an "on" half if the flash kept going.
        assert!(all_pixels_are(&render_at(FLASH, 1000), Rgb::OFF));
    }

    #[test]
    fn flash_lights_up_exactly_times_times() {
        let mut flashes = 0;
        let mut was_on = false;
        for ms in (0..3000).step_by(10) {
            let is_on = render_at(FLASH, ms).pixels()[0] == Rgb::RED;
            if is_on && !was_on {
                flashes += 1;
            }
            was_on = is_on;
        }
        assert_eq!(flashes, 3);
    }

    #[test]
    fn flash_duration_is_times_cycles() {
        assert_eq!(FLASH.duration(), Some(Duration::from_millis(900)));
    }

    #[test]
    fn flash_with_zero_times_is_off_and_already_finished() {
        let effect = Effect::flash(Rgb::RED, 0);
        assert!(all_pixels_are(&render_at(effect, 0), Rgb::OFF));
        assert_eq!(effect.duration(), Some(Duration::ZERO));
    }

    #[test]
    fn endless_blink_keeps_blinking_long_after_start() {
        assert!(all_pixels_are(&render_at(BLINK, 100_000), Rgb::RED));
    }

    #[test]
    fn endless_effects_have_no_duration() {
        assert_eq!(BLINK.duration(), None);
        assert_eq!(Effect::Solid(Rgb::RED).duration(), None);
        assert_eq!(CHASE.duration(), None);
        assert_eq!(BREATHE.duration(), None);
    }

    // Swaps every 500 ms, finished after 3000 ms.
    const CHEQUERED: Effect = Effect::Chequered { duration_ms: 3000 };

    /// True if exactly every fourth pixel is white, starting at `first`, and the rest are off.
    fn is_every_fourth_white_from(frame: &Frame, first: usize) -> bool {
        frame
            .pixels()
            .iter()
            .enumerate()
            .all(|(i, &c)| c == if i % 4 == first { Rgb::WHITE } else { Rgb::OFF })
    }

    #[test]
    fn chequered_starts_with_every_fourth_pixel_white_from_the_first() {
        let frame = render_at(CHEQUERED, 0);
        assert!(is_every_fourth_white_from(&frame, 0));
    }

    #[test]
    fn chequered_shifts_by_two_pixels_after_half_a_second() {
        let frame = render_at(CHEQUERED, 501);
        assert!(is_every_fourth_white_from(&frame, 2));
    }

    #[test]
    fn chequered_shifts_back_after_one_second() {
        let frame = render_at(CHEQUERED, 1001);
        assert!(is_every_fourth_white_from(&frame, 0));
    }

    #[test]
    fn chequered_is_off_after_its_duration() {
        let mut frame = Frame::new();
        frame.fill(Rgb::WHITE);
        CHEQUERED.render(Duration::from_millis(3001), &mut frame);
        assert!(all_pixels_are(&frame, Rgb::OFF));
    }

    #[test]
    fn chequered_lit_pixels_never_touch_while_running() {
        for ms in (0..3000).step_by(50) {
            let frame = render_at(CHEQUERED, ms);
            let touch = frame
                .pixels()
                .windows(2)
                .any(|w| w[0] == Rgb::WHITE && w[1] == Rgb::WHITE);
            assert!(!touch, "lit pixels touch at {ms} ms");
        }
    }

    #[test]
    fn chequered_duration_is_the_configured_length() {
        assert_eq!(CHEQUERED.duration(), Some(Duration::from_millis(3000)));
    }

    // Lights at 0, 1, 2, 3, 4 s; hold 2 s; lights out at 6 s; dark until 7 s.
    const START: Effect = Effect::StartLights { hold_ms: 2000 };

    /// How many segments are fully red, checking that lit segments come first.
    fn lit_segments(frame: &Frame) -> usize {
        let lit = (0..START_LIGHTS)
            .take_while(|&s| {
                (0..NUM_LEDS)
                    .filter(|&i| segment_of(i) == s)
                    .all(|i| frame.pixels()[i] == Rgb::RED)
            })
            .count();
        let rest_off = (0..NUM_LEDS)
            .filter(|&i| segment_of(i) >= lit)
            .all(|i| frame.pixels()[i] == Rgb::OFF);
        assert!(rest_off, "segments after the lit ones must be off");
        lit
    }

    #[test]
    fn every_led_belongs_to_one_of_five_segments() {
        for i in 0..NUM_LEDS {
            assert!(segment_of(i) < START_LIGHTS, "LED {i} has no segment");
        }
    }

    #[test]
    fn every_segment_has_at_least_one_led() {
        for s in 0..START_LIGHTS {
            assert!(
                (0..NUM_LEDS).any(|i| segment_of(i) == s),
                "segment {s} is empty"
            );
        }
    }

    #[test]
    fn start_lights_first_segment_is_lit_at_start() {
        assert_eq!(lit_segments(&render_at(START, 0)), 1);
    }

    #[test]
    fn start_lights_add_one_segment_per_second() {
        for (ms, expected) in [(999, 1), (1000, 2), (2000, 3), (3000, 4), (4000, 5)] {
            assert_eq!(lit_segments(&render_at(START, ms)), expected, "at {ms} ms");
        }
    }

    #[test]
    fn start_lights_have_three_segments_at_two_and_a_half_seconds() {
        assert_eq!(lit_segments(&render_at(START, 2500)), 3);
    }

    #[test]
    fn start_lights_hold_all_five_until_lights_out() {
        assert_eq!(lit_segments(&render_at(START, 5999)), 5);
    }

    #[test]
    fn start_lights_are_fully_dark_after_hold() {
        assert!(all_pixels_are(&render_at(START, 6000), Rgb::OFF));
        assert!(all_pixels_are(&render_at(START, 6999), Rgb::OFF));
    }

    #[test]
    fn start_lights_duration_covers_lights_hold_and_dark_tail() {
        assert_eq!(START.duration(), Some(Duration::from_millis(7000)));
    }

    #[test]
    fn start_lights_with_zero_hold_go_out_as_fifth_would_light() {
        let effect = Effect::StartLights { hold_ms: 0 };
        assert_eq!(lit_segments(&render_at(effect, 3999)), 4);
        assert!(all_pixels_are(&render_at(effect, 4000), Rgb::OFF));
    }

    #[test]
    fn start_lights_go_out_after_five_steps_and_the_hold() {
        // Lights 1-5 at 0, 1, 2, 3, 4 s, then the 2 s hold.
        assert_eq!(START.lights_out_after(), Some(Duration::from_secs(6)));
    }

    #[test]
    fn other_effects_have_no_lights_out() {
        assert_eq!(Effect::Solid(Rgb::RED).lights_out_after(), None);
    }
}
