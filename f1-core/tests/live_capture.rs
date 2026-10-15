//! A capture from the live feed, saved by f1-probe: the `Subscribe` answer
//! after the 2026 Bahrain GP race. Unlike the archive, live data carries
//! `"_kf": true` in every stream.

use f1_core::{
    color::Rgb,
    feed::{update_team_colours, FeedMessage, Stream},
    input::{RaceEvent, TrackFlag},
    live::{LiveSession, MAX_FRAME},
    timeline::Timeline,
};
use std::collections::HashMap;

const DIR: &str = "data/live/2026-bahrain-race-subscribe";

const FILES: [(Stream, &str); 6] = [
    (
        Stream::SessionInfo,
        include_str!("data/live/2026-bahrain-race-subscribe/SessionInfo.jsonStream"),
    ),
    (
        Stream::TrackStatus,
        include_str!("data/live/2026-bahrain-race-subscribe/TrackStatus.jsonStream"),
    ),
    (
        Stream::SessionStatus,
        include_str!("data/live/2026-bahrain-race-subscribe/SessionStatus.jsonStream"),
    ),
    (
        Stream::RaceControl,
        include_str!("data/live/2026-bahrain-race-subscribe/RaceControlMessages.jsonStream"),
    ),
    (
        Stream::DriverList,
        include_str!("data/live/2026-bahrain-race-subscribe/DriverList.jsonStream"),
    ),
    (
        Stream::TopThree,
        include_str!("data/live/2026-bahrain-race-subscribe/TopThree.jsonStream"),
    ),
];

fn messages() -> Vec<FeedMessage> {
    Timeline::new(&FILES)
        .map(|item| item.unwrap_or_else(|e| panic!("{DIR}: {e}")).1)
        .collect()
}

#[test]
fn live_data_carries_the_kf_marker() {
    for (stream, text) in FILES {
        assert!(text.contains("\"_kf\":true"), "{stream:?}");
    }
}

#[test]
fn every_stream_of_the_capture_parses() {
    let msgs = messages();
    let count = |f: fn(&FeedMessage) -> bool| msgs.iter().filter(|m| f(m)).count();
    assert_eq!(count(|m| matches!(m, FeedMessage::SessionInfo(_))), 1);
    assert_eq!(count(|m| matches!(m, FeedMessage::Track(_))), 1);
    assert_eq!(count(|m| matches!(m, FeedMessage::Session(_))), 1);
    assert_eq!(count(|m| matches!(m, FeedMessage::RaceControl(_))), 327);
    assert_eq!(count(|m| matches!(m, FeedMessage::DriverList(_))), 1);
    assert_eq!(count(|m| matches!(m, FeedMessage::TopThree(_))), 1);
}

#[test]
fn driver_list_gives_22_team_colours_despite_kf() {
    let mut colours: HashMap<u8, Rgb> = HashMap::new();
    for msg in messages() {
        if let FeedMessage::DriverList(d) = msg {
            update_team_colours(&mut colours, &d);
        }
    }
    assert_eq!(colours.len(), 22);
    assert_eq!(colours[&3], "#4781d7".parse().unwrap());
}

#[test]
fn race_control_holds_the_whole_race_with_one_chequered_flag() {
    let flags: Vec<String> = messages()
        .into_iter()
        .filter_map(|m| match m {
            FeedMessage::RaceControl(rc) => rc.flag,
            _ => None,
        })
        .collect();
    assert_eq!(flags.iter().filter(|f| *f == "CHEQUERED").count(), 1);
    // Not the last one: a black-and-white flag (a warning) came after it.
    assert_eq!(flags.last().map(String::as_str), Some("BLACK AND WHITE"));
}

#[test]
fn top_three_leader_is_verstappen() {
    let leader = messages().into_iter().find_map(|m| match m {
        FeedMessage::TopThree(t) => t.lines?.indexed().first()?.1.racing_number.clone(),
        _ => None,
    });
    assert_eq!(leader.as_deref(), Some("3"));
}

/// Streams the lamp has asked for only since the Bahrain capture was made:
/// their state at the end of the 2026 Singapore GP race, merged from the
/// probe's capture of it. SessionData then holds every lap's start, its
/// largest size.
const ADDED: [(Stream, &str); 3] = [
    (
        Stream::SessionData,
        include_str!("data/live/2026-singapore-race-end/SessionData.jsonStream"),
    ),
    (
        Stream::LapCount,
        include_str!("data/live/2026-singapore-race-end/LapCount.jsonStream"),
    ),
    (
        Stream::Clock,
        include_str!("data/live/2026-singapore-race-end/ExtrapolatedClock.jsonStream"),
    ),
];

