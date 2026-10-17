//! [`Snapshot`]: what the web page shows about the lamp (`GET /api/state`).

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{
    color::Rgb,
    effect::Motion,
    input::{NetStatus, SessionPhase},
    overlays::Recent,
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
    /// How what shows moves, for the panel.
    pub motion: Motion,
    /// When it started; sent as `age_ms` (see [`StateReply`]).
    #[serde(skip)]
    pub shown_since: Instant,
    /// The number of the overlay showing (see [`Recent`]), if one is.
    pub flash: Option<u32>,
    /// Sent as `flashes`: those that ended lately (see [`StateReply`]).
    #[serde(skip)]
    pub recent: Recent,
    pub phase: SessionPhase,
    pub net: NetStatus,
    pub override_active: bool,
    pub settings: Settings,
    /// Sent as [`TvState`], worked out when asked (see [`StateReply`]).
    #[serde(skip)]
    pub tv: TvView,
}

/// An overlay that ended up to this long ago is sent, so the page can play
/// the ones it didn't see: the panel asks only every 2 s.
pub const FLASHES_KEPT: Duration = Duration::from_secs(4);

/// What `GET /api/state` answers: the snapshot, how long ago what shows
/// started, the overlays that ended lately, and the TV's clock, lap and lap
/// times, all at the moment of answering.
#[derive(Debug, Serialize)]
pub struct StateReply {
    #[serde(flatten)]
    pub snapshot: Snapshot,
    pub age_ms: u32,
    pub flashes: Vec<Flash>,
    pub tv: TvState,
}

