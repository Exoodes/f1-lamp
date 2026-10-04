//! Several archived streams merged into one list ordered by time, the way the
//! live feed delivers them.
//!
//! The merge is lazy: each stream keeps only its next line in memory, and a
//! line is parsed when it's its turn. On the ESP32 that matters, because a
//! whole race of parsed race-control messages would cost tens of KB of heap.

use std::{collections::VecDeque, fmt, time::Duration};

use crate::{
    feed::{DriverList, RaceControl, SessionInfo, SessionStatus, TopThree, TrackStatus},
    stream::{parse_line, ParseLineError},
    track_state::FeedMessage,
};

/// Which stream a file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    SessionInfo,
    TrackStatus,
    SessionStatus,
    RaceControl,
    DriverList,
    TopThree,
}

impl Stream {
    /// Whether a line can carry anything the lamp uses, checked without
    /// parsing. Most TopThree lines only update gaps and lap times, and most
    /// DriverList lines only the running order; on the ESP32, parsing them
    /// all would take seconds when jumping through a race.
    pub fn is_relevant(self, json: &str) -> bool {
        match self {
            Stream::TopThree => json.contains("\"RacingNumber\""),
            Stream::DriverList => json.contains("\"TeamColour\""),
            _ => true,
        }
    }

    /// One JSON message of this stream as feed messages. A race control line
    /// can hold several messages; the other streams always give one.
    pub fn parse(self, json: &str) -> Result<Vec<FeedMessage>, serde_json::Error> {
        Ok(match self {
            Stream::SessionInfo => vec![FeedMessage::SessionInfo(serde_json::from_str::<
                SessionInfo,
            >(json)?)],
            Stream::TrackStatus => vec![FeedMessage::Track(serde_json::from_str::<TrackStatus>(
                json,
            )?)],
            Stream::SessionStatus => vec![FeedMessage::Session(serde_json::from_str::<
                SessionStatus,
            >(json)?)],
            Stream::RaceControl => serde_json::from_str::<RaceControl>(json)?
                .messages
                .map(|m| m.into_vec())
                .unwrap_or_default()
                .into_iter()
                .map(FeedMessage::RaceControl)
                .collect(),
            Stream::DriverList => vec![FeedMessage::DriverList(
                serde_json::from_str::<DriverList>(json)?,
            )],
            Stream::TopThree => vec![FeedMessage::TopThree(serde_json::from_str::<TopThree>(
                json,
            )?)],
        })
    }
}

#[derive(Debug)]
pub enum TimelineErrorKind {
    Line(ParseLineError),
    Json(serde_json::Error),
}

/// A line that couldn't be read. The timeline skips it and carries on.
#[derive(Debug)]
pub struct TimelineError {
    pub stream: Stream,
    /// 1-based, as an editor shows it.
    pub line: usize,
    pub kind: TimelineErrorKind,
}

impl fmt::Display for TimelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TimelineErrorKind::Line(e) => write!(f, "{:?} line {}: {e}", self.stream, self.line),
            TimelineErrorKind::Json(e) => write!(f, "{:?} line {}: {e}", self.stream, self.line),
        }
    }
}

/// One stream and the next line it will give.
struct Source<'a> {
    stream: Stream,
    lines: std::iter::Enumerate<std::str::Lines<'a>>,
    /// Offset, JSON and line number of the next line, already split.
    head: Option<(Duration, &'a str, usize)>,
}

impl<'a> Source<'a> {
    /// Moves `head` to the next non-empty line. A line that doesn't split is
    /// returned as an error, and `head` moves on past it.
    fn advance(&mut self) -> Option<TimelineError> {
        self.head = None;
        for (i, raw) in self.lines.by_ref() {
            if raw.trim().trim_start_matches('\u{feff}').is_empty() {
                continue;
            }
            match parse_line(raw) {
                Ok(line) => {
                    self.head = Some((line.offset, line.json, i + 1));
                    return None;
                }
                Err(e) => {
                    return Some(TimelineError {
                        stream: self.stream,
                        line: i + 1,
                        kind: TimelineErrorKind::Line(e),
                    })
                }
            }
        }
        None
    }
}

