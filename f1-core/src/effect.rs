use std::f32::consts::TAU;
use std::time::Duration;

use crate::{
    color::Rgb,
    frame::{Frame, NUM_LEDS},
};

const BREATHE_FLOOR: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Solid(Rgb),
    Blink {
        color: Rgb,
        period_ms: u32,
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
}

impl Effect {
    pub fn render(&self, elapsed: Duration, frame: &mut Frame) {
        match self {
            Effect::Solid(color) => handle_solid(frame, *color),
            Effect::Blink { color, period_ms } => {
                handle_blink(frame, *color, u128::from(*period_ms).max(1), elapsed)
            }
            Effect::Chase {
                color,
                leds_per_sec,
                tail,
            } => handle_chase(frame, elapsed, *color, *leds_per_sec, *tail),
            Effect::Breathe { color, period_ms } => {
                handle_breathe(frame, elapsed, *color, u128::from(*period_ms).max(1))
            }
        }
    }
}

fn handle_solid(frame: &mut Frame, color: Rgb) {
    frame.fill(color)
}

fn handle_blink(frame: &mut Frame, color: Rgb, period_ms: u128, elapsed: Duration) {
    let phase = ms_into_period(elapsed, period_ms);
    if phase < (period_ms / 2) {
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

fn ms_into_period(elapsed: Duration, period_ms: u128) -> u128 {
    elapsed.as_millis() % period_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLINK: Effect = Effect::Blink {
        color: Rgb::RED,
        period_ms: 1000,
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
}
