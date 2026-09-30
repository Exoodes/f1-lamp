use std::time::Instant;

use crate::{color::Rgb, effect::Effect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackFlag {
    Green,
    Yellow,
    DoubleYellow,
    SafetyCar,
    VirtualSafetyCar,
    Red,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaceEvent {
    TrackFlag(TrackFlag),
    StartLights,
    ChequeredFlag,
    FastestLap { driver: u8, team_color: Rgb },
    PitStop { driver: u8, team_color: Rgb },
    Overtake { driver: u8, team_color: Rgb },
    Winner { driver: u8, team_color: Rgb },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    Idle,
    PreSession,
    Live,
    PostSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetStatus {
    Connecting,
    Online,
    ApiError,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Race { event: RaceEvent, received: Instant },
    Phase(SessionPhase),
    Net(NetStatus),
    Override(Option<Effect>),
}
