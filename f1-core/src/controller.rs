use std::time::Instant;

use crate::{
    effect::Effect,
    frame::Frame,
    input::{Input, RaceEvent, TrackFlag},
    theme::{flag_effect, LAMP},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scene {
    pub effect: Effect,
    pub started: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controller {
    pub current_flag: Option<TrackFlag>,
    pub current_scene: Scene,
}

impl Controller {
    pub fn new(now: Instant) -> Self {
        Self {
            current_flag: None,
            current_scene: Scene {
                effect: Effect::Solid(LAMP),
                started: now,
            },
        }
    }

    pub fn apply(&mut self, input: Input, now: Instant) {
        match input {
            Input::Race {
                event: RaceEvent::TrackFlag(flag),
                ..
            } => self.current_flag = Some(flag),
            _ => {}
        }

        let wanted = self.wanted_effect();
        if wanted != self.current_scene.effect {
            self.current_scene = Scene {
                effect: wanted,
                started: now,
            };
        }
    }

    pub fn render(&mut self, now: Instant, frame: &mut Frame) {
        let elapsed = now.duration_since(self.current_scene.started);
        self.current_scene.effect.render(elapsed, frame);
    }

    fn wanted_effect(&self) -> Effect {
        match self.current_flag {
            Some(flag) => flag_effect(flag),
            None => Effect::Solid(LAMP),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::color::Rgb;

    fn flag(flag: TrackFlag, received: Instant) -> Input {
        Input::Race {
            event: RaceEvent::TrackFlag(flag),
            received,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn rendered(controller: &mut Controller, now: Instant) -> Frame {
        let mut frame = Frame::new();
        controller.render(now, &mut frame);
        frame
    }

    fn all_pixels_are(frame: &Frame, color: Rgb) -> bool {
        frame.pixels().iter().all(|&c| c == color)
    }

    #[test]
    fn with_no_input_frame_is_lamp_colour() {
        let t0 = Instant::now();
        let mut controller = Controller::new(t0);
        let frame = rendered(&mut controller, t0 + ms(100));
        assert!(all_pixels_are(&frame, LAMP));
    }

    #[test]
    fn red_flag_renders_red() {
        let t0 = Instant::now();
        let mut controller = Controller::new(t0);
        controller.apply(flag(TrackFlag::Red, t0), t0);
        let frame = rendered(&mut controller, t0 + ms(100));
        assert!(all_pixels_are(&frame, Rgb::RED));
    }

    #[test]
    fn repeated_flag_keeps_blink_start_time() {
        let t0 = Instant::now();
        let mut controller = Controller::new(t0);
        controller.apply(flag(TrackFlag::Yellow, t0), t0);
        controller.apply(flag(TrackFlag::Yellow, t0 + ms(400)), t0 + ms(400));

        assert_eq!(controller.current_scene.started, t0);
        // 600 ms into a 1000 ms blink is off; it would be on if the start had reset to 400 ms.
        let frame = rendered(&mut controller, t0 + ms(600));
        assert!(all_pixels_are(&frame, Rgb::OFF));
    }

    #[test]
    fn changing_flag_restarts_scene() {
        let t0 = Instant::now();
        let mut controller = Controller::new(t0);
        controller.apply(flag(TrackFlag::Green, t0), t0);
        controller.apply(flag(TrackFlag::Yellow, t0 + ms(400)), t0 + ms(400));
        assert_eq!(controller.current_scene.started, t0 + ms(400));
    }

    #[test]
    fn non_flag_input_leaves_scene_unchanged() {
        let t0 = Instant::now();
        let mut controller = Controller::new(t0);
        controller.apply(flag(TrackFlag::Red, t0), t0);
        let before = controller.current_scene;
        controller.apply(Input::Net(crate::input::NetStatus::Online), t0 + ms(50));
        assert_eq!(controller.current_scene, before);
    }
}
