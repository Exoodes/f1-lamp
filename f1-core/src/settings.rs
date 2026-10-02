use serde::{Deserialize, Serialize};

use crate::{color::Rgb, theme::LAMP};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub lamp_color: Rgb,
    pub lamp_brightness: f32,
    pub global_brightness: f32,
    pub night_start: Option<u16>,
    pub night_end: Option<u16>,
    pub night_brightness: f32,
    pub tv_delay_ms: u32,
    pub favourite_driver: Option<u8>,
    pub winner_display_ms: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lamp_color: LAMP,
            lamp_brightness: 0.4,
            global_brightness: 0.6,
            night_start: None,
            night_end: None,
            night_brightness: 0.3,
            tv_delay_ms: 0,
            favourite_driver: None,
            winner_display_ms: 60000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_lamp_is_the_theme_lamp_colour() {
        assert_eq!(Settings::default().lamp_color, LAMP);
    }

    #[test]
    fn settings_survive_json_round_trip() {
        let settings = Settings {
            lamp_color: Rgb::new(10, 20, 30),
            lamp_brightness: 0.8,
            global_brightness: 0.5,
            night_start: Some(22 * 60),
            night_end: Some(7 * 60),
            night_brightness: 0.1,
            tv_delay_ms: 1500,
            favourite_driver: Some(44),
            winner_display_ms: 90_000,
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn missing_field_uses_default() {
        let settings: Settings = serde_json::from_str(r#"{"tv_delay_ms": 500}"#).unwrap();
        let expected = Settings {
            tv_delay_ms: 500,
            ..Settings::default()
        };
        assert_eq!(settings, expected);
    }

    #[test]
    fn empty_json_gives_defaults() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let settings: Settings = serde_json::from_str(r#"{"old_setting": true}"#).unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn lamp_colour_is_stored_as_hex_string() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains(r##""lamp_color":"#ffa03c""##), "{json}");
    }

    #[test]
    fn optional_fields_can_be_null() {
        let json = r#"{"night_start": null, "favourite_driver": null}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.night_start, None);
        assert_eq!(settings.favourite_driver, None);
    }
}
