//! What's set on the web page: colours, brightness, night mode, TV delay, and
//! which events show for which drivers. Saved in flash as JSON; missing fields
//! take their defaults, so settings saved by older firmware still load.

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::{color::Rgb, drivers::DriverSet, input::RaceEvent, schedule::SessionKind, theme::LAMP};

/// The most bytes the settings JSON may take. The firmware reads saved
/// settings into a buffer this big; a longer entry can't be read and the lamp
/// falls back to defaults. A test below checks the worst case fits.
pub const MAX_JSON: usize = 1024;

/// Streams run up to about a minute behind the feed. Every race event waits
/// this long in memory, so it can't be unlimited.
pub const MAX_TV_DELAY_MS: u32 = 120_000;
pub const MAX_WINNER_DISPLAY_MS: u32 = 60 * 60 * 1000;
pub const MAX_GREEN_DISPLAY_MS: u32 = 10 * 60 * 1000;
/// Night times are minutes after midnight.
pub const LAST_MINUTE: u16 = 24 * 60 - 1;

/// A setting outside what the lamp accepts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutOfRange {
    pub field: &'static str,
    pub max: f64,
}

impl fmt::Display for OutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} must be between 0 and {}", self.field, self.max)
    }
}

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
    /// Drivers whose pit stops and overtakes the lamp shows. Settings saved
    /// by older firmware may hold a single `favourite_driver` instead; like
    /// any unknown key it's ignored, so the list then starts empty.
    pub followed_drivers: DriverSet,
    /// Pit stops, overtakes, knockouts and penalties of every driver,
    /// whatever the list says: to see everything (a race has hundreds of
    /// overtakes).
    pub follow_all: bool,
    pub winner_display_ms: u32,
    /// How long a green flag shows before the lamp's own colour returns;
    /// 0 keeps it green. Green is news only for a moment, and a lamp that
    /// stays green all race hides the flags that matter.
    pub green_display_ms: u32,
    /// Which race events the lamp shows.
    pub effects: EffectToggles,
    /// Which kinds of session the lamp follows.
    pub sessions: SessionToggles,
}

/// One switch per kind of session, all on by default. A session switched
/// off is as if it weren't in the calendar: no countdown, no live feed, no
/// winner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionToggles {
    pub practice: bool,
    pub qualifying: bool,
    pub sprint_qualifying: bool,
    pub sprint: bool,
    pub race: bool,
}

impl Default for SessionToggles {
    fn default() -> Self {
        Self {
            practice: true,
            qualifying: true,
            sprint_qualifying: true,
            sprint: true,
            race: true,
        }
    }
}

impl SessionToggles {
    pub fn follows(self, kind: SessionKind) -> bool {
        match kind {
            SessionKind::Practice => self.practice,
            SessionKind::Qualifying => self.qualifying,
            SessionKind::SprintQualifying => self.sprint_qualifying,
            SessionKind::Sprint => self.sprint,
            SessionKind::Race => self.race,
        }
    }
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
    /// The race's winner; the fastest after each qualifying part (pole after
    /// the last) and after practice.
    pub winner: bool,
    pub knocked_out: bool,
    pub last_lap: bool,
    /// A followed driver's penalty.
    pub penalty: bool,
    /// The countdown before each session (not a race event).
    pub countdown: bool,
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
            knocked_out: true,
            last_lap: true,
            penalty: true,
            countdown: true,
        }
    }
}

impl Settings {
    /// Whether the lamp shows `event`: its switch is on and, for pit stops,
    /// overtakes, knockouts and penalties, the driver is followed (none are by
    /// default).
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
            RaceEvent::KnockedOut { driver, .. } => e.knocked_out && self.follows(driver),
            RaceEvent::LastLap => e.last_lap,
            RaceEvent::Penalty { driver, .. } => e.penalty && self.follows(driver),
        }
    }
}

impl Settings {
    fn follows(&self, driver: u8) -> bool {
        self.follow_all || self.followed_drivers.contains(driver)
    }

