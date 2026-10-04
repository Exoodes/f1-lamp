//! A capture from the live feed, saved by f1-probe: the `Subscribe` answer
//! after the 2026 Bahrain GP race. Unlike the archive, live data carries
//! `"_kf": true` in every stream.

use f1_core::{
    color::Rgb,
    feed::update_team_colours,
    timeline::{Stream, Timeline},
    track_state::FeedMessage,
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
