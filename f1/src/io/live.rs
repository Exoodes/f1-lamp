//! The live connection to F1's timing feed: negotiate, WebSocket, SignalR
//! handshake, subscribe, pings, reconnects. The protocol and the race logic
//! are in f1-core (`live::LiveSession`); this file only moves bytes.
//!
//! The net thread decides when the feed is wanted (pre-session and live
//! phases) through [`LiveControl`]; outside those the connection is closed.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use esp_idf_svc::http::Method;
use f1_core::{
    feed_health::FeedHealth,
    heartbeat::Heartbeat,
    input::{Input, RaceEvent},
    live::LiveSession,
    signalr::PING,
};
use serde::Deserialize;

use crate::io::{
    clock, http,
    token_store::TokenKeeper,
    ws::{WebSocket, WsEvent},
};

const NEGOTIATE: &str = "https://livetiming.formula1.com/signalrcore/negotiate";
const HUB: &str = "wss://livetiming.formula1.com/signalrcore";

/// The server drops clients that stay quiet for about 30 s.
const PING_EVERY: Duration = Duration::from_secs(15);
/// Heartbeat and pings arrive every few seconds; this long without anything
/// means the connection is dead even if nobody said so.
const SILENCE_LIMIT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest the TLS lock is kept after the WebSocket opens, waiting for
/// the answer to `Subscribe`. It normally comes within a second; if it never
/// does (refused, or too large and skipped), holding on would block every
/// HTTPS request of the net thread for the whole session.
const TLS_HOLD_MAX: Duration = Duration::from_secs(15);
/// One SignalR message at most. The state we use arrives as about 12 KB
/// after a race. The race control history (45 KB) is larger on purpose: the
/// splitter skips it without buffering, which logs one "frame larger than"
/// warning per connection. A single 45 KB block is often not available.
const MAX_FRAME: usize = 24 * 1024;
/// How often to look again while the feed isn't wanted.
const IDLE: Duration = Duration::from_secs(1);
const FIRST_RETRY: Duration = Duration::from_secs(5);
const MAX_RETRY: Duration = Duration::from_secs(60);

/// Shared between the net thread, which decides, and the live thread.
#[derive(Default)]
pub struct LiveControl {
    wanted: AtomicBool,
    /// The session whose winner the feed has shown, so the OpenF1 lookup
    /// doesn't show it a second time.
    winner_shown: Mutex<Option<u32>>,
}

impl LiveControl {
    pub fn set_wanted(&self, wanted: bool) {
        if self.wanted.swap(wanted, Ordering::Relaxed) != wanted {
            log::info!(
                "live: feed {}",
                if wanted { "wanted" } else { "not wanted" }
            );
        }
    }

    fn wanted(&self) -> bool {
        self.wanted.load(Ordering::Relaxed)
    }

    pub fn winner_shown(&self) -> Option<u32> {
        *self.winner_shown.lock().unwrap()
    }
}

pub fn spawn(
    tx: SyncSender<Input>,
    control: Arc<LiveControl>,
    tokens: Arc<TokenKeeper>,
    heartbeat: Arc<Heartbeat>,
) -> anyhow::Result<JoinHandle<()>> {
    let handle = thread::Builder::new()
        .name("live".into())
        .stack_size(12 * 1024)
        .spawn(move || run(&tx, &control, &tokens, &heartbeat))?;
    Ok(handle)
}

/// Never returns. Without the render loop the lamp is dead anyway, and the
/// thread check in main restarts the chip.
fn run(tx: &SyncSender<Input>, control: &LiveControl, tokens: &TokenKeeper, heartbeat: &Heartbeat) {
    let mut retry = FIRST_RETRY;
    let mut health = FeedHealth::default();
    loop {
        heartbeat.beat(Instant::now());
        if !control.wanted() {
            retry = FIRST_RETRY;
            if tell(tx, health.not_wanted()).is_err() {
                return;
            }
            thread::sleep(IDLE);
            continue;
        }
        match session(tx, control, tokens, heartbeat, &mut health) {
            Ok(()) => retry = FIRST_RETRY,
            Err(e) => {
                log::warn!("live: {e:#}; again in {} s", retry.as_secs());
                if tell(tx, health.failed()).is_err() {
                    return;
                }
                heartbeat.beat(Instant::now());
                thread::sleep(retry);
                retry = (retry * 2).min(MAX_RETRY);
            }
        }
    }
}

