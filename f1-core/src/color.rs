//! [`Rgb`]: a colour, read and written as `"#rrggbb"`, with scaling and
//! blending for the effects.

use core::fmt;
use core::str::FromStr;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseRgbError {
    WrongLength,
    NotAscii,
    BadDigit,
}

impl fmt::Display for ParseRgbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseRgbError::WrongLength => write!(f, "expected 6 hex digits, like #ff8800"),
            ParseRgbError::NotAscii => write!(f, "colour contains non-ASCII characters"),
            ParseRgbError::BadDigit => write!(f, "colour contains a non-hex digit"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
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

impl FromStr for Rgb {
    type Err = ParseRgbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if !hex.is_ascii() {
            return Err(ParseRgbError::NotAscii);
        }

        if hex.len() != 6 {
            return Err(ParseRgbError::WrongLength);
        }

        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ParseRgbError::BadDigit);
        }

        Ok(Rgb::new(
            channel(hex, 0)?,
            channel(hex, 2)?,
            channel(hex, 4)?,
        ))
    }
}

impl TryFrom<String> for Rgb {
    type Error = ParseRgbError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Rgb> for String {
    fn from(c: Rgb) -> String {
        c.to_string()
    }
}

fn channel(hex: &str, start: usize) -> Result<u8, ParseRgbError> {
    u8::from_str_radix(&hex[start..start + 2], 16).map_err(|_| ParseRgbError::BadDigit)
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

fn lerp_channel(a: u8, b: u8, t: f32) -> u8 {
    let (a, b) = (f32::from(a), f32::from(b));
    to_channel(a + (b - a) * t)
}

pub(crate) fn to_channel(value: f32) -> u8 {
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

    #[test]
    fn parses_hex_with_hash() {
        assert_eq!("#ff8800".parse::<Rgb>(), Ok(Rgb::new(255, 136, 0)));
    }

    #[test]
    fn parses_hex_without_hash() {
        assert_eq!("ff8800".parse::<Rgb>(), Ok(Rgb::new(255, 136, 0)));
    }

    #[test]
    fn parses_uppercase_digits() {
        assert_eq!("#FF8800".parse::<Rgb>(), Ok(Rgb::new(255, 136, 0)));
    }

    #[test]
    fn parses_black_and_white() {
        assert_eq!("#000000".parse::<Rgb>(), Ok(Rgb::OFF));
        assert_eq!("#ffffff".parse::<Rgb>(), Ok(Rgb::WHITE));
    }

    #[test]
    fn rejects_too_short() {
        assert_eq!("#fff".parse::<Rgb>(), Err(ParseRgbError::WrongLength));
    }

    #[test]
    fn rejects_too_long() {
        assert_eq!("#ff88001".parse::<Rgb>(), Err(ParseRgbError::WrongLength));
    }

    #[test]
    fn rejects_empty_string() {
        assert_eq!("".parse::<Rgb>(), Err(ParseRgbError::WrongLength));
    }

    #[test]
    fn rejects_lone_hash() {
        assert_eq!("#".parse::<Rgb>(), Err(ParseRgbError::WrongLength));
    }

    #[test]
    fn strips_only_one_hash() {
        assert_eq!("##ff8800".parse::<Rgb>(), Err(ParseRgbError::WrongLength));
    }

    #[test]
    fn rejects_non_hex_digits() {
        assert_eq!("#gg0000".parse::<Rgb>(), Err(ParseRgbError::BadDigit));
    }

    #[test]
    fn rejects_plus_sign_inside_channel() {
        assert_eq!("+f+f+f".parse::<Rgb>(), Err(ParseRgbError::BadDigit));
    }

    #[test]
    fn rejects_surrounding_whitespace() {
        assert_eq!(" ff8800".parse::<Rgb>(), Err(ParseRgbError::WrongLength));
        assert_eq!("#ff 800".parse::<Rgb>(), Err(ParseRgbError::BadDigit));
    }

    #[test]
    fn rejects_non_ascii_without_panicking() {
        // 'é' is two bytes, so this is six bytes and slicing at 2 would cut it.
        assert_eq!("#féf0f".parse::<Rgb>(), Err(ParseRgbError::NotAscii));
    }

    #[test]
    fn displays_lowercase_with_hash() {
        assert_eq!(Rgb::new(255, 136, 0).to_string(), "#ff8800");
    }

    #[test]
    fn displays_small_channels_with_leading_zero() {
        assert_eq!(Rgb::new(0, 10, 0).to_string(), "#000a00");
    }

    #[test]
    fn display_then_parse_round_trips() {
        for c in [
            Rgb::OFF,
            Rgb::WHITE,
            Rgb::RED,
            Rgb::new(1, 2, 3),
            Rgb::new(255, 160, 60),
        ] {
            assert_eq!(c.to_string().parse::<Rgb>(), Ok(c));
        }
    }

    #[test]
    fn error_messages_are_not_empty() {
        for e in [
            ParseRgbError::WrongLength,
            ParseRgbError::NotAscii,
            ParseRgbError::BadDigit,
        ] {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn serialises_as_hex_string() {
        let json = serde_json::to_string(&Rgb::new(255, 136, 0)).unwrap();
        assert_eq!(json, r##""#ff8800""##);
    }

    #[test]
    fn deserialises_from_hex_string() {
        let c: Rgb = serde_json::from_str(r##""#ff8800""##).unwrap();
        assert_eq!(c, Rgb::new(255, 136, 0));
    }

    #[test]
    fn deserialising_bad_hex_fails() {
        assert!(serde_json::from_str::<Rgb>(r##""#zz0000""##).is_err());
    }

    #[test]
    fn deserialising_old_channel_object_fails() {
        // Settings saved before hex colours fall back to defaults in load().
        assert!(serde_json::from_str::<Rgb>(r#"{"r":1,"g":2,"b":3}"#).is_err());
    }
}
