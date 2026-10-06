//! The live connection to F1's timing feed: negotiate, WebSocket, SignalR
//! handshake, subscribe, pings, reconnects. The protocol and the race logic
//! are in f1-core (`live::LiveSession`); this file only moves bytes.
//!
//! The net thread decides when the feed is wanted (pre-session and live
//! phases) through [`LiveControl`]; outside those the connection is closed.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        Arc, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use esp_idf_svc::http::Method;
use f1_core::{
    backoff::Backoff,
    feed_health::FeedHealth,
    heartbeat::Heartbeat,
    input::{Input, RaceEvent},
    live::{LiveSession, Received},
    live_timers::{LiveTimers, SILENCE_LIMIT, TLS_HOLD_MAX},
    negotiate,
    signalr::PING,
};

use crate::{
    config,
    io::{
        clock, http, system,
        token_store::TokenKeeper,
        ws::{WebSocket, WsEvent},
    },
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
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
        .stack_size(config::stack::LIVE)
        .spawn(move || run(&tx, &control, &tokens, &heartbeat))?;
    Ok(handle)
}

/// Never returns. Without the render loop the lamp is dead anyway, and the
/// thread check in main restarts the chip.
fn run(tx: &SyncSender<Input>, control: &LiveControl, tokens: &TokenKeeper, heartbeat: &Heartbeat) {
    let mut retry = Backoff::new(FIRST_RETRY, MAX_RETRY);
    let mut health = FeedHealth::default();
    loop {
        heartbeat.beat(Instant::now());
        if !control.wanted() {
            retry.reset();
            if tell(tx, health.not_wanted()).is_err() {
                return;
            }
            thread::sleep(IDLE);
            continue;
        }
        match session(tx, control, tokens, heartbeat, &mut health) {
            Ok(()) => retry.reset(),
            Err(e) => {
                let wait = retry.next_wait();
                log::warn!("live: {e:#}; again in {} s", wait.as_secs());
                if tell(tx, health.failed()).is_err() {
                    return;
                }
                heartbeat.beat(Instant::now());
                thread::sleep(wait);
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

/// The load balancer's cookie, then a connection token (see
/// `f1_core::negotiate`). `auth` is the `Authorization` header value, with
/// the F1TV token.
fn negotiate(auth: Option<&str>) -> anyhow::Result<(Option<String>, String)> {
    let options = http::request(Method::Options, negotiate::NEGOTIATE_URL, &[])
        .context("negotiate (OPTIONS)")?;
    // Only one `Set-Cookie` gets here (`http::Reply`), so at most one of the
    // two load-balancer cookies; either keeps us on the same server.
    let cookie = negotiate::load_balancer_cookie(options.set_cookie.as_deref());
    if cookie.is_none() {
        log::warn!("live: no load-balancer cookie; trying without");
    }

    let mut headers = vec![("Content-Length", "0")];
    if let Some(cookie) = &cookie {
        headers.push(("Cookie", cookie));
    }
    if let Some(auth) = auth {
        headers.push(("Authorization", auth));
    }
    let reply = http::request(Method::Post, negotiate::NEGOTIATE_POST_URL, &headers)
        .context("negotiate (POST)")?;
    if reply.status != 200 {
        bail!(
            "negotiate: HTTP {}: {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
    let token = negotiate::connection_token(&reply.body).context("negotiate answer")?;
    Ok((cookie, token))
}

/// One connection, from negotiate until it drops or isn't wanted any more.
fn session(
    tx: &SyncSender<Input>,
    control: &LiveControl,
    tokens: &TokenKeeper,
    heartbeat: &Heartbeat,
    health: &mut FeedHealth,
) -> anyhow::Result<()> {
    let mut conn = Connection::open(tokens, heartbeat)?;
    let mut live = LiveSession::new(MAX_FRAME);
    loop {
        if !control.wanted() {
            log::info!("live: closing, feed not wanted");
            return Ok(());
        }
        let now = Instant::now();
        heartbeat.beat(now);
        conn.tick(now)?;
        let Some(data) = conn.next_data(now)? else {
            continue;
        };
        if data.offset == 0 {
            live.expect(data.frame_len);
        }
        let received = live.receive(&data.bytes);
        handle(received, &mut conn, tx, control, health)?;
    }
}

/// An open WebSocket to the feed, subscribed or about to be.
struct Connection {
    /// Declared first, so it's dropped (closed) before `events` and `tls`.
    ws: WebSocket,
    events: Receiver<WsEvent>,
    /// No HTTPS request while the handshake and the large first answer need
    /// the heap; released once subscribed, or when `timers` says so.
    tls: Option<MutexGuard<'static, ()>>,
    timers: LiveTimers,
}

/// A piece of a WebSocket frame: `frame_len` is the whole frame's length and
/// `offset` where this piece starts in it.
struct Data {
    bytes: Vec<u8>,
    frame_len: usize,
    offset: usize,
}

impl Connection {
    /// Negotiates, opens the WebSocket and sends the handshake and the
    /// subscriptions.
    fn open(tokens: &TokenKeeper, heartbeat: &Heartbeat) -> anyhow::Result<Self> {
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

        let tls = Some(http::TLS.lock().unwrap_or_else(|e| e.into_inner()));
        let (events_tx, events) = mpsc::channel();
        let ws = WebSocket::connect(&negotiate::hub_url(&token), &headers, events_tx)?;

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

        Ok(Connection {
            ws,
            events,
            tls,
            timers: LiveTimers::new(Instant::now()),
        })
    }

    /// Does what `timers` says is due: let go of the TLS lock, ping, or give
    /// up on a silent connection.
    fn tick(&mut self, now: Instant) -> anyhow::Result<()> {
        let due = self.timers.tick(now);
        if due.release_tls {
            drop(self.tls.take());
            log::warn!(
                "live: no answer to Subscribe within {} s; letting HTTPS requests run",
                TLS_HOLD_MAX.as_secs()
            );
        }
        if due.ping {
            self.ws.send_text(PING, SEND_TIMEOUT)?;
        }
        if due.dead {
            bail!("nothing received for {} s", SILENCE_LIMIT.as_secs());
        }
        Ok(())
    }

    /// The answer to `Subscribe` came: the TLS lock can go.
    fn subscribed(&mut self) {
        drop(self.tls.take());
        self.timers.subscribed();
    }

    /// Waits for the next piece of data until something in `tick` is due.
    /// `None` when that time came first, or for an event without data.
    fn next_data(&mut self, now: Instant) -> anyhow::Result<Option<Data>> {
        let wait = self.timers.wake_at().saturating_duration_since(now);
        let event = match self.events.recv_timeout(wait) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => return Ok(None),
            Err(RecvTimeoutError::Disconnected) => bail!("WebSocket task gone"),
        };
        match event {
            WsEvent::Data {
                bytes,
                frame_len,
                offset,
            } => {
                self.timers.data(Instant::now());
                Ok(Some(Data {
                    bytes,
                    frame_len,
                    offset,
                }))
            }
            WsEvent::Connected => Ok(None),
            WsEvent::Disconnected => bail!("connection closed"),
            WsEvent::Error => bail!("WebSocket error"),
        }
    }
}

/// Acts on what a batch of received bytes produced: logs problems, lets the
/// TLS lock go once subscribed, and passes race events on to the render loop.
/// An error when the server closes the connection.
fn handle(
    received: Received,
    conn: &mut Connection,
    tx: &SyncSender<Input>,
    control: &LiveControl,
    health: &mut FeedHealth,
) -> anyhow::Result<()> {
    for problem in &received.problems {
        log::warn!("live: {problem}");
    }
    if received.subscribed {
        conn.subscribed();
        tell(tx, health.working())?;
        let heap = system::heap();
        log::info!(
            "live: subscribed (session {:?}); heap {} B free, largest block {} B",
            received.session_key,
            heap.free,
            heap.largest_block
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
    Ok(())
}