    /// For settings sent from the page: the first field out of range.
    pub fn validate(&self) -> Result<(), OutOfRange> {
        let minute = |m: Option<u16>| m.map_or(0.0, f64::from);
        let limited = [
            ("lamp_brightness", f64::from(self.lamp_brightness), 1.0),
            ("global_brightness", f64::from(self.global_brightness), 1.0),
            ("night_brightness", f64::from(self.night_brightness), 1.0),
            (
                "night_start",
                minute(self.night_start),
                f64::from(LAST_MINUTE),
            ),
            ("night_end", minute(self.night_end), f64::from(LAST_MINUTE)),
            (
                "tv_delay_ms",
                f64::from(self.tv_delay_ms),
                f64::from(MAX_TV_DELAY_MS),
            ),
            (
                "winner_display_ms",
                f64::from(self.winner_display_ms),
                f64::from(MAX_WINNER_DISPLAY_MS),
            ),
            (
                "green_display_ms",
                f64::from(self.green_display_ms),
                f64::from(MAX_GREEN_DISPLAY_MS),
            ),
        ];
        match limited
            .into_iter()
            .find(|&(_, value, max)| !(0.0..=max).contains(&value))
        {
            Some((field, _, max)) => Err(OutOfRange { field, max }),
            None => Ok(()),
        }
    }

    /// For settings saved by firmware that didn't check them: every field
    /// brought into range.
    #[must_use]
    pub fn clamped(self) -> Self {
        Settings {
            lamp_brightness: self.lamp_brightness.clamp(0.0, 1.0),
            global_brightness: self.global_brightness.clamp(0.0, 1.0),
            night_brightness: self.night_brightness.clamp(0.0, 1.0),
            night_start: self.night_start.map(|m| m.min(LAST_MINUTE)),
            night_end: self.night_end.map(|m| m.min(LAST_MINUTE)),
            tv_delay_ms: self.tv_delay_ms.min(MAX_TV_DELAY_MS),
            winner_display_ms: self.winner_display_ms.min(MAX_WINNER_DISPLAY_MS),
            green_display_ms: self.green_display_ms.min(MAX_GREEN_DISPLAY_MS),
            ..self
        }
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
            sessions: SessionToggles::default(),
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
            sessions: SessionToggles {
                practice: false,
                ..SessionToggles::default()
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
                knocked_out: false,
                last_lap: false,
                penalty: false,
                countdown: false,
            },
            sessions: SessionToggles {
                practice: false,
                qualifying: false,
                sprint_qualifying: false,
                sprint: false,
                race: false,
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
                && e.penalty
                && e.winner
                && e.knocked_out
                && e.last_lap
        );
    }

    #[test]
    fn effects_saved_before_last_lap_existed_show_it() {
        let json = r#"{"effects":{"start_lights":true,"chequered_flag":false}}"#;
        let settings: Settings = serde_json::from_str(json).unwrap();
        assert!(settings.effects.last_lap);
        assert!(!settings.effects.chequered_flag);
    }

