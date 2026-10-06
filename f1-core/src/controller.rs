//! What the lamp shows, decided from the inputs the threads send.
//!
//! The picture comes from layers, highest first: a flag or colour forced from
//! the page, a short overlay (start lights, chequered flag, flashes), the
//! winner, the track flag (only in a live session with the network up), the
//! network status (around a session, or before the first connection), and
//! otherwise the lamp's own colour. Race events and phase changes wait for the
//! TV delay first; settings, overrides, the clock and the network status act
//! at once. [`Controller::render`] draws the winning layer into a frame and
//! applies the brightness.

use std::time::{Duration, Instant};

use crate::{
    delay::DelayQueue,
    effect::Effect,
    frame::Frame,
    input::{Input, NetStatus, RaceEvent, SessionPhase, TrackFlag},
    overlays::Overlays,
    post,
    settings::Settings,
    snapshot::Snapshot,
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

/// What waits for the TV delay: race events, and phase changes, so the live
/// layer can't end before a delayed chequered flag has been shown.
#[derive(Clone, Copy, Debug)]
enum Delayed {
    Race(RaceEvent),
    Phase(SessionPhase),
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
    /// WiFi's status, from the net thread.
    net: Stamped<NetStatus>,
    /// Since when the live feed keeps failing; `None` while it works.
    feed_failing: Option<Instant>,
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
    /// Race events and phase changes until the TV shows them. Settings,
    /// overrides, the clock and the network status act at once.
    delayed: DelayQueue<Delayed>,
}

type Layer = fn(&Controller) -> Option<Scene>;

const LAYERS: [(&str, Layer); 5] = [
    ("override", Controller::override_layer),
    ("overlay", Controller::overlay_layer),
    // A winner only arrives after the finish, when the last flag is stale.
    ("winner", Controller::winner_layer),
    ("live track", Controller::live_track_layer),
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
                feed_failing: None,
                phase: SessionPhase::Idle,
                ever_online: false,
                settings: Settings::default(),
                minute_of_day: None,
            },
            overlays: Overlays::default(),
            delayed: DelayQueue::new(),
        }
    }

    pub fn apply(&mut self, input: Input, now: Instant) {
        match input {
            Input::Race {
                event: RaceEvent::StartLights,
                received,
            } => self.schedule_start_lights(received, now),
            // Measured from arrival, so time spent in channels doesn't add up.
            Input::Race { event, received } => self.delay(Delayed::Race(event), received, now),
            Input::Phase(phase) => self.delay(Delayed::Phase(phase), now, now),
            Input::Net(status) => {
                self.layers.net.update(status, now);
                self.layers.ever_online |= status == NetStatus::Online;
            }
            Input::FeedFailing(true) => {
                self.layers.feed_failing.get_or_insert(now);
            }
            Input::FeedFailing(false) => self.layers.feed_failing = None,
            Input::Override(Some(effect)) => stamp(&mut self.layers.override_effect, effect, now),
            Input::Override(None) => self.layers.override_effect = None,
            Input::Settings(settings) => self.layers.settings = settings,
            Input::Clock { minute_of_day } => self.layers.minute_of_day = Some(minute_of_day),
        }
    }

    pub fn tick(&mut self, now: Instant) {
        self.release_due(now);
        self.expire_green(now);
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

    /// After `green_display_ms`, a green flag gives way to the lamp's own
    /// colour; every other flag stays as long as it lasts. The next flag,
    /// even another green, shows again.
    fn expire_green(&mut self, now: Instant) {
        let show_for = self.layers.settings.green_display_ms;
        if show_for == 0 {
            return;
        }
        let shown_until =
            |f: &Stamped<TrackFlag>| f.since + Duration::from_millis(u64::from(show_for));
        if self
            .layers
            .flag
            .is_some_and(|f| f.value == TrackFlag::Green && now >= shown_until(&f))
        {
            self.layers.flag = None;
        }
    }

    fn tv_delay(&self) -> Duration {
        Duration::from_millis(u64::from(self.layers.settings.tv_delay_ms))
    }

    /// Queues `item` until `received` plus the TV delay. With no delay it
    /// acts at once, still in order behind anything already waiting.
    fn delay(&mut self, item: Delayed, received: Instant, now: Instant) {
        self.delayed.push(item, received + self.tv_delay());
        self.release_due(now);
    }

    /// The feed reports the start at lights out, and the TV shows it one
    /// delay later. The lights are queued to begin early enough to go out
    /// exactly then. With a delay shorter than the sequence that moment has
    /// already passed, so they're skipped rather than shown late.
    ///
    /// Overlays play one after another: an overlay still running at that
    /// moment (unlikely on the grid) would make them late.
    fn schedule_start_lights(&mut self, received: Instant, now: Instant) {
        let Some(lead) =
            event_overlay(RaceEvent::StartLights).and_then(|effect| effect.lights_out_after())
        else {
            return;
        };
        let delay = self.tv_delay();
        if delay >= lead {
            // `checked_sub`: an `Instant` can't go before the clock's start.
            if let Some(begin) = (received + delay).checked_sub(lead) {
                self.delayed
                    .push(Delayed::Race(RaceEvent::StartLights), begin);
            }
        }
        self.release_due(now);
    }

    fn release_due(&mut self, now: Instant) {
        while let Some(item) = self.delayed.pop_ready(now) {
            match item {
                Delayed::Race(event) => self.apply_race_event(event, now),
                Delayed::Phase(phase) => {
                    self.layers.phase = phase;
                    // A flag belongs to the live session it came from; the
                    // next session must not start with it.
                    if phase != SessionPhase::Live {
                        self.layers.flag = None;
                    }
                }
            }
        }
    }

    fn apply_race_event(&mut self, event: RaceEvent, now: Instant) {
        // Switched off, or a driver nobody follows: as if it never arrived.
        if !self.layers.settings.shows(event) {
            return;
        }
        match event {
            RaceEvent::TrackFlag(flag) => stamp(&mut self.layers.flag, flag, now),
            RaceEvent::FlagCleared => self.layers.flag = None,
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
        if layers.phase != SessionPhase::Live || self.net_status().value != NetStatus::Online {
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
        let net = self.net_status();
        net_effect(net.value).map(|effect| Scene {
            effect,
            started: net.since,
        })
    }

    /// WiFi's status, unless WiFi is up and the live feed keeps failing: then
    /// `ApiError`. A dead WiFi explains a dead feed, so it comes first.
    fn net_status(&self) -> Stamped<NetStatus> {
        match self.layers.feed_failing {
            Some(since) if self.layers.net.value == NetStatus::Online => Stamped {
                value: NetStatus::ApiError,
                since,
            },
            _ => self.layers.net,
        }
    }

    fn lamp_scene(&self) -> Scene {
        let settings = &self.layers.settings;
        Scene {
            effect: Effect::Solid(settings.lamp_color.scale(settings.lamp_brightness)),
            started: self.booted,
        }
    }

    /// What the web page shows. Call after `render`, which ticks, so expired
    /// overlays and winners are not reported.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            layer: self.arbitrate().0,
            phase: self.layers.phase,
            net: self.net_status().value,
            override_active: self.layers.override_effect.is_some(),
            settings: self.layers.settings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::Rgb,
        drivers::DriverSet,
        settings::EffectToggles,
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
    fn a_failing_feed_shows_the_error_instead_of_a_stale_flag() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Yellow, t0), t0);
        c.apply(Input::FeedFailing(true), t0 + ms(10));
        let error = net_effect(NetStatus::ApiError).unwrap();
        assert_eq!(winner_at(&mut c, t0 + ms(10)), ("status", error));
        assert_eq!(c.snapshot().net, NetStatus::ApiError);
    }

    #[test]
    fn a_working_feed_again_brings_the_flag_back() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Yellow, t0), t0);
        c.apply(Input::FeedFailing(true), t0 + ms(10));
        c.apply(Input::FeedFailing(false), t0 + ms(20));
        assert_eq!(winner_at(&mut c, t0 + ms(20)).0, "live track");
        assert_eq!(c.snapshot().net, NetStatus::Online);
    }

    #[test]
    fn wifi_down_is_shown_before_a_failing_feed() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(Input::FeedFailing(true), t0);
        c.apply(Input::Net(NetStatus::Connecting), t0 + ms(10));
        let connecting = net_effect(NetStatus::Connecting).unwrap();
        assert_eq!(winner_at(&mut c, t0 + ms(10)), ("status", connecting));
    }

    #[test]
    fn a_failing_feed_between_sessions_leaves_the_lamp_alone() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(Input::Phase(SessionPhase::Idle), t0);
        c.apply(Input::FeedFailing(true), t0 + ms(10));
        assert_eq!(winner_at(&mut c, t0 + ms(10)), ("lamp", default_lamp()));
    }

    #[test]
    fn the_error_blinks_from_when_the_feed_started_failing() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(Input::FeedFailing(true), t0 + ms(100));
        // A repeat doesn't restart the blinking.
        c.apply(Input::FeedFailing(true), t0 + ms(900));
        c.tick(t0 + ms(900));
        assert_eq!(c.arbitrate().1.started, t0 + ms(100));
    }

    #[test]
    fn flag_cleared_gives_the_lamp_back() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::SafetyCar, t0), t0);
        assert_eq!(winner_at(&mut c, t0).0, "live track");
        c.apply(race(RaceEvent::FlagCleared, t0 + ms(10)), t0 + ms(10));
        assert_eq!(winner_at(&mut c, t0 + ms(10)), ("lamp", default_lamp()));
    }

    #[test]
    fn the_next_session_does_not_start_with_the_last_flag() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Red, t0), t0);
        c.apply(Input::Phase(SessionPhase::Idle), t0 + ms(10));
        c.apply(Input::Phase(SessionPhase::PreSession), t0 + ms(20));
        c.apply(Input::Phase(SessionPhase::Live), t0 + ms(30));
        assert_eq!(winner_at(&mut c, t0 + ms(30)), ("lamp", default_lamp()));
    }

    #[test]
    fn a_delayed_flag_cleared_waits_behind_the_chequered_flag() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(
            Input::Settings(Settings {
                tv_delay_ms: 5000,
                ..Settings::default()
            }),
            t0,
        );
        c.apply(flag(TrackFlag::SafetyCar, t0), t0);
        c.apply(race(RaceEvent::FlagCleared, t0 + ms(100)), t0 + ms(100));
        // Still the safety car until the TV shows the finish.
        assert_eq!(winner_at(&mut c, t0 + ms(5050)).0, "live track");
        assert_eq!(winner_at(&mut c, t0 + ms(5100)), ("lamp", default_lamp()));
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
    fn status_layer_follows_network_through_connect_error_and_recovery() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Phase(SessionPhase::Live), t0);
        c.apply(flag(TrackFlag::Green, t0), t0);

        let connecting = net_effect(NetStatus::Connecting).unwrap();
        assert_eq!(winner_at(&mut c, t0), ("status", connecting));

        c.apply(Input::Net(NetStatus::Online), t0 + ms(10));
        assert_eq!(winner_at(&mut c, t0 + ms(10)).0, "live track");

        c.apply(Input::Net(NetStatus::ApiError), t0 + ms(20));
        let error = net_effect(NetStatus::ApiError).unwrap();
        assert_eq!(winner_at(&mut c, t0 + ms(20)), ("status", error));

        c.apply(Input::Net(NetStatus::Online), t0 + ms(30));
        assert_eq!(winner_at(&mut c, t0 + ms(30)).0, "live track");
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
    fn winner_shows_above_the_last_flag_then_the_flag_returns() {
        let t0 = Instant::now();
        let mut c = live(t0);
        // Yellow: a green would have given way to the lamp by then.
        c.apply(flag(TrackFlag::Yellow, t0), t0);
        c.apply(winner(t0), t0);
        assert_eq!(winner_at(&mut c, t0), ("winner", winner_effect(TEAM)));
        let display = default_winner_display();
        assert_eq!(winner_at(&mut c, t0 + display).0, "live track");
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

    // ---- Switched-off events and followed drivers ----

    fn pit_stop(driver: u8, at: Instant) -> Input {
        race(
            RaceEvent::PitStop {
                driver,
                team_color: TEAM,
            },
            at,
        )
    }

    fn settings_with(effects: EffectToggles, followed: &[u8]) -> Input {
        Input::Settings(Settings {
            effects,
            followed_drivers: DriverSet::try_from(followed.to_vec()).unwrap(),
            ..Settings::default()
        })
    }

    #[test]
    fn switched_off_fastest_lap_leaves_the_track_flag_showing() {
        let t0 = Instant::now();
        let mut c = live(t0);
        let off = EffectToggles {
            fastest_lap: false,
            ..EffectToggles::default()
        };
        c.apply(settings_with(off, &[]), t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        c.apply(fastest_lap(t0), t0);
        assert_eq!(winner_at(&mut c, t0).0, "live track");
    }

    #[test]
    fn followed_drivers_pit_stop_flashes() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(settings_with(EffectToggles::default(), &[44]), t0);
        c.apply(pit_stop(44, t0), t0);
        assert_eq!(winner_at(&mut c, t0).0, "overlay");
    }

    #[test]
    fn pit_stop_of_a_driver_nobody_follows_is_ignored() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(settings_with(EffectToggles::default(), &[1]), t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        c.apply(pit_stop(44, t0), t0);
        assert_eq!(winner_at(&mut c, t0).0, "live track");
    }

    #[test]
    fn switched_off_winner_is_not_shown() {
        let t0 = Instant::now();
        let mut c = Controller::new(t0);
        c.apply(Input::Net(NetStatus::Online), t0);
        let off = EffectToggles {
            winner: false,
            ..EffectToggles::default()
        };
        c.apply(settings_with(off, &[]), t0);
        c.apply(winner(t0), t0);
        assert_eq!(winner_at(&mut c, t0).0, "lamp");
    }

    #[test]
    fn switching_an_effect_off_lets_a_running_one_finish() {
        // The switch decides when an event arrives; an overlay already
        // playing is short and simply ends.
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(fastest_lap(t0), t0);
        let off = EffectToggles {
            fastest_lap: false,
            ..EffectToggles::default()
        };
        c.apply(settings_with(off, &[]), t0);
        assert_eq!(winner_at(&mut c, t0).0, "overlay");
    }

    fn with_delay(ms_delay: u32) -> Input {
        Input::Settings(Settings {
            tv_delay_ms: ms_delay,
            ..Settings::default()
        })
    }

    /// Live, online, with a 10 s TV delay.
    fn live_delayed(t0: Instant) -> Controller {
        let mut c = live(t0);
        c.apply(with_delay(10_000), t0);
        c
    }

    #[test]
    fn flag_waits_for_the_tv_delay() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        c.apply(flag(TrackFlag::Yellow, t0), t0);
        assert_ne!(winner_at(&mut c, t0 + ms(9_999)).0, "live track");
        assert_eq!(
            winner_at(&mut c, t0 + ms(10_000)),
            ("live track", flag_effect(TrackFlag::Yellow))
        );
    }

    #[test]
    fn delay_counts_from_arrival_not_from_apply() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        // Received at t0, handed to the controller 3 s later.
        c.apply(flag(TrackFlag::Red, t0), t0 + ms(3_000));
        assert_eq!(winner_at(&mut c, t0 + ms(10_000)).0, "live track");
    }

    #[test]
    fn zero_delay_applies_at_once_even_without_a_tick() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Red, t0), t0);
        assert_eq!(
            c.arbitrate(),
            (
                "live track",
                Scene {
                    effect: flag_effect(TrackFlag::Red),
                    started: t0
                }
            )
        );
    }

    #[test]
    fn phase_change_waits_for_the_tv_delay() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        c.apply(Input::Phase(SessionPhase::PostSession), t0);
        assert_eq!(c.snapshot().phase, SessionPhase::Live);
        c.tick(t0 + ms(10_000));
        assert_eq!(c.snapshot().phase, SessionPhase::PostSession);
    }

    #[test]
    fn chequered_flag_shows_before_the_lamp_leaves_live() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        let chequered = Input::Race {
            event: RaceEvent::ChequeredFlag,
            received: t0 + ms(5_000),
        };
        c.apply(chequered, t0 + ms(5_000));
        // The scheduler ends the session a second after the flag arrived.
        c.apply(Input::Phase(SessionPhase::PostSession), t0 + ms(6_000));

        let at_flag = t0 + ms(15_000);
        assert_eq!(winner_at(&mut c, at_flag).0, "overlay");
        assert_eq!(c.snapshot().phase, SessionPhase::Live);
        winner_at(&mut c, t0 + ms(16_000));
        assert_eq!(c.snapshot().phase, SessionPhase::PostSession);
    }

    #[test]
    fn overrides_and_settings_ignore_the_delay() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        c.apply(Input::Override(Some(Effect::Solid(Rgb::WHITE))), t0);
        assert_eq!(winner_at(&mut c, t0).0, "override");
        let mut brighter = Settings {
            tv_delay_ms: 10_000,
            ..Settings::default()
        };
        brighter.lamp_brightness = 1.0;
        c.apply(Input::Settings(brighter), t0);
        assert!((c.snapshot().settings.lamp_brightness - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn events_keep_their_order_through_the_delay() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        c.apply(flag(TrackFlag::Yellow, t0), t0);
        c.apply(flag(TrackFlag::SafetyCar, t0), t0);
        assert_eq!(
            winner_at(&mut c, t0 + ms(10_000)),
            ("live track", flag_effect(TrackFlag::SafetyCar))
        );
    }

    #[test]
    fn changing_the_delay_leaves_queued_events_where_they_are() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        c.apply(flag(TrackFlag::Red, t0), t0);
        c.apply(with_delay(0), t0 + ms(1));
        assert_ne!(winner_at(&mut c, t0 + ms(5_000)).0, "live track");
        assert_eq!(winner_at(&mut c, t0 + ms(10_000)).0, "live track");
    }

    fn start(at: Instant) -> Input {
        Input::Race {
            event: RaceEvent::StartLights,
            received: at,
        }
    }

    fn start_lights() -> Effect {
        event_overlay(RaceEvent::StartLights).unwrap()
    }

    #[test]
    fn start_lights_go_out_exactly_when_the_delayed_start_is_released() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        // The feed's start: lights and green flag, both received at lights out.
        c.apply(start(t0), t0);
        c.apply(flag(TrackFlag::Green, t0), t0);

        // 10 s delay - 6 s sequence: the lights begin 4 s after the feed said go.
        assert_ne!(winner_at(&mut c, t0 + ms(3_999)).0, "overlay");
        let (layer, effect) = winner_at(&mut c, t0 + ms(4_000));
        assert_eq!((layer, effect), ("overlay", start_lights()));
        let lights_out = t0 + ms(4_000) + start_lights().lights_out_after().unwrap();
        assert_eq!(lights_out, t0 + ms(10_000));

        // The green flag is released at that same moment, under the last
        // second of darkness, and shows once the lights have finished.
        let (_, scene) = c.arbitrate();
        assert_eq!(scene.started, t0 + ms(4_000));
        winner_at(&mut c, t0 + ms(10_000));
        assert_eq!(
            winner_at(&mut c, t0 + ms(11_000)),
            ("live track", flag_effect(TrackFlag::Green))
        );
    }

    #[test]
    fn with_a_two_second_delay_no_start_lights_are_scheduled() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(with_delay(2_000), t0);
        c.apply(start(t0), t0);
        for t in (0..=12_000).step_by(500) {
            assert_ne!(winner_at(&mut c, t0 + ms(t)).0, "overlay", "at {t} ms");
        }
    }

    #[test]
    fn without_a_delay_no_start_lights_are_scheduled() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(start(t0), t0);
        assert_ne!(winner_at(&mut c, t0).0, "overlay");
    }

    #[test]
    fn delay_equal_to_the_sequence_starts_the_lights_at_once() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(with_delay(6_000), t0);
        c.apply(start(t0), t0);
        assert_eq!(winner_at(&mut c, t0), ("overlay", start_lights()));
    }

    #[test]
    fn switched_off_start_lights_stay_off_with_a_delay() {
        let t0 = Instant::now();
        let mut c = live(t0);
        let mut settings = Settings {
            tv_delay_ms: 10_000,
            ..Settings::default()
        };
        settings.effects.start_lights = false;
        c.apply(Input::Settings(settings), t0);
        c.apply(start(t0), t0);
        assert_ne!(winner_at(&mut c, t0 + ms(4_000)).0, "overlay");
    }

    fn with_green_display(ms_display: u32) -> Input {
        Input::Settings(Settings {
            green_display_ms: ms_display,
            ..Settings::default()
        })
    }

    #[test]
    fn green_gives_way_to_the_lamp_after_its_display_time() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        assert_eq!(
            winner_at(&mut c, t0 + ms(9_999)),
            ("live track", flag_effect(TrackFlag::Green))
        );
        assert_eq!(winner_at(&mut c, t0 + ms(10_000)), ("lamp", default_lamp()));
    }

    #[test]
    fn other_flags_stay_as_long_as_they_last() {
        let t0 = Instant::now();
        for f in [
            TrackFlag::Yellow,
            TrackFlag::DoubleYellow,
            TrackFlag::SafetyCar,
            TrackFlag::VirtualSafetyCar,
            TrackFlag::Red,
        ] {
            let mut c = live(t0);
            c.apply(flag(f, t0), t0);
            assert_eq!(
                winner_at(&mut c, t0 + ms(600_000)),
                ("live track", flag_effect(f)),
                "{f:?}"
            );
        }
    }

    #[test]
    fn green_after_a_yellow_shows_again_for_its_display_time() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        winner_at(&mut c, t0 + ms(15_000));
        c.apply(flag(TrackFlag::Yellow, t0 + ms(20_000)), t0 + ms(20_000));
        c.apply(flag(TrackFlag::Green, t0 + ms(30_000)), t0 + ms(30_000));
        assert_eq!(winner_at(&mut c, t0 + ms(35_000)).0, "live track");
        assert_eq!(winner_at(&mut c, t0 + ms(40_000)).0, "lamp");
    }

    #[test]
    fn zero_green_display_keeps_green() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(with_green_display(0), t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        assert_eq!(winner_at(&mut c, t0 + ms(3_600_000)).0, "live track");
    }

    #[test]
    fn green_display_counts_from_when_the_tv_shows_it() {
        let t0 = Instant::now();
        let mut c = live_delayed(t0);
        c.apply(flag(TrackFlag::Green, t0), t0);
        // Released at +10 s by the TV delay (the lamp ticks every 20 ms),
        // so shown until +20 s.
        winner_at(&mut c, t0 + ms(10_000));
        assert_eq!(winner_at(&mut c, t0 + ms(19_999)).0, "live track");
        assert_eq!(winner_at(&mut c, t0 + ms(20_000)).0, "lamp");
    }

    #[test]
    fn forced_green_override_stays_until_released() {
        let t0 = Instant::now();
        let mut c = live(t0);
        c.apply(Input::Override(Some(flag_effect(TrackFlag::Green))), t0);
        assert_eq!(winner_at(&mut c, t0 + ms(60_000)).0, "override");
    }
}
