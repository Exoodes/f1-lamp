use crate::{color::Rgb, effect::Effect, input::TrackFlag};

/// Shown when no flag has been received yet.
pub const LAMP: Rgb = Rgb::new(255, 160, 60);

pub fn flag_effect(flag: TrackFlag) -> Effect {
    match flag {
        TrackFlag::Green => Effect::Solid(Rgb::GREEN),
        TrackFlag::Yellow => Effect::Blink {
            color: Rgb::YELLOW,
            period_ms: 1000,
            times: None,
        },
        TrackFlag::DoubleYellow => Effect::Solid(Rgb::YELLOW),
        TrackFlag::SafetyCar => Effect::Blink {
            color: Rgb::RED,
            period_ms: 1000,
            times: None,
        },
        TrackFlag::VirtualSafetyCar => Effect::Blink {
            color: Rgb::RED,
            period_ms: 1000,
            times: None,
        },
        TrackFlag::Red => Effect::Solid(Rgb::RED),
    }
}