    #[test]
    fn settings_saved_before_session_switches_follow_every_session() {
        let settings: Settings = serde_json::from_str(r#"{"tv_delay_ms":500}"#).unwrap();
        assert_eq!(settings.sessions, SessionToggles::default());
    }

    #[test]
    fn each_session_switch_covers_its_own_kind() {
        let practice_off = SessionToggles {
            practice: false,
            ..SessionToggles::default()
        };
        assert!(!practice_off.follows(SessionKind::Practice));
        for kind in [
            SessionKind::Qualifying,
            SessionKind::SprintQualifying,
            SessionKind::Sprint,
            SessionKind::Race,
        ] {
            assert!(practice_off.follows(kind), "{kind:?}");
        }
        let sprint_off: SessionToggles = serde_json::from_str(r#"{"sprint":false}"#).unwrap();
        assert!(!sprint_off.follows(SessionKind::Sprint));
        assert!(sprint_off.follows(SessionKind::Race));
    }

    #[test]
    fn effects_saved_before_the_countdown_existed_show_it() {
        let settings: Settings = serde_json::from_str(r#"{"effects":{"last_lap":false}}"#).unwrap();
        assert!(settings.effects.countdown);
        assert!(!settings.effects.last_lap);
    }

    #[test]
    fn last_lap_can_be_switched_off() {
        let off = EffectToggles {
            last_lap: false,
            ..EffectToggles::default()
        };
        assert!(!with_effects(off).shows(RaceEvent::LastLap));
        assert!(Settings::default().shows(RaceEvent::LastLap));
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
    fn penalties_show_only_for_followed_drivers_and_can_be_switched_off() {
        let penalty = |driver| RaceEvent::Penalty {
            driver,
            team_color: TEAM,
        };
        let mut s = following(&[11]);
        assert!(s.shows(penalty(11)));
        assert!(!s.shows(penalty(63)));
        s.effects.penalty = false;
        assert!(!s.shows(penalty(11)));
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
            knocked_out: false,
            last_lap: false,
            penalty: false,
            countdown: false,
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

    // ---- Limits ----

    fn at_the_limits() -> Settings {
        Settings {
            lamp_brightness: 1.0,
            global_brightness: 0.0,
            night_brightness: 1.0,
            night_start: Some(LAST_MINUTE),
            night_end: Some(0),
            tv_delay_ms: MAX_TV_DELAY_MS,
            winner_display_ms: MAX_WINNER_DISPLAY_MS,
            green_display_ms: MAX_GREEN_DISPLAY_MS,
            ..Settings::default()
        }
    }

    #[test]
    fn defaults_and_the_limits_themselves_are_valid() {
        assert_eq!(Settings::default().validate(), Ok(()));
        assert_eq!(at_the_limits().validate(), Ok(()));
    }

    #[test]
    fn a_tv_delay_above_the_limit_is_rejected_by_name() {
        let s = Settings {
            tv_delay_ms: MAX_TV_DELAY_MS + 1,
            ..Settings::default()
        };
        let e = s.validate().unwrap_err();
        assert_eq!(e.field, "tv_delay_ms");
        assert_eq!(e.to_string(), "tv_delay_ms must be between 0 and 120000");
    }

    #[test]
    fn every_limited_field_is_checked() {
        let too_far = [
            Settings {
                lamp_brightness: 1.5,
                ..Settings::default()
            },
            Settings {
                global_brightness: -0.1,
                ..Settings::default()
            },
            Settings {
                night_brightness: 2.0,
                ..Settings::default()
            },
            Settings {
                night_start: Some(LAST_MINUTE + 1),
                ..Settings::default()
            },
            Settings {
                night_end: Some(u16::MAX),
                ..Settings::default()
            },
            Settings {
                winner_display_ms: MAX_WINNER_DISPLAY_MS + 1,
                ..Settings::default()
            },
            Settings {
                green_display_ms: MAX_GREEN_DISPLAY_MS + 1,
                ..Settings::default()
            },
        ];
        for s in too_far {
            assert!(s.validate().is_err(), "{s:?}");
        }
    }

    #[test]
    fn clamping_brings_every_field_into_range() {
        let wild = Settings {
            lamp_brightness: 7.0,
            global_brightness: -1.0,
            night_brightness: 3.0,
            night_start: Some(u16::MAX),
            night_end: Some(u16::MAX),
            tv_delay_ms: u32::MAX,
            winner_display_ms: u32::MAX,
            green_display_ms: u32::MAX,
            ..Settings::default()
        };
        let expected = Settings {
            night_end: Some(LAST_MINUTE),
            ..at_the_limits()
        };
        assert_eq!(wild.clamped(), expected);
    }

    #[test]
    fn clamping_leaves_valid_settings_alone() {
        assert_eq!(at_the_limits().clamped(), at_the_limits());
        assert_eq!(Settings::default().clamped(), Settings::default());
    }
}
