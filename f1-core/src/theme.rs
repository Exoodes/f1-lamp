//! Which effect and colour each event and state gets: the flags, the overlays
//! (start lights, chequered flag, flashes), the winner and the network status.
//! The lamp's look is changed here.

use crate::{
    color::Rgb,
    effect::Effect,
    input::{NetStatus, RaceEvent, TrackFlag},
};

pub const LAMP: Rgb = Rgb::new(255, 160, 60);
pub const PURPLE: Rgb = Rgb::new(160, 0, 255);
pub const NET_BLUE: Rgb = Rgb::new(0, 80, 255);

const DOUBLE_YELLOW_PERIOD_MS: u32 = 400;
const SC_CHASE_SPEED: f32 = 12.0;
const SC_CHASE_TAIL: u8 = 4;
const VSC_PERIOD_MS: u32 = 3000;

const FASTEST_LAP_FLASHES: u8 = 3;
const PIT_STOP_FLASHES: u8 = 2;
const OVERTAKE_FLASHES: u8 = 1;
const CHEQUERED_MS: u32 = 10_000;
const START_HOLD_MS: u32 = 2000;

const WINNER_CHASE_SPEED: f32 = 8.0;
const WINNER_CHASE_TAIL: u8 = 6;

const NET_CONNECTING_PERIOD_MS: u32 = 2000;
const NET_ERROR_PERIOD_MS: u32 = 2000;

pub fn flag_effect(flag: TrackFlag) -> Effect {
    match flag {
        TrackFlag::Green => Effect::Solid(Rgb::GREEN),
        TrackFlag::Yellow => Effect::Solid(Rgb::YELLOW),
        TrackFlag::DoubleYellow => Effect::Blink {
            color: Rgb::YELLOW,
            period_ms: DOUBLE_YELLOW_PERIOD_MS,
            times: None,
        },
        TrackFlag::SafetyCar => Effect::Chase {
            color: Rgb::YELLOW,
            leds_per_sec: SC_CHASE_SPEED,
            tail: SC_CHASE_TAIL,
        },
        TrackFlag::VirtualSafetyCar => Effect::Breathe {
            color: Rgb::YELLOW,
            period_ms: VSC_PERIOD_MS,
        },
        TrackFlag::Red => Effect::Solid(Rgb::RED),
    }
}

pub fn event_overlay(event: RaceEvent) -> Option<Effect> {
    match event {
        RaceEvent::TrackFlag(_) | RaceEvent::FlagCleared => None,
        RaceEvent::StartLights => Some(Effect::StartLights {
            hold_ms: START_HOLD_MS,
        }),
        RaceEvent::ChequeredFlag => Some(Effect::Chequered {
            duration_ms: CHEQUERED_MS,
        }),
        RaceEvent::FastestLap { .. } => Some(Effect::flash(PURPLE, FASTEST_LAP_FLASHES)),
        RaceEvent::PitStop { team_color, .. } => Some(Effect::flash(team_color, PIT_STOP_FLASHES)),
        RaceEvent::Overtake { team_color, .. } => Some(Effect::flash(team_color, OVERTAKE_FLASHES)),
        RaceEvent::Winner { .. } => None,
    }
}

pub fn winner_effect(team_color: Rgb) -> Effect {
    Effect::Chase {
        color: team_color,
        leds_per_sec: WINNER_CHASE_SPEED,
        tail: WINNER_CHASE_TAIL,
    }
}

pub fn net_effect(status: NetStatus) -> Option<Effect> {
    match status {
        NetStatus::Connecting => Some(Effect::Breathe {
            color: NET_BLUE,
            period_ms: NET_CONNECTING_PERIOD_MS,
        }),
        NetStatus::Online => None,
        NetStatus::ApiError => Some(Effect::Blink {
            color: Rgb::RED,
            period_ms: NET_ERROR_PERIOD_MS,
            times: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: Rgb = Rgb::new(0, 210, 190);

    fn all_flags() -> [TrackFlag; 6] {
        [
            TrackFlag::Green,
            TrackFlag::Yellow,
            TrackFlag::DoubleYellow,
            TrackFlag::SafetyCar,
            TrackFlag::VirtualSafetyCar,
            TrackFlag::Red,
        ]
    }

    fn all_events() -> [RaceEvent; 7] {
        [
            RaceEvent::TrackFlag(TrackFlag::Green),
            RaceEvent::StartLights,
            RaceEvent::ChequeredFlag,
            RaceEvent::FastestLap {
                driver: 44,
                team_color: TEAM,
            },
            RaceEvent::PitStop {
                driver: 44,
                team_color: TEAM,
            },
            RaceEvent::Overtake {
                driver: 44,
                team_color: TEAM,
            },
            RaceEvent::Winner {
                driver: 44,
                team_color: TEAM,
            },
        ]
    }

    #[test]
    fn safety_car_and_vsc_look_different() {
        assert_ne!(
            flag_effect(TrackFlag::SafetyCar),
            flag_effect(TrackFlag::VirtualSafetyCar)
        );
    }

    #[test]
    fn every_flag_has_a_distinct_effect() {
        let flags = all_flags();
        for (i, &a) in flags.iter().enumerate() {
            for &b in &flags[i + 1..] {
                assert_ne!(flag_effect(a), flag_effect(b), "{a:?} looks like {b:?}");
            }
        }
    }

    #[test]
    fn flag_effects_last_until_replaced() {
        for flag in all_flags() {
            assert_eq!(
                flag_effect(flag).duration(),
                None,
                "{flag:?} ends on its own"
            );
        }
    }

    #[test]
    fn every_event_overlay_ends() {
        for event in all_events() {
            if let Some(effect) = event_overlay(event) {
                assert!(effect.duration().is_some(), "{event:?} overlay never ends");
            }
        }
    }

    #[test]
    fn track_flag_event_has_no_overlay() {
        assert_eq!(event_overlay(RaceEvent::TrackFlag(TrackFlag::Red)), None);
    }

    #[test]
    fn fastest_lap_flashes_purple() {
        let event = RaceEvent::FastestLap {
            driver: 44,
            team_color: TEAM,
        };
        assert_eq!(
            event_overlay(event),
            Some(Effect::flash(PURPLE, FASTEST_LAP_FLASHES))
        );
    }

    #[test]
    fn pit_stop_flash_uses_team_colour() {
        let event = RaceEvent::PitStop {
            driver: 44,
            team_color: TEAM,
        };
        assert_eq!(
            event_overlay(event),
            Some(Effect::flash(TEAM, PIT_STOP_FLASHES))
        );
    }

    #[test]
    fn pit_stop_and_overtake_look_different() {
        let pit = RaceEvent::PitStop {
            driver: 44,
            team_color: TEAM,
        };
        let overtake = RaceEvent::Overtake {
            driver: 44,
            team_color: TEAM,
        };
        assert_ne!(event_overlay(pit), event_overlay(overtake));
    }

    #[test]
    fn winner_is_a_team_colour_chase_that_runs_until_stopped() {
        let effect = winner_effect(TEAM);
        assert!(matches!(effect, Effect::Chase { color, .. } if color == TEAM));
        assert_eq!(effect.duration(), None);
    }

    #[test]
    fn online_has_no_status_effect() {
        assert_eq!(net_effect(NetStatus::Online), None);
        assert!(net_effect(NetStatus::Connecting).is_some());
        assert!(net_effect(NetStatus::ApiError).is_some());
    }

    #[test]
    fn connecting_does_not_look_like_vsc() {
        assert_ne!(
            net_effect(NetStatus::Connecting),
            Some(flag_effect(TrackFlag::VirtualSafetyCar))
        );
    }
}
