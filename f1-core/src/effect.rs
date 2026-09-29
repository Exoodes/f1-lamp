use std::time::Duration;

use crate::{color::Rgb, frame::Frame};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Solid(Rgb),
    Blink { color: Rgb, period_ms: u32 },
}

impl Effect {
    pub fn render(&self, elapsed: Duration, frame: &mut Frame) {
        match self {
            Effect::Solid(color) => handle_solid(frame, color),
            Effect::Blink { color, period_ms } => {
                handle_blink(frame, color, u128::from(*period_ms).max(1), elapsed)
            }
        }
    }
}

fn handle_solid(frame: &mut Frame, color: &Rgb) {
    frame.fill(*color)
}

fn handle_blink(frame: &mut Frame, color: &Rgb, period_ms: u128, elapsed: Duration) {
    let phase = elapsed.as_millis() % period_ms;
    if phase < (period_ms / 2) {
        frame.fill(*color);
    } else {
        frame.fill(Rgb::OFF);
    }
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
}
