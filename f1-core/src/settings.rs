use serde::{Deserialize, Serialize};

use crate::{color::Rgb, drivers::DriverSet, input::RaceEvent, theme::LAMP};

/// The most bytes the settings JSON may take. The firmware reads saved
/// settings into a buffer this big; a longer entry can't be read and the lamp
/// falls back to defaults. A test below checks the worst case fits.
pub const MAX_JSON: usize = 1024;

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
    /// Drivers whose pit stops and overtakes the lamp shows. Replaces the
    /// old single `favourite_driver`, which saved settings may still contain;
    /// it's ignored, so the list starts empty.
    pub followed_drivers: DriverSet,
    /// Pit stops and overtakes of every driver, whatever the list says: for
    /// the showcase, or to see everything (a race has hundreds of overtakes).
    pub follow_all: bool,
    pub winner_display_ms: u32,
    /// How long a green flag shows before the lamp's own colour returns;
    /// 0 keeps it green. Green is news only for a moment, and a lamp that
    /// stays green all race hides the flags that matter.
    pub green_display_ms: u32,
    /// Which race events the lamp shows.
    pub effects: EffectToggles,
}

/// One switch per kind of race event, all on by default. Track flags aren't
/// here: they are the lamp's main job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
// A missing switch (older saved settings, or a partial JSON) means "on".
#[serde(default)]
pub struct EffectToggles {
    pub start_lights: bool,
    pub chequered_flag: bool,
    pub fastest_lap: bool,
    pub pit_stop: bool,
    pub overtake: bool,
    pub winner: bool,
}

impl Default for EffectToggles {
    fn default() -> Self {
        Self {
            start_lights: true,
            chequered_flag: true,
            fastest_lap: true,
            pit_stop: true,
            overtake: true,
            winner: true,
        }
    }
}

impl Settings {
    /// Whether the lamp shows `event`: its switch is on and, for pit stops and
    /// overtakes, the driver is followed (none are by default).
    pub fn shows(&self, event: RaceEvent) -> bool {
        let e = &self.effects;
        // No `_` arm: a new kind of event must get a decision here.
        match event {
            RaceEvent::TrackFlag(_) | RaceEvent::FlagCleared => true,
            RaceEvent::StartLights => e.start_lights,
            RaceEvent::ChequeredFlag => e.chequered_flag,
            RaceEvent::FastestLap { .. } => e.fastest_lap,
            RaceEvent::PitStop { driver, .. } => e.pit_stop && self.follows(driver),
            RaceEvent::Overtake { driver, .. } => e.overtake && self.follows(driver),
            RaceEvent::Winner { .. } => e.winner,
        }
    }
}

