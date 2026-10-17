//! One connection to F1's live feed, without the networking: bytes from the
//! WebSocket go in, race events for the lamp come out.
//!
//! The answer to `Subscribe` holds the current state of every stream. It is
//! applied silently, the way a replay jumps: connecting mid-race must not
//! replay the start lights or a chequered flag. Afterwards only the current
//! flag is sent, so the lamp shows it at once.

use std::time::Duration;

use chrono::DateTime;
use serde_json::value::RawValue;

use crate::{
    feed::{clock_ms, ExtrapolatedClock, FeedMessage, Stream},
    input::{RaceEvent, TimingInfo},
    signalr::{self, Message, Splitter},
    track_state::TrackState,
    tracker::Tracker,
};

/// The order the initial state is applied in: what the session is and who
/// drives first, its status last, so the tracker knows everything it needs.
/// Race control is left out on purpose: late in a race it holds hundreds of
/// messages, which would cost tens of KB to parse, and all they would add is
/// which sectors have a double yellow at the moment of connecting.
const INITIAL_ORDER: [Stream; 8] = [
    Stream::SessionInfo,
    Stream::SessionData,
    Stream::DriverList,
    Stream::TopThree,
    Stream::TrackStatus,
    Stream::SessionStatus,
    // After the status: on the last lap already when connecting, it counts
    // as sent (silently, like everything here).
    Stream::LapCount,
    Stream::Clock,
];

/// One SignalR message at most, for [`LiveSession::new`] on the device. The
/// state the lamp uses arrives as about 16 KB after a race. The race control
/// history (about 45 KB) is meant to exceed it: the splitter skips it without
/// buffering, which logs one "frame larger than" warning per connection.
/// Buffering it would need a single 45 KB block, which the heap often doesn't
/// have. `tests/live_capture.rs` checks both against a real answer.
pub const MAX_FRAME: usize = 24 * 1024;

/// The most frames [`LiveSession::opening_frames`] gives: the device's
/// longest hold of its TLS lock includes sending them.
pub const MAX_OPENING_FRAMES: usize = 6;

/// Also subscribed to, so a quiet session still shows the connection is alive.
const HEARTBEAT: &str = "Heartbeat";

/// Invocation ids of the `Subscribe` calls. The state's answer is applied,
/// the timing stats' answer read for the fastest lap's holder and the
/// account call's for the running order; the others answer with history.
const STATE_CALL: u32 = 1;
const RACE_CONTROL_CALL: u32 = 2;
const DRIVER_EVENTS_CALL: u32 = 3;
const ACCOUNT_CALL: u32 = 4;
const TIMING_STATS_CALL: u32 = 5;

/// The streams whose current state is applied on connecting.
const STATE_STREAMS: [Stream; 8] = [
    Stream::SessionInfo,
    Stream::SessionData,
    Stream::TrackStatus,
    Stream::SessionStatus,
    Stream::DriverList,
    Stream::TopThree,
    Stream::LapCount,
    Stream::Clock,
];
/// Public streams with driver events: pit stops and qualifying knockouts.
/// TimingData's answer to `Subscribe` (the whole timing screen) is larger
/// than a frame and skipped; only its updates count. TimingStats (fastest
/// laps) has a call of its own: its answer (about 8 KB mid-session) says who
/// holds the fastest lap, and next to TimingData's it would be skipped too.
const DRIVER_EVENT_STREAMS: [Stream; 2] = [Stream::PitLane, Stream::TimingData];
/// Streams that only answer with an F1TV token: the running order, for
/// overtakes. Its answer is about 4 KB.
const ACCOUNT_STREAMS: [Stream; 1] = [Stream::DriverRaceInfo];

