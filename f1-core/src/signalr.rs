//! The parts of F1's live feed protocol that don't touch the network: ASP.NET
//! Core SignalR with the JSON hub protocol, as used by
//! `wss://livetiming.formula1.com/signalrcore`.
//!
//! Every SignalR message is a JSON object followed by the byte 0x1E. One
//! WebSocket message can hold several of them, or only part of one, so
//! [`Splitter`] collects bytes and hands out complete frames. [`parse`] turns
//! a frame into a [`Message`]. Stream data stays a borrowed [`RawValue`], a
//! slice of the frame, until [`Stream::parse`](crate::timeline::Stream::parse)
//! reads it into typed structs: no `serde_json::Value` tree in between.

use std::{collections::BTreeMap, fmt};

use serde::Deserialize;
use serde_json::value::RawValue;

/// Ends every SignalR message.
pub const RECORD_SEPARATOR: u8 = 0x1e;

/// The first message a client sends: "I speak JSON".
pub const HANDSHAKE: &str = "{\"protocol\":\"json\",\"version\":1}\u{1e}";

/// Keeps the connection alive; the server drops quiet clients.
pub const PING: &str = "{\"type\":6}\u{1e}";

/// The invocation that asks for `streams`. The server answers with a
/// completion holding each stream's current state, then sends updates.
pub fn subscribe(invocation_id: u32, streams: &[&str]) -> String {
    let call = serde_json::json!({
        "type": 1,
        "invocationId": invocation_id.to_string(),
        "target": "Subscribe",
        "arguments": [streams],
    });
    format!("{call}\u{1e}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// More than the splitter's limit arrived without a separator; those
    /// bytes were dropped.
    TooLarge {
        len: usize,
    },
    NotUtf8,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooLarge { len } => write!(f, "frame larger than {len} bytes, dropped"),
            FrameError::NotUtf8 => write!(f, "frame is not UTF-8"),
        }
    }
}

/// Collects received bytes and returns complete frames.
#[derive(Debug)]
pub struct Splitter {
    buf: Vec<u8>,
    max_len: usize,
    /// Throwing away the rest of a frame that was too large.
    skipping: bool,
}

impl Splitter {
    /// `max_len` caps one frame, so a broken stream can't use all memory.
    pub fn new(max_len: usize) -> Self {
        Splitter {
            buf: Vec::new(),
            max_len,
            skipping: false,
        }
    }

    /// Adds received bytes; returns every frame they complete, without the
    /// separator.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Result<String, FrameError>> {
        let mut frames = Vec::new();
        for part in chunk.split_inclusive(|&b| b == RECORD_SEPARATOR) {
            let complete = part.last() == Some(&RECORD_SEPARATOR);
            let bytes = if complete {
                &part[..part.len() - 1]
            } else {
                part
            };

            if !self.skipping {
                if self.buf.len() + bytes.len() > self.max_len {
                    self.buf.clear();
                    self.skipping = true;
                    frames.push(Err(FrameError::TooLarge { len: self.max_len }));
                } else {
                    self.buf.extend_from_slice(bytes);
                }
            }

            if complete {
                if !self.skipping {
                    let frame = std::mem::take(&mut self.buf);
                    frames.push(String::from_utf8(frame).map_err(|_| FrameError::NotUtf8));
                }
                self.skipping = false;
            }
        }
        frames
    }

    /// Bytes waiting for their separator.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

/// One SignalR message. Borrowed from the frame it was parsed from.
#[derive(Debug)]
pub enum Message<'a> {
    /// The server's answer to [`HANDSHAKE`]: `{}`, or an error.
    Handshake {
        error: Option<String>,
    },
    /// The server calls a method on the client, e.g. `feed`.
    Invocation {
        target: String,
        arguments: Vec<&'a RawValue>,
    },
    /// The answer to one of our invocations, e.g. `Subscribe`.
    Completion {
        invocation_id: Option<String>,
        result: Option<&'a RawValue>,
        error: Option<String>,
    },
    Ping,
    /// The server is closing the connection.
    Close {
        error: Option<String>,
    },
    /// A type we don't use (streams, cancel, ...).
    Other(u8),
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(rename = "type")]
    kind: Option<u8>,
    target: Option<String>,
    #[serde(borrow, default)]
    arguments: Vec<&'a RawValue>,
    #[serde(rename = "invocationId")]
    invocation_id: Option<String>,
    #[serde(borrow)]
    result: Option<&'a RawValue>,
    error: Option<String>,
}

