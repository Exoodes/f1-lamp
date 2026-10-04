//! Splits every line of the archived 2026 Italian GP race streams
//! (`livetiming.formula1.com/static/2026/...`) into offset and JSON.

use std::time::Duration;

use f1_core::stream::{parse_line, Line};
use serde::de::IgnoredAny;

const TRACK_STATUS: &str = include_str!("data/2026-italy/race/TrackStatus.jsonStream");
const SESSION_STATUS: &str = include_str!("data/2026-italy/race/SessionStatus.jsonStream");
const RACE_CONTROL: &str = include_str!("data/2026-italy/race/RaceControlMessages.jsonStream");
const DRIVER_LIST: &str = include_str!("data/2026-italy/race/DriverList.jsonStream");
const TOP_THREE: &str = include_str!("data/2026-italy/race/TopThree.jsonStream");

const ALL: [(&str, &str); 5] = [
    ("TrackStatus", TRACK_STATUS),
    ("SessionStatus", SESSION_STATUS),
    ("RaceControlMessages", RACE_CONTROL),
    ("DriverList", DRIVER_LIST),
    ("TopThree", TOP_THREE),
];

/// Every line of a file, parsed. Panics with the file name and line number.
fn lines<'a>(name: &str, file: &'a str) -> Vec<Line<'a>> {
    file.lines()
        .enumerate()
        .map(|(i, raw)| parse_line(raw).unwrap_or_else(|e| panic!("{name} line {}: {e}", i + 1)))
        .collect()
}

fn hms(h: u64, m: u64, s: u64, ms: u64) -> Duration {
    Duration::from_millis(((h * 60 + m) * 60 + s) * 1000 + ms)
}

#[test]
fn fixtures_start_with_a_byte_order_mark() {
    for (name, file) in ALL {
        assert!(file.starts_with('\u{feff}'), "{name} has no BOM");
    }
}

#[test]
fn every_line_of_every_fixture_has_an_offset_and_json() {
    for (name, file) in ALL {
        for line in lines(name, file) {
            assert!(line.json.starts_with('{'), "{name}: {}", line.json);
            assert!(line.json.ends_with('}'), "{name}: {}", line.json);
        }
    }
}

#[test]
fn every_json_part_is_valid_json() {
    // IgnoredAny checks the syntax without building a tree in memory.
    for (name, file) in ALL {
        for (i, line) in lines(name, file).into_iter().enumerate() {
            serde_json::from_str::<IgnoredAny>(line.json)
                .unwrap_or_else(|e| panic!("{name} line {}: {e}", i + 1));
        }
    }
}

#[test]
fn offsets_within_each_fixture_never_go_backwards() {
    for (name, file) in ALL {
        let offsets: Vec<Duration> = lines(name, file).iter().map(|l| l.offset).collect();
        for (i, pair) in offsets.windows(2).enumerate() {
            assert!(
                pair[0] <= pair[1],
                "{name} line {}: {:?} comes after {:?}",
                i + 2,
                pair[1],
                pair[0]
            );
        }
    }
}

#[test]
fn fixtures_have_the_expected_number_of_lines() {
    let counts: Vec<usize> = ALL
        .iter()
        .map(|(name, file)| lines(name, file).len())
        .collect();
    assert_eq!(counts, [16, 7, 154, 149, 56]);
}

#[test]
fn first_track_status_line_is_at_zero_despite_the_bom() {
    let first = lines("TrackStatus", TRACK_STATUS)[0];
    assert_eq!(first.offset, Duration::ZERO);
    assert_eq!(first.json, r#"{"Status":"2","Message":"Yellow"}"#);
}

#[test]
fn session_finishes_at_2_48_11() {
    let finished = lines("SessionStatus", SESSION_STATUS)
        .into_iter()
        .find(|l| l.json.contains(r#""Status":"Finished""#))
        .unwrap();
    assert_eq!(finished.offset, hms(2, 48, 11, 236));
}

#[test]
fn red_flag_track_status_comes_right_after_the_safety_car() {
    let track = lines("TrackStatus", TRACK_STATUS);
    let sc = track
        .iter()
        .position(|l| l.json.contains("SCDeployed"))
        .unwrap();
    assert!(track[sc + 1].json.contains("Red"));
    assert_eq!(track[sc + 1].offset, hms(1, 1, 7, 837));
}