/// What one batch of received bytes produced.
#[derive(Debug, Default, PartialEq)]
pub struct Received {
    pub events: Vec<RaceEvent>,
    /// Things worth a log line: frames that didn't parse, refused calls.
    pub problems: Vec<String>,
    /// Frames too large for the splitter, among `problems`: the moment the
    /// most may be waiting in the WebSocket's queue.
    pub dropped_frames: usize,
    /// The server answered `Subscribe` in this batch.
    pub subscribed: bool,
    /// The server is closing the connection, with its reason.
    pub closed: Option<String>,
    /// The session key from SessionInfo, once known.
    pub session_key: Option<u32>,
    /// Whether the session still goes on, once the answer to `Subscribe`
    /// has said where it stands.
    pub running: Option<bool>,
    /// Clock and lap readings, for the page's TV clock and lap.
    pub timing: Vec<TimingInfo>,
    /// Drivers' short names (`"VER"`) by car number, for the race log.
    pub names: Vec<(u8, String)>,
}

pub struct LiveSession {
    splitter: Splitter,
    state: TrackState,
    tracker: Tracker,
    session_key: Option<u32>,
    subscribed: bool,
}

impl LiveSession {
    /// `max_frame` caps one SignalR message; the `Subscribe` answer is the
    /// largest by far.
    pub fn new(max_frame: usize) -> Self {
        LiveSession {
            splitter: Splitter::new(max_frame),
            state: TrackState::default(),
            tracker: Tracker::default(),
            session_key: None,
            subscribed: false,
        }
    }

    /// What to send once the WebSocket is open: the handshake, then the
    /// subscriptions. `with_token`: the connection carries an F1TV token, so
    /// the account-only streams are asked for too.
    ///
    /// Only the first `Subscribe` answer is used. Every stream whose answer
    /// is just history gets a call of its own: race control's is 44 KB after
    /// a race. As separate frames, those larger than
    /// `max_frame` are skipped by the splitter without buffering, so the
    /// device never needs one block that big, and the first answer stays
    /// small.
    pub fn opening_frames(with_token: bool) -> Vec<String> {
        let names = |streams: &[Stream]| streams.iter().map(|s| s.name()).collect::<Vec<_>>();
        let mut state = names(&STATE_STREAMS);
        state.push(HEARTBEAT);
        let mut frames = vec![
            signalr::HANDSHAKE.to_owned(),
            signalr::subscribe(STATE_CALL, &state),
            signalr::subscribe(TIMING_STATS_CALL, &[Stream::TimingStats.name()]),
            signalr::subscribe(RACE_CONTROL_CALL, &[Stream::RaceControl.name()]),
            signalr::subscribe(DRIVER_EVENTS_CALL, &names(&DRIVER_EVENT_STREAMS)),
        ];
        if with_token {
            frames.push(signalr::subscribe(ACCOUNT_CALL, &names(&ACCOUNT_STREAMS)));
        }
        frames
    }

    /// A WebSocket frame of `len` bytes is starting: room for it in one go.
    pub fn expect(&mut self, len: usize) {
        self.splitter.reserve(len);
    }

    pub fn receive(&mut self, bytes: &[u8]) -> Received {
        let mut out = Received::default();
        for frame in self.splitter.push(bytes) {
            match frame {
                Ok(frame) => self.frame(&frame, &mut out),
                Err(e) => {
                    if matches!(e, signalr::FrameError::TooLarge { .. }) {
                        out.dropped_frames += 1;
                    }
                    out.problems.push(e.to_string());
                }
            }
        }
        out.session_key = self.session_key;
        out.running = self.subscribed.then(|| self.state.session_running());
        out
    }

