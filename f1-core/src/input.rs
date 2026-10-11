//! What the threads send the render loop ([`Input`]), and the race events and
//! statuses in it.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{color::Rgb, effect::Effect, settings::Settings};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackFlag {
    Green,
    Yellow,
    DoubleYellow,
    SafetyCar,
    VirtualSafetyCar,
    /// The VSC ends in about 15 s (track status 7, `VSCEnding`).
    VscEnding,
    Red,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaceEvent {
    TrackFlag(TrackFlag),
    /// No flag any more: the session (or a qualifying part) has finished,
    /// so the last flag must not stay on the lamp.
    FlagCleared,
    StartLights,
    ChequeredFlag,
    FastestLap {
        driver: u8,
        team_color: Rgb,
    },
    PitStop {
        driver: u8,
        team_color: Rgb,
    },
    Overtake {
        driver: u8,
        team_color: Rgb,
    },
    /// The race's winner, or pole at the end of qualifying.
    Winner {
        driver: u8,
        team_color: Rgb,
    },
    /// The leader has started a race's last lap.
    LastLap,
    /// Out of qualifying at the end of Q1 or Q2.
    KnockedOut {
        driver: u8,
        team_color: Rgb,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Idle,
    PreSession,
    Live,
    PostSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetStatus {
    Connecting,
    Online,
    ApiError,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Race {
        event: RaceEvent,
        received: Instant,
    },
    Phase(SessionPhase),
    Net(NetStatus),
    /// From the live thread: the feed keeps failing (`true`) or works again.
    /// Kept apart from `Net`, which is WiFi's: two senders of one status
    /// would undo each other. The controller shows it as `ApiError`.
    FeedFailing(bool),
    Override(Option<Effect>),
    Settings(Settings),
    Clock {
        minute_of_day: u16,
    },
}
