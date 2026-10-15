//! The race log on the web page: every race event in words, in step with
//! the TV, marked when the lamp didn't show it (switched off, a driver not
//! followed). The controller hands over a [`LogItem`] when an event's TV
//! time comes; [`text`] says it with the drivers' short names, and
//! [`RaceLog`] keeps the latest lines for the page.

use std::collections::VecDeque;

use serde::Serialize;

use crate::{
    color::Rgb,
    input::{RaceEvent, TrackFlag},
    theme::PURPLE,
};

/// Lines kept: a race with every pit stop and flag fits; with every
/// driver's overtakes followed, only the latest.
pub const LINES: usize = 120;

/// What the controller logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogItem {
    /// A race event the lamp shows.
    Event(RaceEvent),
    /// A race event the lamp doesn't show: switched off, a driver nobody
    /// follows, or no room left for its effect. Dimmed on the page.
    NotShown(RaceEvent),
    SessionStarted,
    SessionEnded,
}

/// Whether the lamp showed it.
pub fn shown(item: LogItem) -> bool {
    !matches!(item, LogItem::NotShown(_))
}

/// The line for `item`, with `name` giving a driver's short name; `None`
/// for what isn't worth a line (a flag being cleared).
pub fn text(item: LogItem, name: &dyn Fn(u8) -> String) -> Option<String> {
    let event = match item {
        LogItem::SessionStarted => return Some("Session started".to_owned()),
        LogItem::SessionEnded => return Some("Session ended".to_owned()),
        LogItem::Event(event) | LogItem::NotShown(event) => event,
    };
    Some(match event {
        RaceEvent::TrackFlag(flag) => match flag {
            TrackFlag::Green => "Green flag",
            TrackFlag::Yellow => "Yellow flag",
            TrackFlag::DoubleYellow => "Double yellow flag",
            TrackFlag::SafetyCar => "Safety car",
            TrackFlag::VirtualSafetyCar => "Virtual safety car",
            TrackFlag::VscEnding => "Virtual safety car ending",
            TrackFlag::Red => "Red flag",
        }
        .to_owned(),
        RaceEvent::FlagCleared => return None,
        RaceEvent::StartLights => "Start lights".to_owned(),
        RaceEvent::ChequeredFlag => "Chequered flag".to_owned(),
        RaceEvent::LastLap => "Last lap".to_owned(),
        RaceEvent::FastestLap { driver, .. } => format!("Fastest lap: {}", name(driver)),
        RaceEvent::PitStop { driver, .. } => format!("{} in the pits", name(driver)),
        RaceEvent::Overtake { driver, .. } => format!("{} overtakes", name(driver)),
        // The race's winner, or pole after qualifying.
        RaceEvent::Winner { driver, .. } => format!("P1: {}", name(driver)),
        RaceEvent::KnockedOut { driver, .. } => format!("{} knocked out", name(driver)),
        RaceEvent::Penalty { driver, .. } => format!("Penalty: {}", name(driver)),
    })
}

