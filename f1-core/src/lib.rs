//! Everything the F1 Lightbox decides, without hardware: it builds and tests on
//! the PC, and the firmware (`f1/`) only moves bytes in and LED colours out.
//!
//! - The live feed: [`negotiate`] (before the WebSocket), [`signalr`] (frames
//!   and messages), [`live`] (one connection: bytes in, race events out),
//!   [`live_timers`] (pings, silence, the TLS lock) and [`feed_health`].
//! - What the feed says: [`feed`] (the streams' JSON), [`track_state`] (which
//!   flag is out) and [`tracker`] (winner, pit stops, fastest laps, overtakes).
//! - Archived sessions: [`stream`] (one line), [`timeline`] (the streams merged
//!   by time), [`replay`] and [`playback`] (its clock).
//! - The lamp: [`input`] (what the threads send), [`controller`] (what to
//!   show), [`effect`], [`theme`], [`overlays`], [`delay`] (the TV delay),
//!   [`post`] (brightness), [`frame`], [`color`], [`snapshot`], [`command`],
//!   [`settings`] and [`drivers`].
//! - Around it: [`schedule`] and [`openf1`] (when sessions are), [`token`]
//!   (F1TV), [`ota_trial`], [`heartbeat`], [`backoff`], [`frame_stats`] and
//!   [`logbuf`].

#![forbid(unsafe_code)]

pub mod backoff;
pub mod color;
pub mod command;
pub mod controller;
pub mod delay;
pub mod drivers;
pub mod effect;
pub mod feed;
pub mod feed_health;
pub mod frame;
pub mod frame_stats;
pub mod heartbeat;
pub mod input;
pub mod live;
pub mod live_timers;
pub mod logbuf;
pub mod negotiate;
pub mod openf1;
pub mod ota_trial;
pub mod overlays;
pub mod playback;
pub mod post;
pub mod replay;
pub mod schedule;
pub mod settings;
pub mod signalr;
pub mod snapshot;
pub mod stream;
pub mod theme;
pub mod timeline;
pub mod token;
pub mod track_state;
pub mod tracker;
