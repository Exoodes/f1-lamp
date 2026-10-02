use serde::Deserialize;

use crate::{
    color::Rgb,
    effect::Effect,
    input::{Input, TrackFlag},
    theme::flag_effect,
};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverrideRequest {
    Flag { flag: TrackFlag },
    Color { color: Rgb },
    Release,
}

impl OverrideRequest {
    pub fn into_input(self) -> Input {
        match self {
            OverrideRequest::Flag { flag } => Input::Override(Some(flag_effect(flag))),
            OverrideRequest::Color { color } => Input::Override(Some(Effect::Solid(color))),
            OverrideRequest::Release => Input::Override(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<OverrideRequest, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn parses_flag_override() {
        assert_eq!(
            parse(r#"{"kind":"flag","flag":"red"}"#).unwrap(),
            OverrideRequest::Flag {
                flag: TrackFlag::Red
            }
        );
    }

    #[test]
    fn parses_every_flag_name() {
        let flags = [
            ("green", TrackFlag::Green),
            ("yellow", TrackFlag::Yellow),
            ("double_yellow", TrackFlag::DoubleYellow),
            ("safety_car", TrackFlag::SafetyCar),
            ("virtual_safety_car", TrackFlag::VirtualSafetyCar),
            ("red", TrackFlag::Red),
        ];
        for (name, flag) in flags {
            let json = format!(r#"{{"kind":"flag","flag":"{name}"}}"#);
            assert_eq!(
                parse(&json).unwrap(),
                OverrideRequest::Flag { flag },
                "{name}"
            );
        }
    }

    #[test]
    fn parses_colour_override() {
        assert_eq!(
            parse(r##"{"kind":"color","color":"#00ff00"}"##).unwrap(),
            OverrideRequest::Color { color: Rgb::GREEN }
        );
    }

    #[test]
    fn parses_release() {
        assert_eq!(
            parse(r#"{"kind":"release"}"#).unwrap(),
            OverrideRequest::Release
        );
    }

    #[test]
    fn parses_kind_in_any_position() {
        assert_eq!(
            parse(r#"{"flag":"yellow","kind":"flag"}"#).unwrap(),
            OverrideRequest::Flag {
                flag: TrackFlag::Yellow
            }
        );
    }

    #[test]
    fn ignores_unknown_extra_fields() {
        assert_eq!(
            parse(r#"{"kind":"release","by":"phone"}"#).unwrap(),
            OverrideRequest::Release
        );
    }

    #[test]
    fn rejects_unknown_kind() {
        assert!(parse(r#"{"kind":"party"}"#).is_err());
    }

    #[test]
    fn rejects_missing_kind() {
        assert!(parse(r#"{"flag":"red"}"#).is_err());
    }

    #[test]
    fn rejects_kind_in_wrong_case() {
        assert!(parse(r#"{"kind":"Flag","flag":"red"}"#).is_err());
    }

    #[test]
    fn rejects_flag_without_flag_field() {
        assert!(parse(r#"{"kind":"flag"}"#).is_err());
    }

    #[test]
    fn rejects_unknown_flag_name() {
        assert!(parse(r#"{"kind":"flag","flag":"purple"}"#).is_err());
    }

    #[test]
    fn rejects_flag_name_in_wrong_case() {
        assert!(parse(r#"{"kind":"flag","flag":"Red"}"#).is_err());
    }

    #[test]
    fn rejects_bad_hex_colour() {
        assert!(parse(r##"{"kind":"color","color":"#zz0000"}"##).is_err());
        assert!(parse(r##"{"kind":"color","color":"#fff"}"##).is_err());
    }

    #[test]
    fn rejects_colour_without_colour_field() {
        assert!(parse(r#"{"kind":"color"}"#).is_err());
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse("garbage").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn rejects_non_object_json() {
        assert!(parse(r#""release""#).is_err());
        assert!(parse("[]").is_err());
    }

    #[test]
    fn rejection_message_names_the_problem() {
        let message = parse(r#"{"kind":"party"}"#).unwrap_err().to_string();
        assert!(message.contains("party"), "{message}");
    }

    #[test]
    fn flag_override_uses_theme_flag_effect() {
        let input = OverrideRequest::Flag {
            flag: TrackFlag::Red,
        }
        .into_input();
        assert_eq!(input, Input::Override(Some(flag_effect(TrackFlag::Red))));
    }

    #[test]
    fn colour_override_is_solid() {
        let input = OverrideRequest::Color { color: Rgb::BLUE }.into_input();
        assert_eq!(input, Input::Override(Some(Effect::Solid(Rgb::BLUE))));
    }

    #[test]
    fn release_clears_override() {
        assert_eq!(OverrideRequest::Release.into_input(), Input::Override(None));
    }
}
