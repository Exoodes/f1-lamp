//! The last few log lines, for the web page's live log.
//!
//! Nothing is kept for later: the page starts empty and asks every second for
//! the lines after the last one it has. The buffer only has to bridge that
//! second, so it holds a few dozen lines; older ones fall out. Every line has
//! a sequence number, so the page can tell when it missed some.

use std::collections::VecDeque;

use serde::Serialize;

#[derive(Debug)]
pub struct LogBuffer {
    lines: VecDeque<String>,
    /// Sequence number of `lines[0]`.
    first: u64,
    capacity: usize,
    max_line: usize,
}

/// One answer to the page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogPage {
    pub lines: Vec<String>,
    /// Ask for lines after this next time.
    pub next: u64,
    /// Lines that fell out of the buffer before the page asked.
    pub missed: u64,
}

impl LogBuffer {
    /// Holds `capacity` lines of at most `max_line` bytes each.
    pub const fn new(capacity: usize, max_line: usize) -> Self {
        LogBuffer {
            lines: VecDeque::new(),
            first: 0,
            capacity,
            max_line,
        }
    }

    pub fn push(&mut self, mut line: String) {
        if line.len() > self.max_line {
            let mut cut = self.max_line;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            line.truncate(cut);
            line.push('…');
        }
        if self.capacity == 0 {
            self.first += 1;
            return;
        }
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
            self.first += 1;
        }
        self.lines.push_back(line);
    }

    /// Sequence number the next line will get.
    pub fn next(&self) -> u64 {
        self.first + self.lines.len() as u64
    }

    /// The lines after `after`. `None` (a page that just opened) gets no
    /// lines, only where to start: the log begins empty.
    pub fn since(&self, after: Option<u64>) -> LogPage {
        let next = self.next();
        let Some(after) = after else {
            return LogPage {
                lines: Vec::new(),
                next,
                missed: 0,
            };
        };
        // A number from before a restart is larger than anything here:
        // start over from now.
        if after > next {
            return LogPage {
                lines: Vec::new(),
                next,
                missed: 0,
            };
        }
        let missed = self.first.saturating_sub(after);
        let skip = after.saturating_sub(self.first) as usize;
        LogPage {
            lines: self.lines.iter().skip(skip).cloned().collect(),
            next,
            missed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(n: usize, capacity: usize) -> LogBuffer {
        let mut b = LogBuffer::new(capacity, 100);
        for i in 0..n {
            b.push(format!("line {i}"));
        }
        b
    }

    #[test]
    fn a_page_that_just_opened_gets_no_old_lines() {
        let b = filled(5, 30);
        assert_eq!(
            b.since(None),
            LogPage {
                lines: vec![],
                next: 5,
                missed: 0
            }
        );
    }

    #[test]
    fn lines_after_the_last_seen_one_come_back_in_order() {
        let b = filled(5, 30);
        let page = b.since(Some(3));
        assert_eq!(page.lines, ["line 3", "line 4"]);
        assert_eq!((page.next, page.missed), (5, 0));
    }

    #[test]
    fn nothing_new_is_an_empty_page() {
        let b = filled(5, 30);
        assert!(b.since(Some(5)).lines.is_empty());
    }

    #[test]
    fn oldest_lines_fall_out_when_full() {
        let b = filled(10, 4);
        assert_eq!(
            b.since(Some(6)).lines,
            ["line 6", "line 7", "line 8", "line 9"]
        );
        assert_eq!(b.next(), 10);
    }

    #[test]
    fn a_slow_page_learns_how_many_lines_it_missed() {
        let b = filled(10, 4);
        let page = b.since(Some(2));
        assert_eq!(page.missed, 4);
        assert_eq!(page.lines, ["line 6", "line 7", "line 8", "line 9"]);
    }

    #[test]
    fn a_number_from_before_a_restart_starts_over() {
        let b = filled(3, 30);
        assert_eq!(
            b.since(Some(500)),
            LogPage {
                lines: vec![],
                next: 3,
                missed: 0
            }
        );
    }

    #[test]
    fn long_lines_are_cut_on_a_character_boundary() {
        let mut b = LogBuffer::new(4, 10);
        b.push("D’ITALIA – race control".to_owned());
        let line = &b.since(Some(0)).lines[0];
        assert!(line.ends_with('…'));
        assert!(line.len() <= 10 + '…'.len_utf8());
    }

    #[test]
    fn zero_capacity_keeps_nothing_but_still_counts() {
        let b = filled(3, 0);
        assert_eq!(b.next(), 3);
        assert!(b.since(Some(0)).lines.is_empty());
    }

    #[test]
    fn page_serialises_for_the_web() {
        let json = serde_json::to_string(&filled(1, 4).since(Some(0))).unwrap();
        assert_eq!(json, r#"{"lines":["line 0"],"next":1,"missed":0}"#);
    }
}