    fn frame(&mut self, frame: &str, out: &mut Received) {
        let msg = match signalr::parse(frame) {
            Ok(msg) => msg,
            Err(e) => {
                out.problems.push(format!("not a SignalR message: {e}"));
                return;
            }
        };
        match &msg {
            Message::Handshake { error: Some(e) } => out.closed = Some(format!("handshake: {e}")),
            Message::Completion { error: Some(e), .. } => {
                out.problems.push(format!("Subscribe refused: {e}"));
            }
            // Invocation ids are strings in SignalR's JSON (`subscribe` sends
            // "1"), so the number is compared as text.
            Message::Completion {
                invocation_id,
                result: Some(result),
                ..
            } if invocation_id.as_deref() == Some(&STATE_CALL.to_string()) => {
                self.initial_state(result, out);
                self.subscribed = true;
                out.subscribed = true;
            }
            Message::Completion {
                invocation_id,
                result: Some(result),
                ..
            } if invocation_id.as_deref() == Some(&TIMING_STATS_CALL.to_string()) => {
                self.read_silently(Stream::TimingStats, result, out);
            }
            Message::Completion {
                invocation_id,
                result: Some(result),
                ..
            } if invocation_id.as_deref() == Some(&ACCOUNT_CALL.to_string()) => {
                self.read_silently(Stream::DriverRaceInfo, result, out);
            }
            Message::Invocation { .. } => {
                if let Some(update) = msg.feed_update() {
                    // Every update moves the tracker's clock, also those
                    // whose content is skipped.
                    if let Some(at) = update.timestamp.as_deref().and_then(feed_time) {
                        out.events.extend(self.tracker.tick(at));
                    }
                    if let Some(stream) = Stream::from_name(&update.stream) {
                        self.update(stream, update.data.get(), out);
                    }
                }
            }
            Message::Close { error } => {
                out.closed = Some(error.clone().unwrap_or_else(|| "no reason".to_owned()));
            }
            // Among these: the answers to the other `Subscribe` calls, which
            // are only history (race control, pit stops so far).
            Message::Handshake { error: None }
            | Message::Completion { .. }
            | Message::Ping
            | Message::Other(_) => {}
        }
    }

    fn initial_state(&mut self, result: &RawValue, out: &mut Received) {
        let state = match signalr::initial_state(result) {
            Ok(state) => state,
            Err(e) => {
                out.problems.push(format!("initial state: {e}"));
                return;
            }
        };
        for stream in INITIAL_ORDER {
            let Some((_, json)) = state.iter().find(|(name, _)| name == stream.name()) else {
                continue;
            };
            match stream.parse(json.get()) {
                Ok(messages) => {
                    for msg in messages {
                        self.note(&msg);
                        out.timing.extend(timing(&msg));
                        out.names.extend(names(&msg));
                        // Silently: the events of the past are not shown.
                        self.tracker.apply(&msg);
                        self.state.apply(msg);
                    }
                }
                Err(e) => out
                    .problems
                    .push(format!("{} (initial): {e}", stream.name())),
            }
        }
        if let Some(flag) = self.state.flag() {
            out.events.push(RaceEvent::TrackFlag(flag));
        }
    }

    /// The current state of `stream` from a `Subscribe` answer, for the
    /// tracker to learn from silently. TimingStats says who holds the
    /// fastest lap, so the holder beating their own time counts after a
    /// reconnect too; DriverRaceInfo gives every car's place, so the first
    /// pass counts. What they hold happened before the lamp connected.
    fn read_silently(&mut self, stream: Stream, result: &RawValue, out: &mut Received) {
        let found = signalr::initial_state(result)
            .map(|state| state.into_iter().find(|(name, _)| name == stream.name()));
        match found {
            Ok(Some((_, json))) => match stream.parse(json.get()) {
                Ok(messages) => {
                    for msg in messages {
                        self.tracker.apply(&msg);
                    }
                }
                Err(e) => out
                    .problems
                    .push(format!("{} (initial): {e}", stream.name())),
            },
            Ok(None) => {}
            Err(e) => out.problems.push(format!("{} state: {e}", stream.name())),
        }
    }

    fn update(&mut self, stream: Stream, json: &str, out: &mut Received) {
        if !stream.is_relevant(json) {
            return;
        }
        match stream.parse(json) {
            Ok(messages) => {
                for msg in messages {
                    self.note(&msg);
                    out.timing.extend(timing(&msg));
                    out.names.extend(names(&msg));
                    out.events.extend(self.tracker.apply(&msg));
                    out.events.extend(self.state.apply(msg));
                }
            }
            Err(e) => out.problems.push(format!("{}: {e}", stream.name())),
        }
    }

