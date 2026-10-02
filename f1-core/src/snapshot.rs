use serde::Serialize;

use crate::{
    input::{NetStatus, SessionPhase},
    settings::Settings,
};

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Snapshot {
    pub layer: &'static str,
    pub phase: SessionPhase,
    pub net: NetStatus,
    pub override_active: bool,
    pub settings: Settings,
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
            ["layer", "net", "override_active", "phase", "settings"]
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
}
