#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    #[must_use]
    pub fn scale(self, factor: f32) -> Rgb {
        let factor = factor.clamp(0.0, 1.0);
        Rgb::new(
            to_channel(f32::from(self.r) * factor),
            to_channel(f32::from(self.g) * factor),
            to_channel(f32::from(self.b) * factor),
        )
    }

    #[must_use]
    pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        Rgb::new(
            lerp_channel(a.r, b.r, t),
            lerp_channel(a.g, b.g, t),
            lerp_channel(a.b, b.b, t),
        )
    }

    pub const OFF: Rgb = Rgb::new(0, 0, 0);

    pub const RED: Rgb = Rgb::new(255, 0, 0);
    pub const GREEN: Rgb = Rgb::new(0, 255, 0);
    pub const BLUE: Rgb = Rgb::new(0, 0, 255);
    pub const YELLOW: Rgb = Rgb::new(255, 255, 0);
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);
}

fn lerp_channel(a: u8, b: u8, t: f32) -> u8 {
    let (a, b) = (f32::from(a), f32::from(b));
    to_channel(a + (b - a) * t)
}

fn to_channel(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_each_channel() {
        let c = Rgb::new(10, 20, 30);
        assert_eq!(c.r, 10);
        assert_eq!(c.g, 20);
        assert_eq!(c.b, 30);
    }

    #[test]
    fn default_is_off() {
        let c = Rgb::default();
        assert_eq!(c, Rgb::OFF);
    }

    #[test]
    fn constants_have_expected_channels() {
        assert_eq!(Rgb::OFF, Rgb::new(0, 0, 0));
        assert_eq!(Rgb::RED, Rgb::new(255, 0, 0));
        assert_eq!(Rgb::GREEN, Rgb::new(0, 255, 0));
        assert_eq!(Rgb::BLUE, Rgb::new(0, 0, 255));
        assert_eq!(Rgb::YELLOW, Rgb::new(255, 255, 0));
        assert_eq!(Rgb::WHITE, Rgb::new(255, 255, 255));
    }

    #[test]
    fn equal_only_when_all_channels_match() {
        assert_eq!(Rgb::new(1, 2, 3), Rgb::new(1, 2, 3));
        assert_ne!(Rgb::new(1, 2, 3), Rgb::new(3, 2, 1));
    }

    #[test]
    fn copy_and_clone_give_equal_values() {
        let c1 = Rgb::new(1, 2, 3);
        let c2 = c1;
        let c3 = c1.clone();
        assert_eq!(c1, c2);
        assert_eq!(c1, c3);
    }

    #[test]
    fn scale_half_halves_each_channel_with_rounding() {
        // 255 * 0.5 = 127.5 and 1 * 0.5 = 0.5 both round up.
        let c = Rgb::new(200, 255, 1).scale(0.5);
        assert_eq!(c, Rgb::new(100, 128, 1));
    }

    #[test]
    fn scale_one_leaves_colour_unchanged() {
        let c = Rgb::new(10, 128, 255);
        assert_eq!(c.scale(1.0), c);
    }

    #[test]
    fn scale_zero_is_off() {
        assert_eq!(Rgb::WHITE.scale(0.0), Rgb::OFF);
    }

    #[test]
    fn scale_above_one_leaves_colour_unchanged() {
        let c = Rgb::new(255, 128, 0);
        assert_eq!(c.scale(2.0), c);
    }

    #[test]
    fn scale_below_zero_is_off() {
        assert_eq!(Rgb::WHITE.scale(-1.0), Rgb::OFF);
    }

    #[test]
    fn lerp_at_zero_returns_start() {
        assert_eq!(Rgb::lerp(Rgb::RED, Rgb::BLUE, 0.0), Rgb::RED);
    }

    #[test]
    fn lerp_at_one_returns_end() {
        assert_eq!(Rgb::lerp(Rgb::RED, Rgb::BLUE, 1.0), Rgb::BLUE);
    }

    #[test]
    fn lerp_at_half_is_midpoint() {
        assert_eq!(
            Rgb::lerp(Rgb::OFF, Rgb::WHITE, 0.5),
            Rgb::new(128, 128, 128)
        );
    }

    #[test]
    fn lerp_towards_darker_colour_decreases_channels() {
        // 255 + (0 - 255) * 0.25 = 191.25
        assert_eq!(
            Rgb::lerp(Rgb::WHITE, Rgb::OFF, 0.25),
            Rgb::new(191, 191, 191)
        );
    }

    #[test]
    fn lerp_outside_zero_to_one_clamps_to_endpoints() {
        assert_eq!(Rgb::lerp(Rgb::RED, Rgb::BLUE, -1.0), Rgb::RED);
        assert_eq!(Rgb::lerp(Rgb::RED, Rgb::BLUE, 2.0), Rgb::BLUE);
    }
}