/// The colour of the dot next to a line: the flag's, or the team's.
pub fn color(item: LogItem) -> Option<Rgb> {
    let (LogItem::Event(event) | LogItem::NotShown(event)) = item else {
        return None;
    };
    match event {
        RaceEvent::TrackFlag(TrackFlag::Green) => Some(Rgb::GREEN),
        RaceEvent::TrackFlag(TrackFlag::Red) => Some(Rgb::RED),
        RaceEvent::TrackFlag(_) => Some(Rgb::YELLOW),
        RaceEvent::ChequeredFlag | RaceEvent::LastLap => Some(Rgb::WHITE),
        RaceEvent::FastestLap { .. } => Some(PURPLE),
        RaceEvent::PitStop { team_color, .. }
        | RaceEvent::Overtake { team_color, .. }
        | RaceEvent::Winner { team_color, .. }
        | RaceEvent::KnockedOut { team_color, .. }
        | RaceEvent::Penalty { team_color, .. } => Some(team_color),
        RaceEvent::FlagCleared | RaceEvent::StartLights => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogLine {
    pub seq: u64,
    /// Local time of day when the lamp showed it: `14:03:21`.
    pub time: String,
    pub text: String,
    pub color: Option<Rgb>,
    /// False for an event the lamp didn't show.
    pub shown: bool,
}

/// One answer to the page.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RaceLogPage {
    pub lines: Vec<LogLine>,
    /// The number the next line will get: ask from it next time.
    pub next: u64,
}

#[derive(Debug)]
pub struct RaceLog {
    lines: VecDeque<LogLine>,
    next: u64,
}

impl RaceLog {
    pub const fn new() -> Self {
        RaceLog {
            lines: VecDeque::new(),
            next: 0,
        }
    }

    pub fn push(&mut self, time: String, text: String, color: Option<Rgb>, shown: bool) {
        if self.lines.len() == LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(LogLine {
            seq: self.next,
            time,
            text,
            color,
            shown,
        });
        self.next += 1;
    }

    /// The lines numbered `from` on, where `from` is the `next` of the
    /// page's last answer: none when nothing is new. Every line kept for a
    /// page that just opened (`None`), or one whose number is from before a
    /// restart (beyond `next`).
    pub fn since(&self, from: Option<u64>) -> RaceLogPage {
        let from = from.filter(|&f| f <= self.next);
        RaceLogPage {
            lines: self
                .lines
                .iter()
                .filter(|l| from.is_none_or(|f| l.seq >= f))
                .cloned()
                .collect(),
            next: self.next,
        }
    }
}

impl Default for RaceLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: Rgb = Rgb::new(0x47, 0x81, 0xD7);

    fn names(n: u8) -> String {
        if n == 1 {
            "VER".to_owned()
        } else {
            format!("#{n}")
        }
    }

    fn say(event: RaceEvent) -> Option<String> {
        text(LogItem::Event(event), &names)
    }

    #[test]
    fn an_event_not_shown_reads_and_colours_the_same() {
        let stop = RaceEvent::PitStop {
            driver: 44,
            team_color: Rgb::new(0, 210, 190),
        };
        let name = |_: u8| "HAM".to_owned();
        let (played, skipped) = (LogItem::Event(stop), LogItem::NotShown(stop));
        assert_eq!(text(skipped, &name), text(played, &name));
        assert_eq!(color(skipped), color(played));
        assert!(shown(played) && !shown(skipped));
    }

    #[test]
    fn driver_events_name_the_driver() {
        let ver = |f: fn(u8, Rgb) -> RaceEvent| say(f(1, TEAM)).unwrap();
        assert_eq!(
            ver(|driver, team_color| RaceEvent::FastestLap { driver, team_color }),
            "Fastest lap: VER"
        );
        assert_eq!(
            ver(|driver, team_color| RaceEvent::PitStop { driver, team_color }),
            "VER in the pits"
        );
        assert_eq!(
            ver(|driver, team_color| RaceEvent::Penalty { driver, team_color }),
            "Penalty: VER"
        );
        assert_eq!(
            ver(|driver, team_color| RaceEvent::Winner { driver, team_color }),
            "P1: VER"
        );
        assert_eq!(
            say(RaceEvent::Overtake {
                driver: 44,
                team_color: TEAM
            })
            .unwrap(),
            "#44 overtakes"
        );
    }

    #[test]
    fn flags_and_race_moments_read_plainly() {
        assert_eq!(
            say(RaceEvent::TrackFlag(TrackFlag::Red)).unwrap(),
            "Red flag"
        );
        assert_eq!(say(RaceEvent::ChequeredFlag).unwrap(), "Chequered flag");
        assert_eq!(say(RaceEvent::LastLap).unwrap(), "Last lap");
        assert_eq!(say(RaceEvent::FlagCleared), None);
        assert_eq!(
            text(LogItem::SessionStarted, &names).unwrap(),
            "Session started"
        );
    }

    #[test]
    fn dots_have_the_flag_or_team_colour() {
        assert_eq!(
            color(LogItem::Event(RaceEvent::TrackFlag(TrackFlag::SafetyCar))),
            Some(Rgb::YELLOW)
        );
        assert_eq!(
            color(LogItem::Event(RaceEvent::PitStop {
                driver: 1,
                team_color: TEAM
            })),
            Some(TEAM)
        );
        assert_eq!(color(LogItem::SessionEnded), None);
    }

    fn log_with(n: u64) -> RaceLog {
        let mut log = RaceLog::new();
        for i in 0..n {
            log.push(format!("14:00:{i:02}"), format!("line {i}"), None, true);
        }
        log
    }

    #[test]
    fn a_page_that_just_opened_gets_every_line() {
        let page = log_with(3).since(None);
        assert_eq!(page.lines.len(), 3);
        assert_eq!(page.next, 3);
    }

    #[test]
    fn later_asks_get_only_new_lines() {
        let page = log_with(5).since(Some(3));
        let seqs: Vec<u64> = page.lines.iter().map(|l| l.seq).collect();
        assert_eq!(seqs, [3, 4]);
    }

    #[test]
    fn nothing_new_gets_no_lines() {
        // Race day 2026-10-11: this sent every line again, every 2 s.
        let log = log_with(1);
        let first = log.since(None);
        assert_eq!(first.lines.len(), 1);
        assert!(log.since(Some(first.next)).lines.is_empty());
    }

    #[test]
    fn the_next_line_is_not_skipped() {
        let mut log = log_with(1);
        let next = log.since(None).next;
        log.push("14:01:00".to_owned(), "Green flag".to_owned(), None, true);
        let page = log.since(Some(next));
        assert_eq!(page.lines.len(), 1);
        assert_eq!(page.lines[0].text, "Green flag");
    }

    #[test]
    fn a_number_from_before_a_restart_gets_every_line() {
        assert_eq!(log_with(2).since(Some(500)).lines.len(), 2);
    }

    #[test]
    fn only_the_latest_lines_are_kept() {
        let page = log_with(LINES as u64 + 10).since(None);
        assert_eq!(page.lines.len(), LINES);
        assert_eq!(page.lines[0].seq, 10);
    }
}