/// Iterates over every message of all streams, ordered by offset. Messages
/// with the same offset come in the order the streams were given.
pub struct Timeline<'a> {
    sources: Vec<Source<'a>>,
    /// Messages of a line already parsed but not yet returned (race control
    /// lines can hold several).
    pending: VecDeque<(Duration, FeedMessage)>,
    /// Bad lines found while moving a source forward, returned first.
    errors: VecDeque<TimelineError>,
}

impl<'a> Timeline<'a> {
    pub fn new(files: &[(Stream, &'a str)]) -> Self {
        let mut errors = VecDeque::new();
        let sources = files
            .iter()
            .map(|&(stream, text)| {
                let mut source = Source {
                    stream,
                    lines: text.lines().enumerate(),
                    head: None,
                };
                // A bad line moves on by itself; keep going until a good one.
                while let Some(e) = source.advance() {
                    errors.push_back(e);
                }
                source
            })
            .collect();
        Timeline {
            sources,
            pending: VecDeque::new(),
            errors,
        }
    }
}

impl Iterator for Timeline<'_> {
    type Item = Result<(Duration, FeedMessage), TimelineError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Pending messages first: they come from an earlier line than
            // any error found while moving on.
            if let Some(item) = self.pending.pop_front() {
                return Some(Ok(item));
            }
            if let Some(e) = self.errors.pop_front() {
                return Some(Err(e));
            }

            // The source whose next line is earliest. `min_by_key` returns
            // the first of equal keys, so ties keep the order of `files`.
            let source = self
                .sources
                .iter_mut()
                .filter(|s| s.head.is_some())
                .min_by_key(|s| s.head.map(|(offset, _, _)| offset))?;
            let (offset, json, line) = source.head?;
            let stream = source.stream;
            let parsed = if stream.is_relevant(json) {
                stream.parse(json)
            } else {
                Ok(Vec::new())
            };
            match parsed {
                Ok(messages) => self
                    .pending
                    .extend(messages.into_iter().map(|m| (offset, m))),
                Err(e) => self.errors.push_back(TimelineError {
                    stream,
                    line,
                    kind: TimelineErrorKind::Json(e),
                }),
            }
            while let Some(e) = source.advance() {
                self.errors.push_back(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::{SessionState, TrackCode};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A short description of a message, easy to compare.
    fn describe(msg: &FeedMessage) -> String {
        match msg {
            FeedMessage::Track(t) => format!("track {:?}", t.code().unwrap()),
            FeedMessage::Session(s) => format!("session {:?}", s.status.unwrap()),
            FeedMessage::RaceControl(m) => format!("rc {}", m.message.as_deref().unwrap()),
            other => format!("{other:?}"),
        }
    }

    fn collect(files: &[(Stream, &str)]) -> Vec<(Duration, String)> {
        Timeline::new(files)
            .map(|item| {
                let (t, msg) = item.unwrap();
                (t, describe(&msg))
            })
            .collect()
    }

    const TRACK: &str =
        "\u{feff}00:00:01.000{\"Status\":\"1\"}\r\n00:00:03.000{\"Status\":\"2\"}\r\n";
    const SESSION: &str = "\u{feff}00:00:02.000{\"Status\":\"Started\"}\n";

    #[test]
    fn streams_are_merged_by_offset() {
        let got = collect(&[
            (Stream::TrackStatus, TRACK),
            (Stream::SessionStatus, SESSION),
        ]);
        assert_eq!(
            got,
            [
                (ms(1000), "track AllClear".to_owned()),
                (ms(2000), "session Started".to_owned()),
                (ms(3000), "track Yellow".to_owned()),
            ]
        );
    }

    #[test]
    fn equal_offsets_keep_the_order_the_streams_were_given() {
        let track = "00:00:01.000{\"Status\":\"1\"}";
        let session = "00:00:01.000{\"Status\":\"Started\"}";
        let got = collect(&[
            (Stream::SessionStatus, session),
            (Stream::TrackStatus, track),
        ]);
        assert_eq!(got[0].1, "session Started");
        assert_eq!(got[1].1, "track AllClear");
    }

    #[test]
    fn race_control_line_with_several_messages_gives_each_in_index_order() {
        let rc = r#"00:00:05.000{"Messages":{"8":{"Message":"B"},"7":{"Message":"A"}}}"#;
        let got = collect(&[(Stream::RaceControl, rc)]);
        assert_eq!(
            got,
            [(ms(5000), "rc A".to_owned()), (ms(5000), "rc B".to_owned())]
        );
    }

    #[test]
    fn race_control_line_without_messages_gives_nothing() {
        assert!(collect(&[(Stream::RaceControl, "00:00:05.000{}")]).is_empty());
    }

    #[test]
    fn empty_lines_are_skipped() {
        let track = "\n00:00:01.000{\"Status\":\"1\"}\n\n\r\n";
        assert_eq!(collect(&[(Stream::TrackStatus, track)]).len(), 1);
    }

    #[test]
    fn empty_input_gives_nothing() {
        assert!(collect(&[]).is_empty());
        assert!(collect(&[(Stream::TrackStatus, "")]).is_empty());
    }

    #[test]
    fn bad_line_is_reported_with_its_number_and_the_rest_still_plays() {
        let track = "00:00:01.000{\"Status\":\"1\"}\nnot a line\n00:00:03.000{\"Status\":\"2\"}";
        let items: Vec<_> = Timeline::new(&[(Stream::TrackStatus, track)]).collect();
        assert_eq!(items.len(), 3);
        let errors: Vec<&TimelineError> = items.iter().filter_map(|i| i.as_ref().err()).collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 2);
        assert!(matches!(
            errors[0].kind,
            TimelineErrorKind::Line(ParseLineError::MissingJson)
        ));
        let codes: Vec<TrackCode> = items
            .into_iter()
            .filter_map(|i| match i {
                Ok((_, FeedMessage::Track(t))) => t.code(),
                _ => None,
            })
            .collect();
        assert_eq!(codes, [TrackCode::AllClear, TrackCode::Yellow]);
    }

    #[test]
    fn bad_json_is_reported_and_skipped() {
        let session = "00:00:01.000{\"Status\":\nx\n00:00:02.000{\"Status\":\"Started\"}";
        let items: Vec<_> = Timeline::new(&[(Stream::SessionStatus, session)]).collect();
        // Line 1 is broken JSON, line 2 has no offset, line 3 is fine.
        let lines: Vec<usize> = items[..2]
            .iter()
            .map(|i| i.as_ref().unwrap_err().line)
            .collect();
        assert_eq!(lines, [1, 2]);
        assert!(matches!(
            items[0].as_ref().unwrap_err().kind,
            TimelineErrorKind::Json(_)
        ));
        assert!(matches!(
            &items[2],
            Ok((_, FeedMessage::Session(s))) if s.status == Some(SessionState::Started)
        ));
    }

    #[test]
    fn error_message_names_stream_and_line() {
        let err = Timeline::new(&[(Stream::TrackStatus, "garbage")])
            .next()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "TrackStatus line 1: line has no JSON object"
        );
    }

    #[test]
    fn top_three_lines_without_racing_number_are_skipped() {
        let top = concat!(
            r#"00:00:01.000{"Lines":{"1":{"DiffToAhead":"+3.8"}}}"#,
            "\n",
            r#"00:00:02.000{"Lines":{"0":{"RacingNumber":"12"}}}"#,
        );
        let items: Vec<_> = Timeline::new(&[(Stream::TopThree, top)]).collect();
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], Ok((t, FeedMessage::TopThree(_))) if *t == ms(2000)));
    }

    #[test]
    fn driver_list_lines_without_team_colour_are_skipped() {
        let drivers = concat!(
            r#"00:00:01.000{"12":{"TeamColour":"00D7B6"}}"#,
            "\n",
            r#"00:00:02.000{"44":{"Line":5}}"#,
        );
        assert_eq!(Timeline::new(&[(Stream::DriverList, drivers)]).count(), 1);
    }

    #[test]
    fn skipped_lines_are_not_checked_for_json_errors() {
        // Cheap skipping means a broken line nobody needs costs nothing.
        let top = "00:00:01.000{\"Lines\":";
        assert_eq!(Timeline::new(&[(Stream::TopThree, top)]).count(), 0);
    }
}