pub fn parse(frame: &str) -> Result<Message<'_>, serde_json::Error> {
    let e: Envelope = serde_json::from_str(frame)?;
    Ok(match e.kind {
        None => Message::Handshake { error: e.error },
        Some(1) => Message::Invocation {
            target: e.target.unwrap_or_default(),
            arguments: e.arguments,
        },
        Some(3) => Message::Completion {
            invocation_id: e.invocation_id,
            result: e.result,
            error: e.error,
        },
        Some(6) => Message::Ping,
        Some(7) => Message::Close { error: e.error },
        Some(other) => Message::Other(other),
    })
}

/// One stream update: `feed(stream, data, timestamp)`.
#[derive(Debug)]
pub struct FeedUpdate<'a> {
    pub stream: String,
    pub data: &'a RawValue,
    pub timestamp: Option<String>,
}

impl<'a> Message<'a> {
    /// The update this message carries, if it's a `feed` invocation.
    pub fn feed_update(&self) -> Option<FeedUpdate<'a>> {
        let Message::Invocation { target, arguments } = self else {
            return None;
        };
        if !target.eq_ignore_ascii_case("feed") {
            return None;
        }
        let stream = serde_json::from_str(arguments.first()?.get()).ok()?;
        let data = *arguments.get(1)?;
        let timestamp = arguments
            .get(2)
            .and_then(|t| serde_json::from_str(t.get()).ok());
        Some(FeedUpdate {
            stream,
            data,
            timestamp,
        })
    }
}

