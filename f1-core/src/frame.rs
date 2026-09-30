use crate::color::Rgb;

pub const NUM_LEDS: usize = 23;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame([Rgb; NUM_LEDS]);

impl Frame {
    pub const fn new() -> Self {
        Self([Rgb::OFF; NUM_LEDS])
    }

    pub fn fill(&mut self, color: Rgb) {
        self.0.fill(color);
    }

    pub fn set(&mut self, index: usize, color: Rgb) {
        if let Some(led) = self.0.get_mut(index) {
            *led = color;
        }
    }

    pub fn pixels(&self) -> &[Rgb] {
        &self.0
    }

    pub fn pixels_mut(&mut self) -> &mut [Rgb] {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_frame_is_all_off() {
        let frame = Frame::new();
        assert!(frame.pixels().iter().all(|&c| c == Rgb::OFF));
        assert_eq!(frame.pixels().len(), NUM_LEDS);
    }

    #[test]
    fn default_frame_is_all_off() {
        let frame = Frame::default();
        assert!(frame.pixels().iter().all(|&c| c == Rgb::OFF));
    }

    #[test]
    fn fill_sets_all_pixels() {
        let mut frame = Frame::new();
        let color = Rgb::new(1, 2, 3);
        frame.fill(color);
        assert!(frame.pixels().iter().all(|&c| c == color));
    }

    #[test]
    fn set_updates_single_pixel() {
        let mut frame = Frame::new();
        let color = Rgb::new(10, 20, 30);
        frame.set(5, color);
        for (i, &c) in frame.pixels().iter().enumerate() {
            if i == 5 {
                assert_eq!(c, color);
            } else {
                assert_eq!(c, Rgb::OFF);
            }
        }
    }

    #[test]
    fn set_out_of_bounds_does_nothing() {
        let mut frame = Frame::new();
        let color = Rgb::new(10, 20, 30);
        frame.set(NUM_LEDS, color);
        frame.set(NUM_LEDS + 100, color);
        assert!(frame.pixels().iter().all(|&c| c == Rgb::OFF));
    }

    #[test]
    fn pixels_returns_correct_length() {
        let frame = Frame::new();
        assert_eq!(frame.pixels().len(), NUM_LEDS);
    }

    #[test]
    fn frames_with_same_content_are_equal() {
        let mut a = Frame::new();
        let mut b = Frame::new();
        a.set(0, Rgb::new(1, 1, 1));
        b.set(0, Rgb::new(1, 1, 1));
        assert_eq!(a, b);
    }

    #[test]
    fn frames_with_different_content_are_not_equal() {
        let mut a = Frame::new();
        let b = Frame::new();
        a.set(0, Rgb::new(1, 1, 1));
        assert_ne!(a, b);
    }
}
