//! [`Snapshot`]: what the web page shows about the lamp (`GET /api/state`).

use std::time::Instant;

use serde::Serialize;

use crate::{
    color::Rgb,
    input::{NetStatus, SessionPhase},
    settings::Settings,
    tv_timing::TvView,
};

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Snapshot {
    pub layer: &'static str,
    /// The colour the lamp shows (`"#ffd100"`), for the page's panel; `null`
    /// for the chequered flag, a pattern. The lamp's own colour unscaled, so
    /// a dim lamp still lights the panel.
    pub color: Option<Rgb>,
    pub phase: SessionPhase,
    pub net: NetStatus,
    pub override_active: bool,
    pub settings: Settings,
    /// Sent as [`TvState`], worked out when asked (see [`StateReply`]).
    #[serde(skip)]
    pub tv: TvView,
}

/// What `GET /api/state` answers: the snapshot, and the TV's clock and lap
/// at the moment of answering.
#[derive(Debug, Serialize)]
pub struct StateReply {
    #[serde(flatten)]
    pub snapshot: Snapshot,
    pub tv: TvState,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct TvState {
    /// The session clock's time left, as the TV shows it now.
    pub clock_ms: Option<u32>,
    pub clock_running: bool,
    pub lap: Option<u32>,
    pub total_laps: Option<u32>,
}

impl StateReply {
    pub fn new(snapshot: Snapshot, now: Instant) -> Self {
        let tv = snapshot.tv;
        StateReply {
            snapshot,
            tv: TvState {
                clock_ms: tv.clock.map(|c| c.remaining_at(now)),
                clock_running: tv.clock.is_some_and(|c| c.running),
                lap: tv.lap,
                total_laps: tv.total_laps,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::{color::Rgb, controller::Controller, effect::Effect, input::Input};

    fn snapshot_after(inputs: &[Input]) -> Snapshot {
        let now = Instant::now();
        let mut c = Controller::new(now);
        for &input in inputs {
            c.apply(input, now);
        }
        c.snapshot()
    }

    #[test]
    fn snapshot_at_boot_shows_connecting_status() {
        let snap = snapshot_after(&[]);
        assert_eq!(snap.layer, "status");
        assert_eq!(snap.phase, SessionPhase::Idle);
        assert_eq!(snap.net, NetStatus::Connecting);
        assert!(!snap.override_active);
        assert_eq!(snap.settings, Settings::default());
    }

    #[test]
    fn snapshot_reports_override_while_active() {
        let on = Input::Override(Some(Effect::Solid(Rgb::RED)));
        let snap = snapshot_after(&[on]);
        assert!(snap.override_active);
        assert_eq!(snap.layer, "override");

        let snap = snapshot_after(&[on, Input::Override(None)]);
        assert!(!snap.override_active);
    }

    #[test]
    fn snapshot_reports_phase_and_net_status() {
        let snap = snapshot_after(&[
            Input::Net(NetStatus::Online),
            Input::Phase(SessionPhase::PreSession),
        ]);
        assert_eq!(snap.net, NetStatus::Online);
        assert_eq!(snap.phase, SessionPhase::PreSession);
    }

    #[test]
    fn snapshot_shows_lamp_when_online_and_idle() {
        let snap = snapshot_after(&[Input::Net(NetStatus::Online)]);
        assert_eq!(snap.layer, "lamp");
    }

    #[test]
    fn snapshot_carries_applied_settings() {
        let settings = Settings {
            lamp_color: Rgb::new(1, 2, 3),
            tv_delay_ms: 5000,
            ..Settings::default()
        };
        let snap = snapshot_after(&[Input::Settings(settings)]);
        assert_eq!(snap.settings, settings);
    }

    #[test]
    fn snapshot_serialises_only_expected_fields() {
        let json = serde_json::to_value(snapshot_after(&[])).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "color",
                "layer",
                "net",
                "override_active",
                "phase",
                "settings"
            ]
        );
    }

    #[test]
    fn snapshot_serialises_enums_in_snake_case() {
        let snap = snapshot_after(&[
            Input::Net(NetStatus::ApiError),
            Input::Phase(SessionPhase::PreSession),
        ]);
        let json = serde_json::to_value(snap).unwrap();
        assert_eq!(json["net"], "api_error");
        assert_eq!(json["phase"], "pre_session");
    }

    #[test]
    fn snapshot_serialises_lamp_colour_as_hex() {
        let json = serde_json::to_value(snapshot_after(&[])).unwrap();
        assert_eq!(json["settings"]["lamp_color"], "#ffa03c");
    }

    #[test]
    fn the_reply_has_the_tv_clock_counted_down_to_now() {
        let t0 = Instant::now();
        let mut snap = snapshot_after(&[]);
        snap.tv = crate::tv_timing::TvView {
            clock: Some(crate::tv_timing::TvClock {
                anchor: t0,
                remaining_ms: 600_000,
                running: true,
            }),
            lap: Some(3),
            total_laps: Some(62),
        };
        let reply = StateReply::new(snap, t0 + std::time::Duration::from_secs(5));
        let json = serde_json::to_string(&reply).unwrap();
        assert!(
            json.contains(
                r#""tv":{"clock_ms":595000,"clock_running":true,"lap":3,"total_laps":62}"#
            ),
            "{json}"
        );
        assert!(json.contains(r#""layer":"#), "{json}");
    }

    #[test]
    fn the_snapshot_has_the_colour_shown() {
        use crate::input::{RaceEvent, TrackFlag};
        let online = Input::Net(crate::input::NetStatus::Online);
        let now = Instant::now();
        let yellow = Input::Race {
            event: RaceEvent::TrackFlag(TrackFlag::Yellow),
            received: now,
        };
        let live = Input::Phase(SessionPhase::Live);
        assert_eq!(
            snapshot_after(&[online, live, yellow]).color,
            Some(Rgb::YELLOW)
        );
        // An overlay starts on the next tick.
        let mut c = Controller::new(now);
        c.apply(online, now);
        c.apply(Input::Preview(RaceEvent::ChequeredFlag), now);
        c.tick(now);
        assert_eq!(c.snapshot().layer, "overlay");
        assert_eq!(c.snapshot().color, None);
    }

    #[test]
    fn the_lamp_colour_is_sent_at_full_strength() {
        let online = Input::Net(crate::input::NetStatus::Online);
        let settings = Settings {
            lamp_color: Rgb::new(0, 200, 100),
            lamp_brightness: 0.1,
            ..Settings::default()
        };
        let snap = snapshot_after(&[online, Input::Settings(settings)]);
        assert_eq!(snap.layer, "lamp");
        assert_eq!(snap.color, Some(Rgb::new(0, 200, 100)));
    }
}