impl Settings {
    fn follows(&self, driver: u8) -> bool {
        self.follow_all || self.followed_drivers.contains(driver)
    }
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
            followed_drivers: DriverSet::new(),
            follow_all: false,
            winner_display_ms: 60000,
            green_display_ms: 10_000,
            effects: EffectToggles::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::MAX_DRIVER;

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
            followed_drivers: DriverSet::try_from(vec![1, 44]).unwrap(),
            follow_all: true,
            winner_display_ms: 90_000,
            green_display_ms: 5_000,
            effects: EffectToggles {
                fastest_lap: false,
                overtake: false,
                ..EffectToggles::default()
            },
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
        let json = r#"{"night_start": null, "night_end": null}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.night_start, None);
        assert_eq!(settings.night_end, None);
    }

    #[test]
    fn followed_drivers_are_stored_as_a_list() {
        let settings = Settings {
            followed_drivers: DriverSet::try_from(vec![44, 1]).unwrap(),
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains(r#""followed_drivers":[1,44]"#), "{json}");
    }

    #[test]
    fn no_followed_drivers_by_default() {
        assert!(Settings::default().followed_drivers.is_empty());
    }

    #[test]
    fn settings_saved_with_the_old_favourite_driver_still_load() {
        // Saved before `followed_drivers` existed: the old field is ignored,
        // everything else is kept.
        let json = r##"{"lamp_color":"#ffffff","tv_delay_ms":500,"favourite_driver":44}"##;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.lamp_color, Rgb::new(255, 255, 255));
        assert_eq!(settings.tv_delay_ms, 500);
        assert!(settings.followed_drivers.is_empty());
    }

    #[test]
    fn an_impossible_followed_driver_is_rejected() {
        let json = r#"{"followed_drivers":[44, 0]}"#;
        assert!(serde_json::from_str::<Settings>(json).is_err());
    }

    #[test]
    fn worst_case_settings_fit_the_nvs_buffer() {
        // Every field at its longest JSON form: floats with many digits,
        // the largest numbers, every option set.
        let settings = Settings {
            lamp_color: Rgb::new(255, 255, 255),
            lamp_brightness: 1.0 / 3.0,
            global_brightness: 2.0 / 3.0,
            night_start: Some(u16::MAX),
            night_end: Some(u16::MAX),
            night_brightness: 1.0 / 7.0,
            tv_delay_ms: u32::MAX,
            followed_drivers: DriverSet::try_from((1..=MAX_DRIVER).collect::<Vec<_>>()).unwrap(),
            follow_all: false,
            winner_display_ms: u32::MAX,
            green_display_ms: u32::MAX,
            // "false" is one character longer than "true".
            effects: EffectToggles {
                start_lights: false,
                chequered_flag: false,
                fastest_lap: false,
                pit_stop: false,
                overtake: false,
                winner: false,
            },
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.len() < MAX_JSON, "{} bytes: {json}", json.len());
    }

    // ---- Effect toggles ----

    #[test]
    fn every_effect_is_on_by_default() {
        let e = Settings::default().effects;
        assert!(
            e.start_lights
                && e.chequered_flag
                && e.fastest_lap
                && e.pit_stop
                && e.overtake
                && e.winner
        );
    }

    #[test]
    fn settings_saved_before_toggles_existed_have_every_effect_on() {
        let json = r##"{"lamp_color":"#ffffff","tv_delay_ms":500}"##;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.effects, EffectToggles::default());
    }

    #[test]
    fn a_missing_toggle_means_on() {
        let json = r#"{"effects":{"fastest_lap":false}}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        let expected = EffectToggles {
            fastest_lap: false,
            ..EffectToggles::default()
        };
        assert_eq!(settings.effects, expected);
    }

    #[test]
    fn toggles_are_stored_by_name() {
        let settings = Settings {
            effects: EffectToggles {
                overtake: false,
                ..EffectToggles::default()
            },
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains(r#""overtake":false"#), "{json}");
        assert!(json.contains(r#""start_lights":true"#), "{json}");
    }

    #[test]
    fn a_toggle_that_is_not_a_bool_is_rejected() {
        let json = r#"{"effects":{"winner":"no"}}"#;
        assert!(serde_json::from_str::<Settings>(json).is_err());
    }

    // ---- Which events the lamp shows ----

    use crate::input::TrackFlag;

    const TEAM: Rgb = Rgb::new(0, 210, 190);

    fn pit(driver: u8) -> RaceEvent {
        RaceEvent::PitStop {
            driver,
            team_color: TEAM,
        }
    }

    fn overtake(driver: u8) -> RaceEvent {
        RaceEvent::Overtake {
            driver,
            team_color: TEAM,
        }
    }

    fn following(drivers: &[u8]) -> Settings {
        Settings {
            followed_drivers: DriverSet::try_from(drivers.to_vec()).unwrap(),
            ..Settings::default()
        }
    }

    fn with_effects(effects: EffectToggles) -> Settings {
        Settings {
            effects,
            ..Settings::default()
        }
    }

    #[test]
    fn defaults_show_every_event_except_driver_events() {
        let s = Settings::default();
        assert!(s.shows(RaceEvent::TrackFlag(TrackFlag::Yellow)));
        assert!(s.shows(RaceEvent::StartLights));
        assert!(s.shows(RaceEvent::ChequeredFlag));
        assert!(s.shows(RaceEvent::FastestLap {
            driver: 44,
            team_color: TEAM
        }));
        assert!(s.shows(RaceEvent::Winner {
            driver: 44,
            team_color: TEAM
        }));
        // Nobody is followed yet.
        assert!(!s.shows(pit(44)));
        assert!(!s.shows(overtake(44)));
    }

    #[test]
    fn pit_stops_and_overtakes_show_only_for_followed_drivers() {
        let s = following(&[1, 44]);
        assert!(s.shows(pit(44)));
        assert!(s.shows(overtake(1)));
        assert!(!s.shows(pit(63)));
        assert!(!s.shows(overtake(63)));
    }

    #[test]
    fn a_switched_off_event_is_not_shown() {
        let off = EffectToggles {
            start_lights: false,
            chequered_flag: false,
            fastest_lap: false,
            winner: false,
            ..EffectToggles::default()
        };
        let s = with_effects(off);
        assert!(!s.shows(RaceEvent::StartLights));
        assert!(!s.shows(RaceEvent::ChequeredFlag));
        assert!(!s.shows(RaceEvent::FastestLap {
            driver: 44,
            team_color: TEAM
        }));
        assert!(!s.shows(RaceEvent::Winner {
            driver: 44,
            team_color: TEAM
        }));
    }

    #[test]
    fn switching_off_pit_stops_hides_them_even_for_followed_drivers() {
        let s = Settings {
            effects: EffectToggles {
                pit_stop: false,
                ..EffectToggles::default()
            },
            ..following(&[44])
        };
        assert!(!s.shows(pit(44)));
        assert!(s.shows(overtake(44)));
    }

    #[test]
    fn fastest_lap_shows_for_any_driver() {
        assert!(following(&[44]).shows(RaceEvent::FastestLap {
            driver: 63,
            team_color: TEAM
        }));
    }

    #[test]
    fn track_flags_cannot_be_switched_off() {
        let all_off = EffectToggles {
            start_lights: false,
            chequered_flag: false,
            fastest_lap: false,
            pit_stop: false,
            overtake: false,
            winner: false,
        };
        assert!(with_effects(all_off).shows(RaceEvent::TrackFlag(TrackFlag::Red)));
    }

    #[test]
    fn settings_saved_before_green_display_get_the_default() {
        let settings: Settings = serde_json::from_str(r#"{"tv_delay_ms":500}"#).unwrap();
        assert_eq!(settings.green_display_ms, 10_000);
    }

    #[test]
    fn follow_all_shows_pit_stops_and_overtakes_of_everyone() {
        let s = Settings {
            follow_all: true,
            ..Settings::default()
        };
        assert!(s.shows(RaceEvent::PitStop {
            driver: 77,
            team_color: LAMP
        }));
        assert!(s.shows(RaceEvent::Overtake {
            driver: 77,
            team_color: LAMP
        }));
    }

    #[test]
    fn follow_all_still_respects_the_switches() {
        let mut s = Settings {
            follow_all: true,
            ..Settings::default()
        };
        s.effects.pit_stop = false;
        assert!(!s.shows(RaceEvent::PitStop {
            driver: 77,
            team_color: LAMP
        }));
    }

    #[test]
    fn follow_all_is_off_by_default_and_for_old_saved_settings() {
        assert!(!Settings::default().follow_all);
        let old: Settings = serde_json::from_str(r#"{"followed_drivers":[44]}"#).unwrap();
        assert!(!old.follow_all);
    }
}
