//! Parses the saved OpenF1 result and drivers of the 2026 Italian GP race
//! (`/v1/session_result` and `/v1/drivers`, session 11361).

use f1_core::{
    color::Rgb,
    openf1::{team_colours, winner, DriverDto, ResultDto},
};

const RESULT: &str = include_str!("data/result.json");
const DRIVERS: &str = include_str!("data/drivers.json");

fn results() -> Vec<ResultDto> {
    serde_json::from_str(RESULT).unwrap()
}

fn drivers() -> Vec<DriverDto> {
    serde_json::from_str(DRIVERS).unwrap()
}

#[test]
fn result_fixture_parses_every_driver() {
    assert_eq!(results().len(), 22);
}

#[test]
fn fixture_race_winner_is_12() {
    assert_eq!(winner(&results()), Some(12));
}

#[test]
fn fixture_winner_drives_a_mercedes_teal() {
    let colours = team_colours(drivers());
    assert_eq!(colours.get(&12), Some(&Rgb::new(0x00, 0xD7, 0xB6)));
}

#[test]
fn fixture_has_a_colour_for_every_driver() {
    assert_eq!(team_colours(drivers()).len(), 22);
}

#[test]
fn unclassified_drivers_in_the_fixture_have_no_position() {
    let unclassified = results().iter().filter(|r| r.position.is_none()).count();
    assert_eq!(unclassified, 3);
}
