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

    pub const OFF: Rgb = Rgb::new(0, 0, 0);

    pub const RED: Rgb = Rgb::new(255, 0, 0);
    pub const GREEN: Rgb = Rgb::new(0, 255, 0);
    pub const BLUE: Rgb = Rgb::new(0, 0, 255);
    pub const YELLOW: Rgb = Rgb::new(255, 255, 0);
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);
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
}
