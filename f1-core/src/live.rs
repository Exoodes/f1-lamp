//! One connection to F1's live feed, without the networking: bytes from the
//! WebSocket go in, race events for the lamp come out.
//!
//! The answer to `Subscribe` holds the current state of every stream. It is
//! applied silently, the way a replay jumps: connecting mid-race must not
//! replay the start lights or a chequered flag. Afterwards only the current
//! flag is sent, so the lamp shows it at once.

use serde_json::value::RawValue;

use crate::{
    input::RaceEvent,
    signalr::{self, Message, Splitter},
    timeline::Stream,
    track_state::{FeedMessage, TrackState},
    tracker::Tracker,
};

/// The order the initial state is applied in: what the session is and who
/// drives first, its status last, so the tracker knows everything it needs.
/// Race control is left out on purpose: late in a race it holds hundreds of
/// messages, which would cost tens of KB to parse, and all they would add is
/// which sectors have a double yellow at the moment of connecting.
const INITIAL_ORDER: [Stream; 5] = [
    Stream::SessionInfo,
    Stream::DriverList,
    Stream::TopThree,
    Stream::TrackStatus,
    Stream::SessionStatus,
];

/// Also subscribed to, so a quiet session still shows the connection is alive.
const HEARTBEAT: &str = "Heartbeat";

/// Invocation ids of the `Subscribe` calls. Only the first one's answer
/// is used; the others answer with history.
const STATE_CALL: u32 = 1;
const RACE_CONTROL_CALL: u32 = 2;
const DRIVER_EVENTS_CALL: u32 = 3;
const ACCOUNT_CALL: u32 = 4;

/// The streams whose current state is applied on connecting.
const STATE_STREAMS: [Stream; 5] = [
    Stream::SessionInfo,
    Stream::TrackStatus,
    Stream::SessionStatus,
    Stream::DriverList,
    Stream::TopThree,
];
/// Public streams with driver events: pit stops and fastest laps.
const DRIVER_EVENT_STREAMS: [Stream; 2] = [Stream::PitLane, Stream::TimingStats];
/// Streams that only answer with an F1TV token: overtakes.
const ACCOUNT_STREAMS: [Stream; 1] = [Stream::Overtakes];

/// What one batch of received bytes produced.
#[derive(Debug, Default, PartialEq)]
pub struct Received {
    pub events: Vec<RaceEvent>,
    /// Things worth a log line: frames that didn't parse, refused calls.
    pub problems: Vec<String>,
    /// The server answered `Subscribe` in this batch.
    pub subscribed: bool,
    /// The server is closing the connection, with its reason.
    pub closed: Option<String>,
    /// The session key from SessionInfo, once known.
    pub session_key: Option<u32>,
}

pub struct LiveSession {
    splitter: Splitter,
    state: TrackState,
    tracker: Tracker,
    session_key: Option<u32>,
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
        }
    }

    /// What to send once the WebSocket is open: the handshake, then the
    /// subscriptions. `with_token`: the connection carries an F1TV token, so
    /// the account-only streams are asked for too.
    ///
    /// Only the first `Subscribe` answer is used. Every stream whose answer
    /// is just history gets a call of its own: race control's is 44 KB after
    /// a race, overtakes' 30 KB. As separate frames, those larger than
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
                Err(e) => out.problems.push(e.to_string()),
            }
        }
        out.session_key = self.session_key;
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
            // The answer to the race-control call is only history: ignored.
            Message::Completion {
                invocation_id,
                result: Some(result),
                ..
            } if invocation_id.as_deref() == Some(&STATE_CALL.to_string()) => {
                self.initial_state(result, out);
                out.subscribed = true;
            }
            Message::Invocation { .. } => {
                if let Some(update) = msg.feed_update() {
                    if let Some(stream) = Stream::from_name(&update.stream) {
                        self.update(stream, update.data.get(), out);
                    }
                }
            }
            Message::Close { error } => {
                out.closed = Some(error.clone().unwrap_or_else(|| "no reason".to_owned()));
            }
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

    fn update(&mut self, stream: Stream, json: &str, out: &mut Received) {
        if !stream.is_relevant(json) {
            return;
        }
        match stream.parse(json) {
            Ok(messages) => {
                for msg in messages {
                    self.note(&msg);
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
    fn opening_frames_are_handshake_then_three_subscribes_without_token() {
        let frames = LiveSession::opening_frames(false);
        let [handshake, state, race_control, driver_events] = &frames[..] else {
            panic!("{} frames", frames.len())
        };
        assert!(
            driver_events.contains("TimingStats")
                && driver_events.contains("PitLaneTimeCollection")
        );
        assert!(!frames.iter().any(|f| f.contains("OvertakeSeries")));
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
        let mut problems = 0;
        for piece in history.as_bytes().chunks(100) {
            problems += live.receive(piece).problems.len();
        }
        assert_eq!(problems, 1);
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
                }
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
    fn with_a_token_overtakes_get_a_fourth_subscribe() {
        let frames = LiveSession::opening_frames(true);
        assert_eq!(frames.len(), 5);
        assert!(frames[4].contains("[[\"OvertakeSeries\"]]"));
        assert!(frames[4].contains("\"invocationId\":\"4\""));
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
}
