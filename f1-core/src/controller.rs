use std::time::{Duration, Instant};

use crate::{
    effect::Effect,
    frame::Frame,
    input::{Input, NetStatus, RaceEvent, SessionPhase, TrackFlag},
    overlays::Overlays,
    post,
    settings::Settings,
    theme::{event_overlay, flag_effect, net_effect, winner_effect},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    pub effect: Effect,
    pub started: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Stamped<T> {
    value: T,
    since: Instant,
}

impl<T: PartialEq> Stamped<T> {
    fn new(value: T, now: Instant) -> Self {
        Self { value, since: now }
    }

    fn update(&mut self, value: T, now: Instant) {
        if self.value != value {
            *self = Self::new(value, now);
        }
    }
}

fn stamp<T: PartialEq>(slot: &mut Option<Stamped<T>>, value: T, now: Instant) {
    match slot {
        Some(stamped) => stamped.update(value, now),
        None => *slot = Some(Stamped::new(value, now)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WinnerScene {
    scene: Scene,
    until: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LayerState {
    override_effect: Option<Stamped<Effect>>,
    flag: Option<Stamped<TrackFlag>>,
    winner: Option<WinnerScene>,
    net: Stamped<NetStatus>,
    phase: SessionPhase,
    ever_online: bool,
    settings: Settings,
    /// Local time in minutes after midnight; `None` until the first clock message.
    minute_of_day: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct Controller {
    booted: Instant,
    layers: LayerState,
    overlays: Overlays,
}

type Layer = fn(&Controller) -> Option<Scene>;

const LAYERS: [(&str, Layer); 5] = [
    ("override", Controller::override_layer),
    ("overlay", Controller::overlay_layer),
    ("live track", Controller::live_track_layer),
    ("winner", Controller::winner_layer),
    ("status", Controller::status_layer),
];

impl Controller {
    pub fn new(now: Instant) -> Self {
        Self {
            booted: now,
            layers: LayerState {
                override_effect: None,
                flag: None,
                winner: None,
                net: Stamped::new(NetStatus::Connecting, now),
                phase: SessionPhase::Idle,
                ever_online: false,
                settings: Settings::default(),
                minute_of_day: None,
            },
            overlays: Overlays::default(),
        }
    }

    pub fn apply(&mut self, input: Input, now: Instant) {
        match input {
            Input::Race { event, .. } => self.apply_race_event(event, now),
            Input::Phase(phase) => self.layers.phase = phase,
            Input::Net(status) => {
                self.layers.net.update(status, now);
                self.layers.ever_online |= status == NetStatus::Online;
            }
            Input::Override(Some(effect)) => stamp(&mut self.layers.override_effect, effect, now),
            Input::Override(None) => self.layers.override_effect = None,
            Input::Settings(settings) => self.layers.settings = settings,
            Input::Clock { minute_of_day } => self.layers.minute_of_day = Some(minute_of_day),
        }
    }

    pub fn tick(&mut self, now: Instant) {
        self.overlays.tick(now);
        if self.layers.winner.is_some_and(|w| now >= w.until) {
            self.layers.winner = None;
        }
    }

    pub fn arbitrate(&self) -> (&'static str, Scene) {
        LAYERS
            .iter()
            .find_map(|&(name, layer)| layer(self).map(|scene| (name, scene)))
            .unwrap_or(("lamp", self.lamp_scene()))
    }

    pub fn render(&mut self, now: Instant, frame: &mut Frame) -> &'static str {
        self.tick(now);
        let (layer, scene) = self.arbitrate();
        scene
            .effect
            .render(now.duration_since(scene.started), frame);

        let layers = &self.layers;
        let brightness = post::brightness(&layers.settings, layers.minute_of_day, layers.phase);
        post::post_process(frame, brightness);
        layer
    }

    fn apply_race_event(&mut self, event: RaceEvent, now: Instant) {
        match event {
            RaceEvent::TrackFlag(flag) => stamp(&mut self.layers.flag, flag, now),
            RaceEvent::Winner { team_color, .. } => {
                self.layers.winner = Some(WinnerScene {
                    scene: Scene {
                        effect: winner_effect(team_color),
                        started: now,
                    },
                    until: now
                        + Duration::from_millis(u64::from(self.layers.settings.winner_display_ms)),
                });
            }
            other => {
                if let Some(effect) = event_overlay(other) {
                    self.overlays.push(effect);
                }
            }
        }
    }

    fn override_layer(&self) -> Option<Scene> {
        self.layers.override_effect.map(|o| Scene {
            effect: o.value,
            started: o.since,
        })
    }

    fn overlay_layer(&self) -> Option<Scene> {
        self.overlays
            .active()
            .map(|(effect, started)| Scene { effect, started })
    }

    fn live_track_layer(&self) -> Option<Scene> {
        let layers = &self.layers;
        if layers.phase != SessionPhase::Live || layers.net.value != NetStatus::Online {
            return None;
        }
        layers.flag.map(|f| Scene {
            effect: flag_effect(f.value),
            started: f.since,
        })
    }

    fn winner_layer(&self) -> Option<Scene> {
        self.layers.winner.map(|w| w.scene)
    }

    fn status_layer(&self) -> Option<Scene> {
        let layers = &self.layers;
        let session_needs_network =
            matches!(layers.phase, SessionPhase::PreSession | SessionPhase::Live);
        if layers.ever_online && !session_needs_network {
            return None;
        }
        net_effect(layers.net.value).map(|effect| Scene {
            effect,
            started: layers.net.since,
        })
    }

    fn lamp_scene(&self) -> Scene {
        let settings = &self.layers.settings;
        Scene {
            effect: Effect::Solid(settings.lamp_color.scale(settings.lamp_brightness)),
            started: self.booted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::Rgb,
        theme::{LAMP, PURPLE},
    };

    const TEAM: Rgb = Rgb::new(0, 210, 190);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn default_lamp() -> Effect {
        let s = Settings::default();
        Effect::Solid(s.lamp_color.scale(s.lamp_brightness))
    }

    fn default_winner_display() -> Duration {
        Duration::from_millis(u64::from(Settings::default().winner_display_ms))
    }

    fn race(event: RaceEvent, at: Instant) -> Input {
        Input::Race {
            event,
            received: at,
        }
    }

    fn flag(flag: TrackFlag, at: Instant) -> Input {
        race(RaceEvent::TrackFlag(flag), at)
    }

    fn fastest_lap(at: Instant) -> Input {
        race(
            RaceEvent::FastestLap {
                driver: 44,
                team_color: TEAM,
            },
            at,
        )
    }

    fn winner(at: Instant) -> Input {
        race(
            RaceEvent::Winner {
                driver: 44,
                team_color: TEAM,
            },
            at,
        )
    }

    /// A controller that is online and in a live session.
    fn live(t0: Instant) -> Controller {
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        c.apply(Input::Phase(SessionPhase::Live), t0);
        c
    }

    /// Ticks, then returns the winning layer and its effect.
    fn winner_at(c: &mut Controller, now: Instant) -> (&'static str, Effect) {
        c.tick(now);
        let (layer, scene) = c.arbitrate();
        (layer, scene.effect)
    }

    #[test]
    fn boot_shows_connecting_status() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        let expected = net_effect(NetStatus::Connecting).unwrap();
        assert_eq!(winner_at(&mut c, t0), ("status", expected));
    }

    #[test]
    fn online_and_idle_shows_lamp() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        assert_eq!(winner_at(&mut c, t0), ("lamp", default_lamp()));
    }

    #[test]
    fn flag_is_ignored_until_session_is_live() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        c.apply(flag(TrackFlag::Red, t0), t0);
        assert_eq!(winner_at(&mut c, t0).0, "lamp");

        c.apply(Input::Phase(SessionPhase::Live), t0);
        assert_eq!(
            winner_at(&mut c, t0),
            ("live track", flag_effect(TrackFlag::Red))
        );
    }

    #[test]
    fn live_track_yields_to_status_when_network_drops() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Red, t0), t0);
        c.apply(Input::Net(NetStatus::ApiError), t0 + ms(10));
        let expected = net_effect(NetStatus::ApiError).unwrap();
        assert_eq!(winner_at(&mut c, t0 + ms(10)), ("status", expected));
    }

    #[test]
    fn status_shows_before_a_session_when_offline() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        c.apply(Input::Phase(SessionPhase::PreSession), t0);
        c.apply(Input::Net(NetStatus::Connecting), t0);
        assert_eq!(winner_at(&mut c, t0).0, "status");
    }

    #[test]
    fn status_stays_hidden_when_no_session_needs_network() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        c.apply(Input::Net(NetStatus::ApiError), t0);
        assert_eq!(winner_at(&mut c, t0).0, "lamp");
    }

    #[test]
    fn overlay_beats_track_flag_then_falls_back() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        c.apply(fastest_lap(t0), t0);

        // Fastest lap: 3 flashes × 300 ms = 900 ms.
        assert_eq!(winner_at(&mut c, t0).0, "overlay");
        assert_eq!(winner_at(&mut c, t0 + ms(899)).0, "overlay");
        assert_eq!(winner_at(&mut c, t0 + ms(900)).0, "live track");
    }

    #[test]
    fn override_beats_everything() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Red, t0), t0);
        c.apply(fastest_lap(t0), t0);
        c.apply(winner(t0), t0);
        c.apply(Input::Override(Some(Effect::Solid(Rgb::WHITE))), t0);
        assert_eq!(
            winner_at(&mut c, t0),
            ("override", Effect::Solid(Rgb::WHITE))
        );
    }

    #[test]
    fn clearing_override_falls_back() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Red, t0), t0);
        c.apply(Input::Override(Some(Effect::Solid(Rgb::WHITE))), t0);
        c.apply(Input::Override(None), t0);
        assert_eq!(winner_at(&mut c, t0).0, "live track");
    }

    #[test]
    fn winner_shows_until_display_time_then_lamp() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        c.apply(Input::Phase(SessionPhase::PostSession), t0);
        c.apply(winner(t0), t0);

        assert_eq!(winner_at(&mut c, t0), ("winner", winner_effect(TEAM)));
        let display = default_winner_display();
        assert_eq!(winner_at(&mut c, t0 + display - ms(1)).0, "winner");
        assert_eq!(winner_at(&mut c, t0 + display).0, "lamp");
    }

    #[test]
    fn repeated_flag_keeps_its_start_time() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::DoubleYellow, t0), t0);
        c.apply(flag(TrackFlag::DoubleYellow, t0 + ms(100)), t0 + ms(100));
        assert_eq!(c.arbitrate().1.started, t0);
    }

    #[test]
    fn changing_flag_restarts_its_clock() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        c.apply(flag(TrackFlag::Yellow, t0 + ms(400)), t0 + ms(400));
        assert_eq!(c.arbitrate().1.started, t0 + ms(400));
    }

    #[test]
    fn render_draws_the_winning_layer_and_names_it() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(fastest_lap(t0), t0);
        let mut frame = Frame::new();
        let layer = c.render(t0, &mut frame);
        assert_eq!(layer, "overlay");
        let expected = post::correct(PURPLE, Settings::default().global_brightness);
        assert!(frame.pixels().iter().all(|&p| p == expected));
    }

    #[test]
    fn lamp_uses_colour_and_brightness_from_settings() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        let settings = Settings {
            lamp_color: Rgb::BLUE,
            lamp_brightness: 0.5,
            ..Settings::default()
        };
        c.apply(Input::Settings(settings), t0);
        assert_eq!(
            winner_at(&mut c, t0),
            ("lamp", Effect::Solid(Rgb::BLUE.scale(0.5)))
        );
    }

    #[test]
    fn winner_display_time_comes_from_settings() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        let settings = Settings {
            winner_display_ms: 5000,
            ..Settings::default()
        };
        c.apply(Input::Settings(settings), t0);
        c.apply(winner(t0), t0);
        assert_eq!(winner_at(&mut c, t0 + ms(4999)).0, "winner");
        assert_eq!(winner_at(&mut c, t0 + ms(5000)).0, "lamp");
    }

    #[test]
    fn new_settings_only_affect_the_next_winner() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        c.apply(winner(t0), t0);
        let shorter = Settings {
            winner_display_ms: 1000,
            ..Settings::default()
        };
        c.apply(Input::Settings(shorter), t0 + ms(10));
        assert_eq!(winner_at(&mut c, t0 + ms(2000)).0, "winner");
    }

    fn night_settings() -> Settings {
        Settings {
            night_start: Some(22 * 60),
            night_end: Some(7 * 60),
            global_brightness: 0.5,
            night_brightness: 0.1,
            ..Settings::default()
        }
    }

    fn rendered_pixel(c: &mut Controller, now: Instant) -> Rgb {
        let mut frame = Frame::new();
        c.render(now, &mut frame);
        frame.pixels()[0]
    }

    #[test]
    fn lamp_dims_when_clock_enters_night_window() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        c.apply(Input::Settings(night_settings()), t0);

        c.apply(
            Input::Clock {
                minute_of_day: 12 * 60,
            },
            t0,
        );
        let day = rendered_pixel(&mut c, t0);
        c.apply(
            Input::Clock {
                minute_of_day: 23 * 60,
            },
            t0,
        );
        let night = rendered_pixel(&mut c, t0);

        assert!(night.r < day.r, "day {day:?}, night {night:?}");
    }

    #[test]
    fn live_race_is_not_dimmed_at_night() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(Input::Settings(night_settings()), t0);
        c.apply(flag(TrackFlag::Red, t0), t0);

        c.apply(
            Input::Clock {
                minute_of_day: 12 * 60,
            },
            t0,
        );
        let day = rendered_pixel(&mut c, t0);
        c.apply(
            Input::Clock {
                minute_of_day: 23 * 60,
            },
            t0,
        );
        let night = rendered_pixel(&mut c, t0);

        assert_eq!(night, day);
    }

    #[test]
    fn it_is_never_night_before_the_first_clock_message() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        c.apply(Input::Settings(night_settings()), t0);
        let lamp = rendered_pixel(&mut c, t0);
        let expected = post::correct(LAMP.scale(Settings::default().lamp_brightness), 0.5);
        assert_eq!(lamp, expected);
    }
}
