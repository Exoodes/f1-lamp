//! [`OverrideRequest`]: forcing a flag or a colour, or trying an event, from
//! the web page, as JSON.

use serde::Deserialize;

use crate::{
    color::Rgb,
    effect::Effect,
    input::{Input, RaceEvent, TrackFlag},
    theme::flag_effect,
};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverrideRequest {
    Flag {
        flag: TrackFlag,
    },
    Color {
        color: Rgb,
    },
    Release,
    /// Plays one event's effect once, in `team`'s colour for the driver
    /// events.
    Event {
        event: PreviewEvent,
        #[serde(default)]
        team: Team,
    },
}

/// The events the page can try. Flags have their own buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewEvent {
    StartLights,
    ChequeredFlag,
    LastLap,
    FastestLap,
    PitStop,
    Overtake,
    Winner,
    KnockedOut,
    Penalty,
    Retired,
}

/// The 2026 teams, so a preview can only use a real team colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Team {
    Alpine,
    AstonMartin,
    Audi,
    Cadillac,
    Ferrari,
    Haas,
    #[default]
    #[serde(rename = "mclaren")]
    McLaren,
    Mercedes,
    RacingBulls,
    RedBull,
    Williams,
}

impl Team {
    /// As the feed's driver list sends them (Singapore 2026).
    pub const fn color(self) -> Rgb {
        match self {
            Team::Alpine => Rgb::new(0x00, 0xA1, 0xE8),
            Team::AstonMartin => Rgb::new(0x22, 0x99, 0x71),
            Team::Audi => Rgb::new(0xF5, 0x05, 0x37),
            Team::Cadillac => Rgb::new(0x90, 0x90, 0x90),
            Team::Ferrari => Rgb::new(0xED, 0x11, 0x31),
            Team::Haas => Rgb::new(0x9C, 0x9F, 0xA2),
            Team::McLaren => Rgb::new(0xF4, 0x76, 0x00),
            Team::Mercedes => Rgb::new(0x00, 0xD7, 0xB6),
            Team::RacingBulls => Rgb::new(0x6C, 0x98, 0xFF),
            Team::RedBull => Rgb::new(0x47, 0x81, 0xD7),
            Team::Williams => Rgb::new(0x18, 0x68, 0xDB),
        }
    }
}

/// No real driver has number 0, so nothing mistakes a preview for one.
const PREVIEW_DRIVER: u8 = 0;

impl PreviewEvent {
    pub fn race_event(self, team_color: Rgb) -> RaceEvent {
        let driver = PREVIEW_DRIVER;
        match self {
            PreviewEvent::StartLights => RaceEvent::StartLights,
            PreviewEvent::ChequeredFlag => RaceEvent::ChequeredFlag,
            PreviewEvent::LastLap => RaceEvent::LastLap,
            PreviewEvent::FastestLap => RaceEvent::FastestLap { driver, team_color },
            PreviewEvent::PitStop => RaceEvent::PitStop { driver, team_color },
            PreviewEvent::Overtake => RaceEvent::Overtake { driver, team_color },
            PreviewEvent::Winner => RaceEvent::Winner { driver, team_color },
            PreviewEvent::KnockedOut => RaceEvent::KnockedOut { driver, team_color },
            PreviewEvent::Penalty => RaceEvent::Penalty { driver, team_color },
            PreviewEvent::Retired => RaceEvent::Retired { driver, team_color },
        }
    }
}

impl OverrideRequest {
    pub fn into_input(self) -> Input {
        match self {
            OverrideRequest::Flag { flag } => Input::Override(Some(flag_effect(flag))),
            OverrideRequest::Color { color } => Input::Override(Some(Effect::Solid(color))),
            OverrideRequest::Release => Input::Override(None),
            OverrideRequest::Event { event, team } => {
                Input::Preview(event.race_event(team.color()))
            }
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
            ("vsc_ending", TrackFlag::VscEnding),
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
    fn parses_every_event_name() {
        let events = [
            ("start_lights", PreviewEvent::StartLights),
            ("chequered_flag", PreviewEvent::ChequeredFlag),
            ("last_lap", PreviewEvent::LastLap),
            ("fastest_lap", PreviewEvent::FastestLap),
            ("pit_stop", PreviewEvent::PitStop),
            ("overtake", PreviewEvent::Overtake),
            ("winner", PreviewEvent::Winner),
            ("knocked_out", PreviewEvent::KnockedOut),
            ("penalty", PreviewEvent::Penalty),
            ("retired", PreviewEvent::Retired),
        ];
        for (name, event) in events {
            let json = format!(r#"{{"kind":"event","event":"{name}","team":"ferrari"}}"#);
            assert_eq!(
                parse(&json).unwrap(),
                OverrideRequest::Event {
                    event,
                    team: Team::Ferrari
                },
                "{name}"
            );
        }
    }

    #[test]
    fn parses_every_team_name() {
        let teams = [
            ("alpine", Team::Alpine),
            ("aston_martin", Team::AstonMartin),
            ("audi", Team::Audi),
            ("cadillac", Team::Cadillac),
            ("ferrari", Team::Ferrari),
            ("haas", Team::Haas),
            ("mclaren", Team::McLaren),
            ("mercedes", Team::Mercedes),
            ("racing_bulls", Team::RacingBulls),
            ("red_bull", Team::RedBull),
            ("williams", Team::Williams),
        ];
        for (name, team) in teams {
            let json = format!(r#"{{"kind":"event","event":"pit_stop","team":"{name}"}}"#);
            assert_eq!(
                parse(&json).unwrap(),
                OverrideRequest::Event {
                    event: PreviewEvent::PitStop,
                    team
                },
                "{name}"
            );
        }
    }

    #[test]
    fn event_without_team_is_mclaren() {
        assert_eq!(
            parse(r#"{"kind":"event","event":"pit_stop"}"#).unwrap(),
            OverrideRequest::Event {
                event: PreviewEvent::PitStop,
                team: Team::McLaren
            }
        );
    }

    #[test]
    fn rejects_unknown_team() {
        assert!(parse(r#"{"kind":"event","event":"pit_stop","team":"brawn"}"#).is_err());
    }

    #[test]
    fn rejects_a_colour_instead_of_a_team() {
        assert!(parse(r##"{"kind":"event","event":"pit_stop","team":"#ff0000"}"##).is_err());
    }

    #[test]
    fn rejects_unknown_event_name() {
        assert!(parse(r#"{"kind":"event","event":"party"}"#).is_err());
    }

    #[test]
    fn rejects_flag_as_event() {
        assert!(parse(r#"{"kind":"event","event":"red"}"#).is_err());
    }

    #[test]
    fn event_is_a_preview_in_the_team_colour() {
        let input = OverrideRequest::Event {
            event: PreviewEvent::Overtake,
            team: Team::Williams,
        }
        .into_input();
        assert_eq!(
            input,
            Input::Preview(RaceEvent::Overtake {
                driver: PREVIEW_DRIVER,
                team_color: Rgb::new(0x18, 0x68, 0xDB)
            })
        );
    }

    #[test]
    fn release_clears_override() {
        assert_eq!(OverrideRequest::Release.into_input(), Input::Override(None));
    }
}