/// The `Subscribe` completion's result: each stream's current state, by name.
/// Streams with no state yet are left out.
pub fn initial_state(result: &RawValue) -> Result<Vec<(String, &RawValue)>, serde_json::Error> {
    // `null` becomes `None`.
    let streams: BTreeMap<String, Option<&RawValue>> = serde_json::from_str(result.get())?;
    Ok(streams
        .into_iter()
        .filter_map(|(name, data)| Some((name, data?)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RS: char = '\u{1e}';

    fn frames(s: &mut Splitter, chunk: &str) -> Vec<String> {
        s.push(chunk.as_bytes())
            .into_iter()
            .map(|f| f.unwrap())
            .collect()
    }

    #[test]
    fn one_frame_in_one_chunk() {
        let mut s = Splitter::new(1024);
        assert_eq!(
            frames(&mut s, &format!("{{\"type\":6}}{RS}")),
            ["{\"type\":6}"]
        );
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn frame_split_across_two_chunks() {
        let mut s = Splitter::new(1024);
        assert!(frames(&mut s, "{\"type\":").is_empty());
        assert_eq!(s.pending(), 8);
        assert_eq!(frames(&mut s, &format!("6}}{RS}")), ["{\"type\":6}"]);
    }

    #[test]
    fn two_frames_in_one_chunk() {
        let mut s = Splitter::new(1024);
        assert_eq!(
            frames(&mut s, &format!("{{\"a\":1}}{RS}{{\"b\":2}}{RS}")),
            ["{\"a\":1}", "{\"b\":2}"]
        );
    }

    #[test]
    fn frame_end_and_start_of_the_next_in_one_chunk() {
        let mut s = Splitter::new(1024);
        frames(&mut s, "{\"a\"");
        assert_eq!(frames(&mut s, &format!(":1}}{RS}{{\"b\"")), ["{\"a\":1}"]);
        assert_eq!(frames(&mut s, &format!(":2}}{RS}")), ["{\"b\":2}"]);
    }

    #[test]
    fn separator_alone_ends_the_buffered_frame() {
        let mut s = Splitter::new(1024);
        frames(&mut s, "{}");
        assert_eq!(frames(&mut s, &RS.to_string()), ["{}"]);
    }

    #[test]
    fn utf8_character_split_across_chunks_survives() {
        let mut s = Splitter::new(1024);
        let text = format!("{{\"m\":\"D’ITALIA\"}}{RS}");
        let bytes = text.as_bytes();
        // Cut inside the three bytes of ’.
        let cut = text.find('’').unwrap() + 1;
        assert!(s.push(&bytes[..cut]).is_empty());
        let out = s.push(&bytes[cut..]);
        assert_eq!(out, [Ok("{\"m\":\"D’ITALIA\"}".to_owned())]);
    }

    #[test]
    fn too_large_frame_is_dropped_and_the_next_one_still_arrives() {
        let mut s = Splitter::new(8);
        assert_eq!(
            s.push(b"0123456789"),
            [Err(FrameError::TooLarge { len: 8 })]
        );
        // The rest of the big frame is skipped, the next frame is whole.
        assert!(s.push(b"abc\x1e").is_empty());
        assert_eq!(s.push(b"{}\x1e"), [Ok("{}".to_owned())]);
    }

    #[test]
    fn invalid_utf8_is_reported() {
        let mut s = Splitter::new(64);
        assert_eq!(s.push(b"\xff\x1e"), [Err(FrameError::NotUtf8)]);
    }

    #[test]
    fn handshake_and_ping_end_with_the_separator() {
        assert!(HANDSHAKE.ends_with(RS));
        assert!(PING.ends_with(RS));
        assert!(matches!(
            parse(PING.trim_end_matches(RS)).unwrap(),
            Message::Ping
        ));
    }

    #[test]
    fn subscribe_lists_the_streams_as_one_argument() {
        let frame = subscribe(1, &["TrackStatus", "SessionStatus"]);
        assert!(frame.ends_with(RS));
        let v: serde_json::Value = serde_json::from_str(frame.trim_end_matches(RS)).unwrap();
        assert_eq!(v["type"], 1);
        assert_eq!(v["invocationId"], "1");
        assert_eq!(v["target"], "Subscribe");
        assert_eq!(
            v["arguments"],
            serde_json::json!([["TrackStatus", "SessionStatus"]])
        );
    }

    #[test]
    fn empty_object_is_the_handshake_answer() {
        assert!(matches!(
            parse("{}").unwrap(),
            Message::Handshake { error: None }
        ));
    }

    #[test]
    fn handshake_error_is_kept() {
        let Message::Handshake { error } = parse(r#"{"error":"bad protocol"}"#).unwrap() else {
            panic!()
        };
        assert_eq!(error.as_deref(), Some("bad protocol"));
    }

    #[test]
    fn feed_invocation_gives_stream_data_and_timestamp() {
        let frame = r#"{"type":1,"target":"feed","arguments":["TrackStatus",{"Status":"2","Message":"Yellow"},"2026-10-09T09:31:02.123Z"]}"#;
        let msg = parse(frame).unwrap();
        let update = msg.feed_update().unwrap();
        assert_eq!(update.stream, "TrackStatus");
        assert_eq!(update.data.get(), r#"{"Status":"2","Message":"Yellow"}"#);
        assert_eq!(
            update.timestamp.as_deref(),
            Some("2026-10-09T09:31:02.123Z")
        );
    }

    #[test]
    fn feed_data_parses_with_the_stream_parsers() {
        use crate::{timeline::Stream, track_state::FeedMessage};
        let frame = r#"{"type":1,"target":"feed","arguments":["TrackStatus",{"Status":"4","Message":"SCDeployed"},"t"]}"#;
        let msg = parse(frame).unwrap();
        let update = msg.feed_update().unwrap();
        let stream = Stream::from_name(&update.stream).unwrap();
        let parsed = stream.parse(update.data.get()).unwrap();
        assert!(matches!(
            &parsed[..],
            [FeedMessage::Track(t)] if t.code() == Some(crate::feed::TrackCode::ScDeployed)
        ));
    }

    #[test]
    fn other_invocations_are_not_feed_updates() {
        let msg = parse(r#"{"type":1,"target":"somethingElse","arguments":[1]}"#).unwrap();
        assert!(msg.feed_update().is_none());
    }

    #[test]
    fn completion_holds_the_initial_state_of_each_stream() {
        let frame = r#"{"type":3,"invocationId":"1","result":{"TrackStatus":{"Status":"1","Message":"AllClear"},"SessionStatus":{"Status":"Inactive"},"TopThree":null}}"#;
        let Message::Completion {
            invocation_id,
            result,
            error,
        } = parse(frame).unwrap()
        else {
            panic!()
        };
        assert_eq!(invocation_id.as_deref(), Some("1"));
        assert_eq!(error, None);
        let state = initial_state(result.unwrap()).unwrap();
        let names: Vec<&str> = state.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["SessionStatus", "TrackStatus"]);
        assert_eq!(state[1].1.get(), r#"{"Status":"1","Message":"AllClear"}"#);
    }

    #[test]
    fn completion_error_is_kept() {
        let Message::Completion { error, .. } =
            parse(r#"{"type":3,"invocationId":"1","error":"no such method"}"#).unwrap()
        else {
            panic!()
        };
        assert_eq!(error.as_deref(), Some("no such method"));
    }

    #[test]
    fn close_message_carries_its_error() {
        let Message::Close { error } =
            parse(r#"{"type":7,"error":"Connection closed with an error."}"#).unwrap()
        else {
            panic!()
        };
        assert!(error.is_some());
    }

    #[test]
    fn unknown_type_is_other() {
        assert!(matches!(parse(r#"{"type":5}"#).unwrap(), Message::Other(5)));
    }

    #[test]
    fn data_with_escapes_stays_raw() {
        let frame = r#"{"type":1,"target":"feed","arguments":["RaceControlMessages",{"Messages":[{"Message":"CAR 11 \"PER\" – NOTED"}]},"t"]}"#;
        let msg = parse(frame).unwrap();
        assert!(msg
            .feed_update()
            .unwrap()
            .data
            .get()
            .contains(r#"\"PER\" –"#));
    }
}
