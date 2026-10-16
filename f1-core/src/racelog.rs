//! The race log on the web page: every race event in words, in step with
//! the TV, marked when the lamp didn't show it (switched off, a driver not
//! followed). The controller hands over a [`LogItem`] when an event's TV
//! time comes; [`RaceLog`] keeps the latest with their time of day, and
//! writes the lines when the page asks, with the drivers' short names
//! ([`text`]).

use std::collections::VecDeque;

use serde::Serialize;

use crate::{
    color::Rgb,
    input::{RaceEvent, TrackFlag},
    theme::PURPLE,
};

/// Lines kept: a whole race, every driver's overtakes too (Monza 2026: 296
/// overtakes, about 370 lines). An event and its time take 16 bytes, so
/// the log takes 8 KB at most.
pub const LINES: usize = 512;

/// The most lines in one answer to the page, about 5 KB of JSON: a page that
/// just opened gets the newest, one that fell behind asks again.
pub const PAGE_LINES: usize = 50;

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

/// Whether `item` gets a line: a flag being cleared doesn't.
pub fn has_line(item: LogItem) -> bool {
    !matches!(
        item,
        LogItem::Event(RaceEvent::FlagCleared) | LogItem::NotShown(RaceEvent::FlagCleared)
    )
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
        RaceEvent::Retired { driver, .. } => format!("{} retired", name(driver)),
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
        | RaceEvent::Penalty { team_color, .. }
        | RaceEvent::Retired { team_color, .. } => Some(team_color),
        RaceEvent::FlagCleared | RaceEvent::StartLights => None,
    }
}