/// One overlay that showed and ended, for the page to play late.
#[derive(Debug, PartialEq, Serialize)]
pub struct Flash {
    pub seq: u32,
    pub motion: Motion,
    /// `None` for the chequered flag, a pattern.
    pub color: Option<Rgb>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct TvState {
    /// The session clock's time left, as the TV shows it now.
    pub clock_ms: Option<u32>,
    pub clock_running: bool,
    pub lap: Option<u32>,
    pub total_laps: Option<u32>,
    /// The last lap time on the TV and those due soon (see [`TvView`]).
    pub lap_times: Vec<LapTimeState>,
}

/// One lap time, on the TV `in_ms` from now (on already if not above 0).
#[derive(Debug, PartialEq, Serialize)]
pub struct LapTimeState {
    pub car: u8,
    /// The short name, `"VER"`, if known; see [`TvState::name_cars`].
    pub name: Option<String>,
    pub ms: u32,
    pub in_ms: i64,
}

impl TvState {
    /// Fills in the cars' short names, which the device keeps elsewhere.
    pub fn name_cars(&mut self, name: impl Fn(u8) -> Option<String>) {
        for lap_time in &mut self.lap_times {
            lap_time.name = name(lap_time.car);
        }
    }
}

impl StateReply {
    pub fn new(snapshot: Snapshot, now: Instant) -> Self {
        let tv = snapshot.tv;
        let age = now.saturating_duration_since(snapshot.shown_since);
        StateReply {
            snapshot,
            age_ms: u32::try_from(age.as_millis()).unwrap_or(u32::MAX),
            flashes: snapshot
                .recent
                .iter()
                .filter(|p| now.saturating_duration_since(p.ended) <= FLASHES_KEPT)
                .map(|p| Flash {
                    seq: p.seq,
                    motion: p.effect.motion(),
                    color: p.effect.main_color(),
                })
                .collect(),
            tv: TvState {
                clock_ms: tv.clock.map(|c| c.remaining_at(now)),
                clock_running: tv.clock.is_some_and(|c| c.running),
                lap: tv.lap,
                total_laps: tv.total_laps,
                lap_times: tv
                    .lap_times
                    .iter()
                    .flatten()
                    .map(|t| LapTimeState {
                        car: t.car,
                        name: None,
                        ms: t.ms,
                        in_ms: signed_ms(t.on_tv, now),
                    })
                    .collect(),
            },
        }
    }
}

/// `at - now` in milliseconds, below 0 if `at` has passed.
fn signed_ms(at: Instant, now: Instant) -> i64 {
    let ms = |d: Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
    match at.checked_duration_since(now) {
        Some(ahead) => ms(ahead),
        None => -ms(now.duration_since(at)),
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
                "flash",
                "layer",
                "motion",
                "net",
                "override_active",
                "phase",
                "settings"
            ]
        );
    }

    #[test]
    fn the_lamp_colour_is_still_on_the_panel() {
        let json = serde_json::to_value(snapshot_after(&[Input::Net(NetStatus::Online)])).unwrap();
        assert_eq!(json["motion"], serde_json::json!({"kind": "solid"}));
    }

    #[test]
    fn the_reply_says_how_long_ago_what_shows_started() {
        let t0 = Instant::now();
        let snap = Controller::new(t0).snapshot();
        let reply = StateReply::new(snap, t0 + std::time::Duration::from_millis(1500));
        assert_eq!(reply.age_ms, 1500);
        // Breathing blue while connecting, from boot.
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["age_ms"], 1500);
        assert_eq!(json["motion"]["kind"], "breathe");
    }

    const TEAM: Rgb = Rgb::new(0, 210, 190);

    fn pit_stop() -> Input {
        Input::Preview(crate::input::RaceEvent::PitStop {
            driver: 44,
            team_color: TEAM,
        })
    }

    #[test]
    fn a_flash_that_ended_is_sent_for_a_few_seconds() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(pit_stop(), t0);
        c.tick(t0);
        assert_eq!(c.snapshot().flash, Some(0), "showing now");

        // Two flashes of 300 ms: over by 600 ms, before the page asked.
        c.tick(t0 + Duration::from_millis(700));
        let snap = c.snapshot();
        assert_eq!(snap.flash, None);
        let reply = StateReply::new(snap, t0 + Duration::from_secs(2));
        assert_eq!(
            reply.flashes,
            [Flash {
                seq: 0,
                motion: Effect::flash(TEAM, 2).motion(),
                color: Some(TEAM),
            }]
        );
        let later = t0 + Duration::from_millis(600) + FLASHES_KEPT + Duration::from_millis(1);
        assert!(StateReply::new(snap, later).flashes.is_empty());
    }

    #[test]
    fn a_flash_hidden_by_a_forced_colour_is_not_sent() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Override(Some(Effect::Solid(Rgb::RED))), t0);
        c.apply(pit_stop(), t0);
        c.tick(t0);
        c.tick(t0 + Duration::from_millis(700));
        let reply = StateReply::new(c.snapshot(), t0 + Duration::from_secs(1));
        assert!(reply.flashes.is_empty());
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
            lap_times: [None; crate::tv_timing::LAP_TIMES_SENT],
        };
        let reply = StateReply::new(snap, t0 + std::time::Duration::from_secs(5));
        let json = serde_json::to_string(&reply).unwrap();
        assert!(
            json.contains(
                r#""tv":{"clock_ms":595000,"clock_running":true,"lap":3,"total_laps":62,"lap_times":[]}"#
            ),
            "{json}"
        );
        assert!(json.contains(r#""layer":"#), "{json}");
    }

    #[test]
    fn the_reply_says_when_each_lap_time_is_on_the_tv() {
        use crate::tv_timing::TvLapTime;
        let t0 = Instant::now();
        let mut snap = snapshot_after(&[]);
        let lap_time = |car, on_tv| {
            Some(TvLapTime {
                car,
                ms: 86_651,
                on_tv,
            })
        };
        snap.tv.lap_times[0] = lap_time(63, t0);
        snap.tv.lap_times[1] = lap_time(1, t0 + Duration::from_millis(2500));
        let mut reply = StateReply::new(snap, t0 + Duration::from_millis(500));
        reply
            .tv
            .name_cars(|car| (car == 63).then(|| "RUS".to_string()));
        let lap_times = serde_json::to_string(&reply.tv.lap_times).unwrap();
        assert_eq!(
            lap_times,
            r#"[{"car":63,"name":"RUS","ms":86651,"in_ms":-500},{"car":1,"name":null,"ms":86651,"in_ms":2000}]"#
        );
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