/// Tells the render loop when the feed starts or stops failing, so the lamp
/// blinks red instead of showing a flag that may be stale.
fn tell(tx: &SyncSender<Input>, change: Option<bool>) -> anyhow::Result<()> {
    let Some(failing) = change else {
        return Ok(());
    };
    if failing {
        log::warn!("live: the feed keeps failing; the lamp shows the error");
    } else {
        log::info!("live: the feed works again");
    }
    tx.send(Input::FeedFailing(failing))
        .context("render loop is gone")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Negotiation {
    connection_token: String,
}

/// The load balancer's cookie, then a connection token.
/// `auth` is the `Authorization` header value, with the F1TV token.
fn negotiate(auth: Option<&str>) -> anyhow::Result<(Option<String>, String)> {
    // The OPTIONS answer is 405, but it sets the cookie that keeps the
    // WebSocket on the same server as the negotiate.
    let options = http::request(Method::Options, NEGOTIATE, &[]).context("negotiate (OPTIONS)")?;
    let cookie = options
        .set_cookie
        .as_deref()
        .and_then(|c| c.split(';').next())
        .filter(|c| c.starts_with("AWSALBCORS="))
        .map(str::to_owned);
    if cookie.is_none() {
        log::warn!("live: no AWSALBCORS cookie; trying without");
    }

    let mut headers = vec![("Content-Length", "0")];
    if let Some(cookie) = &cookie {
        headers.push(("Cookie", cookie));
    }
    if let Some(auth) = auth {
        headers.push(("Authorization", auth));
    }
    let reply = http::request(
        Method::Post,
        &format!("{NEGOTIATE}?negotiateVersion=1"),
        &headers,
    )
    .context("negotiate (POST)")?;
    if reply.status != 200 {
        bail!(
            "negotiate: HTTP {}: {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
    let negotiation: Negotiation =
        serde_json::from_slice(&reply.body).context("negotiate answer")?;
    Ok((cookie, negotiation.connection_token))
}

/// One connection, from negotiate until it drops or isn't wanted any more.
fn session(
    tx: &SyncSender<Input>,
    control: &LiveControl,
    tokens: &TokenKeeper,
    heartbeat: &Heartbeat,
    health: &mut FeedHealth,
) -> anyhow::Result<()> {
    // Without a usable token, the public streams: the lamp works either way.
    // `auth` holds the token: never log it or `headers`.
    let auth = tokens.bearer_if_usable(clock::unix_now());
    log::info!(
        "live: connecting {} F1TV token",
        if auth.is_some() { "with" } else { "without" }
    );
    let (cookie, token) = negotiate(auth.as_deref())?;
    heartbeat.beat(Instant::now());
    let mut headers = cookie.map_or(String::new(), |c| format!("Cookie: {c}\r\n"));
    if let Some(auth) = &auth {
        headers.push_str(&format!("Authorization: {auth}\r\n"));
    }

    // No HTTPS request while the handshake and the large first answer need
    // the heap; released once subscribed, or after `TLS_HOLD_MAX` at most.
    let mut tls = Some(http::TLS.lock().unwrap_or_else(|e| e.into_inner()));
    let (events_tx, events) = mpsc::channel();
    let ws = WebSocket::connect(&format!("{HUB}?id={token}"), &headers, events_tx)?;

    match events.recv_timeout(CONNECT_TIMEOUT) {
        Ok(WsEvent::Connected) => {}
        Ok(other) => bail!("WebSocket didn't open: {other:?}"),
        Err(_) => bail!(
            "WebSocket didn't open within {} s",
            CONNECT_TIMEOUT.as_secs()
        ),
    }
    for frame in LiveSession::opening_frames(auth.is_some()) {
        ws.send_text(&frame, SEND_TIMEOUT)?;
    }
    log::info!("live: connected, subscribing");

    let mut live = LiveSession::new(MAX_FRAME);
    let mut last_data = Instant::now();
    let mut next_ping = Instant::now() + PING_EVERY;
    let release_tls_at = Instant::now() + TLS_HOLD_MAX;
    loop {
        if !control.wanted() {
            log::info!("live: closing, feed not wanted");
            return Ok(());
        }
        let now = Instant::now();
        heartbeat.beat(now);
        if tls.is_some() && now >= release_tls_at {
            drop(tls.take());
            log::warn!(
                "live: no answer to Subscribe within {} s; letting HTTPS requests run",
                TLS_HOLD_MAX.as_secs()
            );
        }
        if now >= next_ping {
            ws.send_text(PING, SEND_TIMEOUT)?;
            next_ping = now + PING_EVERY;
        }
        if now.duration_since(last_data) > SILENCE_LIMIT {
            bail!("nothing received for {} s", SILENCE_LIMIT.as_secs());
        }

        let wake = if tls.is_some() {
            next_ping.min(release_tls_at)
        } else {
            next_ping
        };
        let event = match events.recv_timeout(wake.saturating_duration_since(now)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => bail!("WebSocket task gone"),
        };
        let (bytes, frame_len, offset) = match event {
            WsEvent::Data {
                bytes,
                frame_len,
                offset,
            } => (bytes, frame_len, offset),
            WsEvent::Connected => continue,
            WsEvent::Disconnected => bail!("connection closed"),
            WsEvent::Error => bail!("WebSocket error"),
        };
        last_data = Instant::now();
        if offset == 0 {
            live.expect(frame_len);
        }

        let received = live.receive(&bytes);
        for problem in &received.problems {
            log::warn!("live: {problem}");
        }
        if received.subscribed {
            drop(tls.take());
            tell(tx, health.working())?;
            let (free, largest) = heap();
            log::info!(
                "live: subscribed (session {:?}); heap {free} B free, largest block {largest} B",
                received.session_key
            );
        }
        for event in received.events {
            log::info!("live: {event:?}");
            if matches!(event, RaceEvent::Winner { .. }) {
                *control.winner_shown.lock().unwrap() = received.session_key;
            }
            tx.send(Input::Race {
                event,
                received: Instant::now(),
            })
            .context("render loop is gone")?;
        }
        if let Some(reason) = received.closed {
            bail!("server closed the connection: {reason}");
        }
    }
}

/// Free heap and the largest block that could still be allocated, in bytes.
fn heap() -> (u32, usize) {
    // SAFETY: both only read ESP-IDF's heap counters.
    unsafe {
        (
            esp_idf_svc::sys::esp_get_free_heap_size(),
            esp_idf_svc::sys::heap_caps_get_largest_free_block(esp_idf_svc::sys::MALLOC_CAP_8BIT),
        )
    }
}