/// A second of the day as `14:03:21`; `--:--:--` when the clock wasn't set.
pub fn time_of_day(second: Option<u32>) -> String {
    match second {
        Some(s) => format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
        None => "--:--:--".to_owned(),
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
    /// The number to ask from next time.
    pub next: u64,
    /// More lines are waiting after these: ask again at once.
    pub more: bool,
}

/// What a line is made from when the page asks.
#[derive(Clone, Copy, Debug)]
struct Entry {
    item: LogItem,
    /// Local seconds since midnight.
    at: Option<u32>,
}

#[derive(Debug)]
pub struct RaceLog {
    entries: VecDeque<Entry>,
    /// The number the next line will get; the first kept is `next - len`.
    next: u64,
}

impl RaceLog {
    pub const fn new() -> Self {
        RaceLog {
            entries: VecDeque::new(),
            next: 0,
        }
    }

    /// Logs `item`, shown at `at` (local seconds since midnight).
    pub fn push(&mut self, item: LogItem, at: Option<u32>) {
        if !has_line(item) {
            return;
        }
        if self.entries.capacity() == 0 {
            // All at once while the heap is fresh, not doubling later.
            self.entries.reserve_exact(LINES);
        }
        if self.entries.len() == LINES {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry { item, at });
        self.next += 1;
    }

    /// At most [`PAGE_LINES`] lines numbered `from` on, where `from` is the
    /// `next` of the page's last answer: none when nothing is new. The
    /// newest lines for a page that just opened (`None`), or one whose
    /// number is from before a restart (beyond `next`). `name` gives a
    /// driver's short name.
    pub fn since(&self, from: Option<u64>, name: &dyn Fn(u8) -> String) -> RaceLogPage {
        let first = self.next - self.entries.len() as u64;
        let start = match from.filter(|&f| f <= self.next) {
            Some(f) => f.max(first),
            None => self.next.saturating_sub(PAGE_LINES as u64).max(first),
        };
        let end = self.next.min(start + PAGE_LINES as u64);
        let lines = (start..end)
            .filter_map(|seq| {
                let entry = self.entries[usize::try_from(seq - first).ok()?];
                Some(LogLine {
                    seq,
                    time: time_of_day(entry.at),
                    text: text(entry.item, name)?,
                    color: color(entry.item),
                    shown: shown(entry.item),
                })
            })
            .collect();
        RaceLogPage {
            lines,
            next: end,
            more: end < self.next,
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
            ver(|driver, team_color| RaceEvent::Retired { driver, team_color }),
            "VER retired"
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
        assert!(!has_line(LogItem::Event(RaceEvent::FlagCleared)));
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
            log.push(LogItem::Event(RaceEvent::LastLap), Some(50_400 + i as u32));
        }
        log
    }

    fn page(log: &RaceLog, from: Option<u64>) -> RaceLogPage {
        log.since(from, &names)
    }

    fn seqs(page: &RaceLogPage) -> Vec<u64> {
        page.lines.iter().map(|l| l.seq).collect()
    }

    #[test]
    fn a_line_is_written_when_the_page_asks() {
        let mut log = RaceLog::new();
        let overtake = RaceEvent::Overtake {
            driver: 1,
            team_color: TEAM,
        };
        log.push(LogItem::NotShown(overtake), Some(14 * 3600 + 3 * 60 + 21));
        assert_eq!(
            page(&log, None).lines,
            [LogLine {
                seq: 0,
                time: "14:03:21".to_owned(),
                text: "VER overtakes".to_owned(),
                color: Some(TEAM),
                shown: false,
            }]
        );
    }

    #[test]
    fn a_line_logged_before_the_clock_was_set_has_no_time() {
        assert_eq!(time_of_day(None), "--:--:--");
        assert_eq!(time_of_day(Some(9)), "00:00:09");
    }

    #[test]
    fn a_cleared_flag_gets_no_line_or_number() {
        let mut log = log_with(1);
        log.push(LogItem::Event(RaceEvent::FlagCleared), Some(0));
        assert_eq!(page(&log, None).next, 1);
    }

    #[test]
    fn an_entry_takes_sixteen_bytes() {
        assert!(std::mem::size_of::<Entry>() <= 16);
    }

    #[test]
    fn a_page_that_just_opened_gets_every_line_of_a_short_log() {
        let page = page(&log_with(3), None);
        assert_eq!(seqs(&page), [0, 1, 2]);
        assert_eq!((page.next, page.more), (3, false));
    }

    #[test]
    fn a_page_that_just_opened_gets_only_the_newest_lines() {
        let n = PAGE_LINES as u64 + 30;
        let page = page(&log_with(n), None);
        assert_eq!(page.lines.len(), PAGE_LINES);
        assert_eq!(page.lines[0].seq, 30);
        assert_eq!((page.next, page.more), (n, false));
    }

    #[test]
    fn later_asks_get_only_new_lines() {
        let page = page(&log_with(5), Some(3));
        assert_eq!(seqs(&page), [3, 4]);
    }

    #[test]
    fn nothing_new_gets_no_lines() {
        // Race day 2026-10-11: this sent every line again, every 2 s.
        let log = log_with(1);
        let first = page(&log, None);
        assert_eq!(first.lines.len(), 1);
        assert!(page(&log, Some(first.next)).lines.is_empty());
    }

    #[test]
    fn the_next_line_is_not_skipped() {
        let mut log = log_with(1);
        let next = page(&log, None).next;
        log.push(LogItem::Event(RaceEvent::TrackFlag(TrackFlag::Green)), None);
        let page = page(&log, Some(next));
        assert_eq!(page.lines.len(), 1);
        assert_eq!(page.lines[0].text, "Green flag");
    }

    #[test]
    fn a_page_that_fell_behind_catches_up_in_steps() {
        let log = log_with(PAGE_LINES as u64 + 10);
        let first = page(&log, Some(0));
        assert_eq!(first.lines.len(), PAGE_LINES);
        assert!(first.more);
        let rest = page(&log, Some(first.next));
        assert_eq!(rest.lines.len(), 10);
        assert!(!rest.more);
    }

    #[test]
    fn a_number_from_before_a_restart_gets_the_newest_lines() {
        assert_eq!(page(&log_with(2), Some(500)).lines.len(), 2);
    }

    #[test]
    fn only_the_latest_lines_are_kept() {
        let log = log_with(LINES as u64 + 10);
        // Asking from a line no longer kept starts at the oldest kept.
        assert_eq!(page(&log, Some(0)).lines[0].seq, 10);
        assert_eq!(log.entries.len(), LINES);
        assert_eq!(log.entries.capacity(), LINES);
    }
}