    fn note(&mut self, msg: &FeedMessage) {
        if let FeedMessage::SessionInfo(info) = msg {
            if info.key.is_some() {
                self.session_key = info.key;
            }
        }
    }
}

/// The short names in a driver list message.
fn names(msg: &FeedMessage) -> Vec<(u8, String)> {
    let FeedMessage::DriverList(list) = msg else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|(number, driver)| Some((*number, driver.tla.clone()?)))
        .collect()
}

/// The clock, lap or lap time readings in `msg`.
fn timing(msg: &FeedMessage) -> Vec<TimingInfo> {
    match msg {
        FeedMessage::Clock(clock) => Vec::from_iter(clock_reading(clock)),
        FeedMessage::LapCount(laps) => vec![TimingInfo::Lap {
            current: laps.current_lap,
            total: laps.total_laps,
        }],
        FeedMessage::TimingData(data) => data
            .lines
            .iter()
            .flat_map(|lines| lines.iter())
            .filter_map(|(&car, line)| {
                let ms = line.last_lap_time.as_ref()?.ms()?;
                Some(TimingInfo::LapTime { car, ms })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn clock_reading(clock: &ExtrapolatedClock) -> Option<TimingInfo> {
    Some(TimingInfo::Clock {
        remaining_ms: clock.remaining.as_deref().and_then(clock_ms)?,
        running: clock.extrapolating.unwrap_or(false),
        utc_ms: clock
            .utc
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.timestamp_millis()),
    })
}

/// An update's timestamp, e.g. `2026-10-09T13:36:00.133Z`, as time since
/// the Unix epoch.
fn feed_time(timestamp: &str) -> Option<Duration> {
    let millis = DateTime::parse_from_rfc3339(timestamp)
        .ok()?
        .timestamp_millis();
    u64::try_from(millis).ok().map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::TrackFlag;

    const RS: &str = "\u{1e}";

    fn completion(result: &str) -> String {
        format!(r#"{{"type":3,"invocationId":"1","result":{result}}}{RS}"#)
    }

    fn feed(stream: &str, data: &str) -> String {
        format!(
            r#"{{"type":1,"target":"feed","arguments":["{stream}",{data},"2026-10-09T09:00:00Z"]}}{RS}"#
        )
    }

    /// A session already running under a safety car, with #81 leading.
    fn racing() -> (LiveSession, Received) {
        let mut live = LiveSession::new(64 * 1024);
        live.receive(format!("{{}}{RS}").as_bytes());
        let received = live.receive(
            completion(
                r#"{"SessionInfo":{"Type":"Race","Name":"Race","Key":11379,"_kf":true},
                "DriverList":{"81":{"TeamColour":"F47600"},"_kf":true},
                "TopThree":{"Lines":[{"RacingNumber":"81"}],"_kf":true},
                "TrackStatus":{"Status":"4","Message":"SCDeployed","_kf":true},
                "SessionStatus":{"Status":"Started","_kf":true},
                "RaceControlMessages":{"Messages":[{"Category":"Flag","Flag":"CHEQUERED"}],"_kf":true},
                "Heartbeat":{"Utc":"2026-10-09T09:00:00Z","_kf":true}}"#,
            )
            .as_bytes(),
        );
        (live, received)
    }

    #[test]
    fn opening_frames_are_handshake_then_four_subscribes_without_token() {
        let frames = LiveSession::opening_frames(false);
        let [handshake, state, timing_stats, race_control, driver_events] = &frames[..] else {
            panic!("{} frames", frames.len())
        };
        assert!(timing_stats.contains("[[\"TimingStats\"]]"));
        assert!(timing_stats.contains("\"invocationId\":\"5\""));
        assert!(
            driver_events.contains("PitLaneTimeCollection")
                && !driver_events.contains("TimingStats")
        );
        assert!(!frames.iter().any(|f| f.contains("DriverRaceInfo")));
        assert!(!state.contains("TimingStats"));
        assert_eq!(handshake, signalr::HANDSHAKE);
        assert!(state.contains("\"TrackStatus\"") && state.contains("\"Heartbeat\""));
        assert!(!state.contains("RaceControlMessages"));
        assert!(race_control.contains("[[\"RaceControlMessages\"]]"));
        assert!(race_control.contains("\"invocationId\":\"2\""));
    }

    #[test]
    fn race_control_history_answer_is_ignored() {
        let (mut live, _) = racing();
        let r = live.receive(
            format!(
                r#"{{"type":3,"invocationId":"2","result":{{"RaceControlMessages":{{"Messages":[{{"Category":"Flag","Flag":"CHEQUERED"}}]}}}}}}{RS}"#
            )
            .as_bytes(),
        );
        assert!(r.events.is_empty() && !r.subscribed);
    }

    #[test]
    fn oversized_race_control_history_is_skipped_and_updates_still_arrive() {
        let mut live = LiveSession::new(512);
        live.receive(
            completion(r#"{"TrackStatus":{"Status":"1"},"SessionStatus":{"Status":"Started"}}"#)
                .as_bytes(),
        );
        let history = format!(
            r#"{{"type":3,"invocationId":"2","result":{{"RaceControlMessages":{{"Messages":[{}]}}}}}}{RS}"#,
            vec![r#"{"Category":"Other","Message":"LONG HISTORY"}"#; 40].join(",")
        );
        live.expect(history.len());
        let (mut problems, mut dropped) = (0, 0);
        for piece in history.as_bytes().chunks(100) {
            let r = live.receive(piece);
            problems += r.problems.len();
            dropped += r.dropped_frames;
        }
        assert_eq!((problems, dropped), (1, 1));
        let r = live.receive(feed("TrackStatus", r#"{"Status":"2"}"#).as_bytes());
        assert_eq!(r.events, [RaceEvent::TrackFlag(TrackFlag::Yellow)]);
    }

    #[test]
    fn joining_mid_race_shows_only_the_current_flag() {
        let (_, received) = racing();
        assert!(received.subscribed);
        assert!(received.problems.is_empty(), "{:?}", received.problems);
        // No start lights, no chequered flag from the race control history.
        assert_eq!(
            received.events,
            [RaceEvent::TrackFlag(TrackFlag::SafetyCar)]
        );
        assert_eq!(received.session_key, Some(11379));
    }

    #[test]
    fn updates_after_joining_change_the_flag() {
        let (mut live, _) = racing();
        let received =
            live.receive(feed("TrackStatus", r#"{"Status":"1","Message":"AllClear"}"#).as_bytes());
        assert_eq!(received.events, [RaceEvent::TrackFlag(TrackFlag::Green)]);
    }

    #[test]
    fn finish_after_joining_shows_the_leader_as_winner() {
        let (mut live, _) = racing();
        let mut events = live
            .receive(
                feed(
                    "RaceControlMessages",
                    r#"{"Messages":{"40":{"Category":"Flag","Flag":"CHEQUERED","Scope":"Track"}}}"#,
                )
                .as_bytes(),
            )
            .events;
        events.extend(
            live.receive(feed("SessionStatus", r#"{"Status":"Finished"}"#).as_bytes())
                .events,
        );
        assert_eq!(
            events,
            [
                RaceEvent::ChequeredFlag,
                RaceEvent::Winner {
                    driver: 81,
                    team_color: "#f47600".parse().unwrap()
                },
                RaceEvent::FlagCleared,
            ]
        );
    }

    #[test]
    fn joining_before_the_start_shows_nothing_then_the_start_plays() {
        let mut live = LiveSession::new(64 * 1024);
        let received = live.receive(
            completion(r#"{"SessionStatus":{"Status":"Inactive"},"TrackStatus":{"Status":"1"}}"#)
                .as_bytes(),
        );
        assert!(received.events.is_empty());
        let received = live.receive(feed("SessionStatus", r#"{"Status":"Started"}"#).as_bytes());
        assert_eq!(
            received.events,
            [
                RaceEvent::StartLights,
                RaceEvent::TrackFlag(TrackFlag::Green)
            ]
        );
    }

    #[test]
    fn joining_after_the_finish_shows_no_winner() {
        let mut live = LiveSession::new(64 * 1024);
        let received = live.receive(
            completion(
                r#"{"SessionInfo":{"Type":"Race"},"TopThree":{"Lines":[{"RacingNumber":"81"}]},"SessionStatus":{"Status":"Finished"}}"#,
            )
            .as_bytes(),
        );
        assert!(received.events.is_empty());
    }

    #[test]
    fn joining_after_the_last_qualifying_flag_shows_pole_at_the_cap() {
        // Car 81 has sent no TimingData, so it may still be on track; the
        // two minutes count from the first update after connecting.
        let mut live = LiveSession::new(64 * 1024);
        live.receive(
            completion(
                r#"{"SessionInfo":{"Type":"Qualifying","Name":"Qualifying"},
                "SessionData":{"Series":[{"Utc":"t","QualifyingPart":3}]},
                "DriverList":{"81":{"TeamColour":"F47600"}},
                "TopThree":{"Lines":[{"RacingNumber":"81"}]},
                "SessionStatus":{"Status":"Finished"}}"#,
            )
            .as_bytes(),
        );
        let heartbeat = |at: &str| {
            format!(
                r#"{{"type":1,"target":"feed","arguments":["Heartbeat",{{"Utc":"{at}"}},"{at}"]}}{RS}"#
            )
        };
        let r = live.receive(heartbeat("2026-10-09T09:00:00Z").as_bytes());
        assert!(r.events.is_empty());
        let r = live.receive(heartbeat("2026-10-09T09:02:01Z").as_bytes());
        assert_eq!(
            r.events,
            [RaceEvent::Winner {
                driver: 81,
                team_color: "#f47600".parse().unwrap()
            }]
        );
    }

    #[test]
    fn bytes_can_arrive_in_any_pieces() {
        let (_, whole) = racing();
        let mut live = LiveSession::new(64 * 1024);
        let mut all = Received::default();
        let text = completion(
            r#"{"SessionInfo":{"Type":"Race","Key":11379},"TrackStatus":{"Status":"4"},"SessionStatus":{"Status":"Started"}}"#,
        );
        for piece in text.as_bytes().chunks(7) {
            let r = live.receive(piece);
            all.events.extend(r.events);
            all.subscribed |= r.subscribed;
        }
        assert!(all.subscribed);
        assert_eq!(all.events, whole.events);
    }

    #[test]
    fn heartbeat_and_unknown_streams_are_ignored() {
        let (mut live, _) = racing();
        let r = live.receive(feed("Heartbeat", r#"{"Utc":"2026-10-09T09:00:15Z"}"#).as_bytes());
        assert_eq!(
            r,
            Received {
                session_key: Some(11379),
                running: Some(true),
                ..Received::default()
            }
        );
        let r = live.receive(feed("WeatherData", r#"{"AirTemp":"30"}"#).as_bytes());
        assert!(r.events.is_empty() && r.problems.is_empty());
    }

    #[test]
    fn a_broken_update_is_reported_and_the_session_goes_on() {
        let (mut live, _) = racing();
        let r = live.receive(feed("TrackStatus", r#"{"Status":5}"#).as_bytes());
        assert_eq!(r.problems.len(), 1);
        let r = live.receive(feed("TrackStatus", r#"{"Status":"5"}"#).as_bytes());
        assert_eq!(r.events, [RaceEvent::TrackFlag(TrackFlag::Red)]);
    }

    #[test]
    fn whether_the_session_runs_is_unknown_until_subscribed() {
        let mut live = LiveSession::new(1024);
        let r = live.receive(format!("{{}}{RS}").as_bytes());
        assert_eq!(r.running, None);
    }

    #[test]
    fn a_race_runs_until_it_finishes() {
        let (mut live, received) = racing();
        assert_eq!(received.running, Some(true));
        let r = live.receive(feed("SessionStatus", r#"{"Status":"Finished"}"#).as_bytes());
        assert_eq!(r.running, Some(false));
    }

    #[test]
    fn server_close_is_reported() {
        let mut live = LiveSession::new(1024);
        let r =
            live.receive(format!(r#"{{"type":7,"error":"Server shutting down"}}{RS}"#).as_bytes());
        assert_eq!(r.closed.as_deref(), Some("Server shutting down"));
    }

    #[test]
    fn refused_handshake_closes() {
        let mut live = LiveSession::new(1024);
        let r = live.receive(format!(r#"{{"error":"Protocol not supported"}}{RS}"#).as_bytes());
        assert!(r.closed.is_some());
    }

    #[test]
    fn too_large_initial_state_is_a_problem_not_a_crash() {
        let mut live = LiveSession::new(64);
        let r = live.receive(
            completion(r#"{"TrackStatus":{"Status":"4","Message":"SCDeployed"}}"#).as_bytes(),
        );
        assert!(!r.subscribed);
        assert_eq!(r.problems.len(), 1);
    }

    #[test]
    fn opening_frames_never_exceed_their_maximum() {
        for with_token in [false, true] {
            assert!(LiveSession::opening_frames(with_token).len() <= MAX_OPENING_FRAMES);
        }
    }

    #[test]
    fn with_a_token_the_running_order_gets_a_fifth_subscribe() {
        let frames = LiveSession::opening_frames(true);
        assert_eq!(frames.len(), 6);
        assert!(frames[5].contains("[[\"DriverRaceInfo\"]]"));
        assert!(frames[5].contains("\"invocationId\":\"4\""));
    }

    #[test]
    fn the_fastest_lap_holder_is_read_silently_on_connecting() {
        let (mut live, _) = racing();
        // Singapore FP1 2026, joined mid-session: #81 holds the fastest lap.
        let answer = format!(
            r#"{{"type":3,"invocationId":"5","result":{{"TimingStats":{{"Lines":{{"81":{{"PersonalBestLapTime":{{"Value":"1:33.830","Lap":11,"Position":1}}}},"4":{{"PersonalBestLapTime":{{"Value":"1:35.515","Lap":9,"Position":12}}}}}},"_kf":true}}}}}}{RS}"#
        );
        let r = live.receive(answer.as_bytes());
        assert!(r.events.is_empty() && r.problems.is_empty(), "{r:?}");
        // Still P1, so no position: a fastest lap only if #81 is known to hold it.
        let r = live.receive(
            feed(
                "TimingStats",
                r#"{"Lines":{"81":{"PersonalBestLapTime":{"Lap":12,"Value":"1:33.702"}}}}"#,
            )
            .as_bytes(),
        );
        assert!(
            matches!(r.events[..], [RaceEvent::FastestLap { driver: 81, .. }]),
            "{:?}",
            r.events
        );
    }

    #[test]
    fn the_running_order_is_read_silently_and_the_first_pass_counts() {
        let (mut live, _) = racing();
        live.receive(
            feed(
                "DriverList",
                r#"{"4":{"TeamColour":"F47600"},"16":{"TeamColour":"ED1131"}}"#,
            )
            .as_bytes(),
        );
        let answer = format!(
            r#"{{"type":3,"invocationId":"4","result":{{"DriverRaceInfo":{{"81":{{"RacingNumber":"81","Position":"1","Gap":"LAP 12","OvertakeState":1,"IsOut":false}},"4":{{"RacingNumber":"4","Position":"2","OvertakeState":1}},"16":{{"RacingNumber":"16","Position":"3","OvertakeState":1}},"_kf":true}}}}}}{RS}"#
        );
        let r = live.receive(answer.as_bytes());
        assert!(r.events.is_empty() && r.problems.is_empty(), "{r:?}");
        let r = live.receive(
            feed(
                "DriverRaceInfo",
                r#"{"4":{"Position":"3","Interval":"","OvertakeState":0},"16":{"Position":"2","Interval":"","OvertakeState":2}}"#,
            )
            .as_bytes(),
        );
        assert_eq!(
            r.events,
            [RaceEvent::Overtake {
                driver: 16,
                team_color: "#ed1131".parse().unwrap()
            }]
        );
    }

    #[test]
    fn pit_stop_update_after_joining_flashes_in_team_colour() {
        let (mut live, _) = racing();
        let r = live.receive(
            feed(
                "PitLaneTimeCollection",
                r#"{"PitTimes":{"81":{"RacingNumber":"81","Duration":"23.9"}}}"#,
            )
            .as_bytes(),
        );
        assert_eq!(
            r.events,
            [RaceEvent::PitStop {
                driver: 81,
                team_color: "#f47600".parse().unwrap()
            }]
        );
    }

    #[test]
    fn driver_event_history_answers_are_ignored() {
        let (mut live, _) = racing();
        for id in ["3", "4"] {
            let r = live.receive(
                format!(
                    r#"{{"type":3,"invocationId":"{id}","result":{{"PitLaneTimeCollection":{{"PitTimes":{{"81":{{"Duration":"23.9"}}}}}}}}}}{RS}"#
                )
                .as_bytes(),
            );
            assert!(r.events.is_empty() && !r.subscribed, "call {id}");
        }
    }

    #[test]
    fn feed_timestamps_are_read_to_the_millisecond() {
        let a = feed_time("2026-10-09T13:36:00.133Z").unwrap();
        let b = feed_time("2026-10-09T13:38:00.1331234Z").unwrap();
        assert_eq!(b - a, Duration::from_secs(120));
        assert_eq!(feed_time("not a time"), None);
    }

    #[test]
    fn clock_and_lap_updates_are_passed_on() {
        let (mut live, _) = racing();
        let received = live.receive(
            feed(
                "ExtrapolatedClock",
                r#"{"Utc":"2026-10-09T09:00:00.000Z","Remaining":"00:17:42","Extrapolating":true}"#,
            )
            .as_bytes(),
        );
        assert_eq!(
            received.timing,
            [TimingInfo::Clock {
                remaining_ms: 1_062_000,
                running: true,
                utc_ms: Some(1_791_536_400_000),
            }]
        );
        let received = live.receive(feed("LapCount", r#"{"CurrentLap":12}"#).as_bytes());
        assert_eq!(
            received.timing,
            [TimingInfo::Lap {
                current: Some(12),
                total: None
            }]
        );
    }

    #[test]
    fn lap_times_are_passed_on() {
        let (mut live, _) = racing();
        let received = live.receive(
            feed(
                "TimingData",
                r#"{"Lines":{"63":{"NumberOfLaps":2,"LastLapTime":{"Value":"1:26.651"}},"10":{"LastLapTime":{"Value":""}}}}"#,
            )
            .as_bytes(),
        );
        assert_eq!(
            received.timing,
            [TimingInfo::LapTime {
                car: 63,
                ms: 86_651
            }]
        );
    }

    #[test]
    fn the_clock_is_asked_for_and_read_on_connecting() {
        let frames = LiveSession::opening_frames(false);
        assert!(frames[1].contains("\"ExtrapolatedClock\""));
        let mut live = LiveSession::new(64 * 1024);
        live.receive(format!("{{}}{RS}").as_bytes());
        let received = live.receive(
            completion(
                r#"{"ExtrapolatedClock":{"Utc":"2026-10-09T09:00:00Z","Remaining":"00:10:00","Extrapolating":false,"_kf":true}}"#,
            )
            .as_bytes(),
        );
        assert_eq!(received.timing.len(), 1);
    }

    #[test]
    fn short_names_are_passed_on_for_the_race_log() {
        let mut live = LiveSession::new(64 * 1024);
        live.receive(format!("{{}}{RS}").as_bytes());
        let received = live.receive(
            completion(r#"{"DriverList":{"1":{"Tla":"VER","TeamColour":"4781D7"},"_kf":true}}"#)
                .as_bytes(),
        );
        assert_eq!(received.names, [(1, "VER".to_owned())]);
    }
}