/// The streams of the captures by name, Heartbeat included.
fn captured(name: &str) -> Option<&'static str> {
    match name {
        "Heartbeat" => Some(include_str!(
            "data/live/2026-bahrain-race-subscribe/Heartbeat.jsonStream"
        )),
        _ => FILES
            .iter()
            .chain(&ADDED)
            .find(|(stream, _)| stream.name() == name)
            .map(|(_, text)| *text),
    }
}

/// The capture put back together as the server's answer to the `Subscribe`
/// call `id` for `names`. A stream the capture lacks is left out, as the
/// server leaves out one without data.
fn answer(id: &str, names: &[&str]) -> String {
    let streams: Vec<String> = names
        .iter()
        .filter_map(|name| {
            let json = captured(name)?.trim_start_matches('\u{feff}')[12..].trim_end();
            Some(format!("\"{name}\":{json}"))
        })
        .collect();
    format!(
        "{{\"type\":3,\"invocationId\":\"{id}\",\"result\":{{{}}}}}\u{1e}",
        streams.join(",")
    )
}

/// A `Subscribe` call's invocation id and the streams it asks for.
fn asks_for(frame: &str) -> (String, Vec<String>) {
    let call: serde_json::Value = serde_json::from_str(frame.trim_end_matches('\u{1e}')).unwrap();
    let names = call["arguments"][0]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect();
    (call["invocationId"].as_str().unwrap().to_owned(), names)
}

/// The answers to the lamp's own first two `Subscribe` calls, built from what
/// they ask for: the state it uses, and the race control history.
fn the_lamps_answers() -> (String, String) {
    let call = |frame: &str| {
        let (id, names) = asks_for(frame);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        answer(&id, &names)
    };
    let frames = LiveSession::opening_frames(false);
    let race_control = frames
        .iter()
        .find(|f| f.contains("[[\"RaceControlMessages\"]]"))
        .unwrap();
    (call(&frames[1]), call(race_control))
}

/// Feeds `answer` to `live` in the pieces the device's WebSocket client hands
/// over (its buffer is 4 KB): the problems reported, and whether it
/// subscribed.
fn receive_as_on_the_device(live: &mut LiveSession, answer: &str) -> (Vec<String>, bool) {
    live.expect(answer.len());
    let mut problems = Vec::new();
    let mut subscribed = false;
    for piece in answer.as_bytes().chunks(4096) {
        let r = live.receive(piece);
        problems.extend(r.problems);
        subscribed |= r.subscribed;
    }
    (problems, subscribed)
}

#[test]
fn the_state_the_lamp_asks_for_fits_one_frame_on_the_device() {
    let (state, _) = the_lamps_answers();
    // Every stream the call asks for has captured data, so the answer is as
    // large as after a real race.
    let (_, names) = asks_for(&LiveSession::opening_frames(false)[1]);
    for name in names {
        assert!(captured(&name).is_some(), "no captured {name}");
    }
    assert!(state.len() < MAX_FRAME, "{} bytes", state.len());
    let mut live = LiveSession::new(MAX_FRAME);
    let (problems, subscribed) = receive_as_on_the_device(&mut live, &state);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(subscribed);
}

#[test]
fn the_race_control_history_is_skipped_on_the_device_and_updates_still_arrive() {
    let (state, history) = the_lamps_answers();
    assert!(history.len() > MAX_FRAME, "{} bytes", history.len());
    let mut live = LiveSession::new(MAX_FRAME);
    receive_as_on_the_device(&mut live, &state);
    let (problems, _) = receive_as_on_the_device(&mut live, &history);
    assert_eq!(
        problems,
        [format!("frame larger than {MAX_FRAME} bytes, dropped")]
    );
    let update = "{\"type\":1,\"target\":\"feed\",\"arguments\":[\"SessionStatus\",{\"Status\":\"Aborted\"},\"t\"]}\u{1e}";
    assert_eq!(
        live.receive(update.as_bytes()).events,
        [RaceEvent::TrackFlag(TrackFlag::Red)]
    );
}

#[test]
fn joining_after_the_race_applies_57_kb_of_state_silently() {
    let names: Vec<&str> = FILES.iter().map(|(stream, _)| stream.name()).collect();
    let answer = answer("1", &names);
    assert!(answer.len() > 50_000);

    let mut live = LiveSession::new(96 * 1024);
    live.expect(answer.len());
    let mut subscribed = false;
    for piece in answer.as_bytes().chunks(1024) {
        let r = live.receive(piece);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        // The session has ended: no flag, and no chequered flag or winner
        // from the race control history.
        assert!(r.events.is_empty(), "{:?}", r.events);
        subscribed |= r.subscribed;
    }
    assert!(subscribed);
}
