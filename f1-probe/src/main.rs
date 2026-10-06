//! Talks to F1's live timing feed from the PC: negotiate, WebSocket, SignalR
//! handshake, subscribe. Prints every message and saves it, in the archive's
//! `.jsonStream` format, under `captures/<date>_<time>/`, so a capture can
//! become a test fixture or a replay as it is.
//!
//! ```text
//! cargo run                 until Ctrl+C
//! cargo run -- 30           for 30 minutes
//! cargo run -- --raw        also print every SignalR frame
//! cargo run -- --extra      also subscribe to the streams in `EXTRA_STREAMS`
//! ```
//!
//! With the environment variable `F1TV_TOKEN` set, it is sent as
//! `Authorization: Bearer ...` with the negotiate and the WebSocket upgrade.
//! The token itself is never printed.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{ErrorKind, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use f1_core::{
    feed::SessionState,
    live::LiveSession,
    negotiate,
    signalr::{self, Message, Splitter, PING},
    timeline::Stream,
    token::{F1tvToken, TokenStatus},
    track_state::FeedMessage,
};
use tungstenite::{client::IntoClientRequest, http::HeaderValue, WebSocket};

/// The hub's host, for our own TCP connection (see `connect`).
const HOST: &str = "livetiming.formula1.com";

/// The server drops clients that stay quiet for about 30 s.
const PING_EVERY: Duration = Duration::from_secs(15);
/// How long one read waits, so pings go out on time.
const READ_TIMEOUT: Duration = Duration::from_secs(1);
/// One SignalR frame. The Subscribe answer late in a race can be large.
const MAX_FRAME: usize = 4 * 1024 * 1024;

/// With `--extra`: streams beyond the lamp's, to see which ones answer with
/// and without an F1TV token.
const EXTRA_STREAMS: [&str; 15] = [
    "PitStopSeries",
    "PitStop",
    "PitLaneTimeCollection",
    "TimingData",
    "TimingAppData",
    "TimingStats",
    "TyreStintSeries",
    "LapCount",
    "WeatherData",
    "TeamRadio",
    "ExtrapolatedClock",
    "OvertakeSeries",
    "DriverRaceInfo",
    "CarData.z",
    "Position.z",
];

fn main() -> anyhow::Result<()> {
    let mut minutes = None;
    let mut raw = false;
    let mut extra = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--raw" => raw = true,
            "--extra" => extra = true,
            n => {
                minutes = Some(
                    n.parse::<u64>()
                        .context("usage: f1-probe [minutes] [--raw] [--extra]")?,
                )
            }
        }
    }
    let stop_at = minutes.map(|m| Instant::now() + Duration::from_secs(m * 60));

    // Checked the same way as on the lamp; a bad token is left out.
    let auth = match std::env::var("F1TV_TOKEN") {
        Err(_) => {
            println!("F1TV token: none");
            None
        }
        Ok(pasted) => match F1tvToken::parse(&pasted) {
            Ok(token) => {
                let now = chrono::Utc::now().timestamp();
                let expires = chrono::DateTime::from_timestamp(token.expires(), 0)
                    .map_or("?".to_owned(), |t| t.to_rfc3339());
                println!(
                    "F1TV token: {:?}, expires {expires}",
                    TokenStatus::of(Some(&token), now)
                );
                token.is_usable(now).then(|| token.bearer())
            }
            Err(e) => {
                println!("F1TV token rejected ({e}); going on without");
                println!("  what arrived: {}", shape(&pasted));
                None
            }
        },
    };

    let token = negotiate(auth.as_deref())?;
    let mut ws = connect(&token, auth.as_deref())?;
    let mut capture = Capture::create()?;
    println!("saving to {}", capture.dir.display());

    // The same frames the lamp sends.
    for frame in LiveSession::opening_frames(auth.is_some()) {
        send(&mut ws, &frame)?;
    }
    println!("sent the handshake and the Subscribe calls");
    if extra {
        send(&mut ws, &signalr::subscribe(3, &EXTRA_STREAMS))?;
        println!("sent Subscribe 3: {}", EXTRA_STREAMS.join(", "));
    }

    let mut splitter = Splitter::new(MAX_FRAME);
    let mut next_ping = Instant::now() + PING_EVERY;
    loop {
        if stop_at.is_some_and(|t| Instant::now() >= t) {
            println!("time is up");
            break;
        }
        if Instant::now() >= next_ping {
            send(&mut ws, PING)?;
            next_ping = Instant::now() + PING_EVERY;
        }

        let bytes = match ws.read() {
            Ok(tungstenite::Message::Text(text)) => text.as_bytes().to_vec(),
            Ok(tungstenite::Message::Binary(bytes)) => bytes.to_vec(),
            Ok(tungstenite::Message::Close(frame)) => {
                println!("server closed the WebSocket: {frame:?}");
                break;
            }
            Ok(_) => continue, // WebSocket-level ping/pong: tungstenite answers itself
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                continue
            }
            Err(e) => return Err(e).context("reading from the feed"),
        };

        for frame in splitter.push(&bytes) {
            let frame = match frame {
                Ok(frame) => frame,
                Err(e) => {
                    println!("!! {e}");
                    continue;
                }
            };
            if raw {
                println!("<< {frame}");
            }
            if !handle(&frame, &mut capture)? {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Gets the load balancer's cookie, then a connection token (see
/// `f1_core::negotiate`). Returns the cookie as one `Cookie` header value
/// (empty without one) and the token.
fn negotiate(auth: Option<&str>) -> anyhow::Result<(String, String)> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();

    let options = agent
        .options(negotiate::NEGOTIATE_URL)
        .call()
        .context("negotiate (OPTIONS)")?;
    let set_cookies = options.headers().get_all("set-cookie");
    let cookie =
        negotiate::load_balancer_cookie(set_cookies.iter().filter_map(|v| v.to_str().ok()))
            .unwrap_or_default();
    if cookie.is_empty() {
        println!("!! no AWSALB cookie in the OPTIONS answer; trying without");
    }

    let mut request = agent
        .post(negotiate::NEGOTIATE_POST_URL)
        .header("Cookie", &cookie);
    if let Some(auth) = auth {
        request = request.header("Authorization", auth);
    }
    let mut response = request.send_empty().context("negotiate (POST)")?;
    let status = response.status();
    let body = response.body_mut().read_to_string()?;
    if !status.is_success() {
        bail!("negotiate answered {status}: {body}");
    }
    let token = negotiate::connection_token(body.as_bytes())
        .with_context(|| format!("negotiate answer: {body}"))?;
    println!("negotiated ({status})");
    Ok((cookie, token))
}

type Socket = WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

fn connect((cookie, token): &(String, String), auth: Option<&str>) -> anyhow::Result<Socket> {
    let mut request = negotiate::hub_url(token).into_client_request()?;
    if !cookie.is_empty() {
        request
            .headers_mut()
            .insert("Cookie", HeaderValue::from_str(cookie)?);
    }
    if let Some(auth) = auth {
        let mut value = HeaderValue::from_str(auth).context("token is not a valid header")?;
        value.set_sensitive(true);
        request.headers_mut().insert("Authorization", value);
    }
    // Our own TCP stream, so reads can time out and pings go out on time.
    let tcp = TcpStream::connect((HOST, 443)).context("TCP connect")?;
    tcp.set_read_timeout(Some(READ_TIMEOUT))?;
    let (ws, response) = tungstenite::client_tls(request, tcp).context("WebSocket upgrade")?;
    println!("WebSocket open ({})", response.status());
    Ok(ws)
}

fn send(ws: &mut Socket, frame: &str) -> anyhow::Result<()> {
    ws.send(tungstenite::Message::text(frame))
        .context("sending to the feed")
}

/// Prints and saves one frame. Returns false when the server closes.
fn handle(frame: &str, capture: &mut Capture) -> anyhow::Result<bool> {
    let msg = match signalr::parse(frame) {
        Ok(msg) => msg,
        Err(e) => {
            println!("!! not a SignalR message ({e}): {frame}");
            return Ok(true);
        }
    };
    match &msg {
        Message::Handshake { error: None } => println!("handshake ok"),
        Message::Handshake { error: Some(e) } => bail!("handshake refused: {e}"),
        Message::Completion { error: Some(e), .. } => bail!("Subscribe refused: {e}"),
        Message::Completion {
            invocation_id,
            result: Some(result),
            ..
        } => {
            let state = signalr::initial_state(result)?;
            println!(
                "answer to Subscribe {}: {} bytes, initial state of {} streams:",
                invocation_id.as_deref().unwrap_or("?"),
                frame.len(),
                state.len()
            );
            for (stream, data) in state {
                report(capture, &stream, data.get())?;
            }
        }
        Message::Completion { result: None, .. } => println!("Subscribe answered with no state"),
        Message::Invocation { .. } => match msg.feed_update() {
            Some(update) => report(capture, &update.stream, update.data.get())?,
            None => println!("?? {frame}"),
        },
        Message::Ping => {}
        Message::Close { error } => {
            println!("server closed the connection: {error:?}");
            return Ok(false);
        }
        Message::Other(kind) => println!("?? message type {kind}: {frame}"),
    }
    Ok(true)
}

/// Saves one stream message and prints a line about it. Our streams are also
/// run through the typed parsers, so a format change shows up here first.
fn report(capture: &mut Capture, stream: &str, json: &str) -> anyhow::Result<()> {
    let offset = capture.write(stream, json)?;
    let summary = match Stream::from_name(stream) {
        None => format!("{} bytes", json.len()),
        Some(s) => match s.parse(json) {
            Ok(messages) => messages
                .iter()
                .map(describe)
                .collect::<Vec<_>>()
                .join(" | "),
            Err(e) => format!("!! doesn't parse: {e}\n   {json}"),
        },
    };
    println!("{offset} {stream:<20} {summary}");
    Ok(())
}

fn describe(msg: &FeedMessage) -> String {
    match msg {
        FeedMessage::SessionInfo(i) => format!(
            "{} / {}",
            i.kind.as_deref().unwrap_or("?"),
            i.name.as_deref().unwrap_or("?")
        ),
        FeedMessage::Track(t) => format!(
            "{} {}",
            t.status.as_deref().unwrap_or("?"),
            t.code().map_or(String::new(), |c| format!("{c:?}"))
        ),
        FeedMessage::Session(s) => match s.status {
            Some(SessionState::Unknown) => "unknown state".to_owned(),
            Some(state) => format!("{state:?}"),
            None => "(no status)".to_owned(),
        },
        FeedMessage::RaceControl(m) => format!(
            "[{}] {}",
            m.category.as_deref().unwrap_or("?"),
            m.message.as_deref().unwrap_or("")
        ),
        FeedMessage::DriverList(d) => {
            let colours = d.values().filter(|p| p.team_colour.is_some()).count();
            format!("{} drivers, {colours} with colour", d.len())
        }
        FeedMessage::TopThree(t) => {
            let leader = t
                .lines
                .as_ref()
                .and_then(|l| l.indexed().into_iter().find(|(i, _)| *i == 0))
                .and_then(|(_, line)| line.racing_number.clone());
            match leader {
                Some(n) => format!("leader #{n}"),
                None => "update".to_owned(),
            }
        }
        FeedMessage::PitLane(p) => {
            let leaving: Vec<String> = p
                .pit_times
                .iter()
                .flat_map(|t| t.iter())
                .filter_map(|(d, t)| Some(format!("#{d} {}s", t.duration.as_deref()?)))
                .collect();
            if leaving.is_empty() {
                "update".to_owned()
            } else {
                format!("leaving the pit lane: {}", leaving.join(", "))
            }
        }
        FeedMessage::TimingStats(t) => {
            let fastest: Vec<String> = t
                .lines
                .iter()
                .flat_map(|l| l.iter())
                .filter_map(|(d, l)| {
                    let best = l.personal_best_lap_time.as_ref()?;
                    let value = best.value.as_deref().filter(|v| !v.is_empty())?;
                    (best.position == Some(1)).then(|| format!("#{d} {value}"))
                })
                .collect();
            if fastest.is_empty() {
                "update".to_owned()
            } else {
                format!("fastest lap: {}", fastest.join(", "))
            }
        }
        FeedMessage::Overtakes(o) => {
            let drivers: Vec<String> = o
                .overtakes
                .iter()
                .flat_map(|m| m.iter())
                .map(|(d, e)| format!("#{d} x{}", e.len()))
                .collect();
            format!("overtakes: {}", drivers.join(", "))
        }
    }
}

/// One `.jsonStream` file per stream, offsets counted from the probe's start.
struct Capture {
    dir: PathBuf,
    started: Instant,
    files: HashMap<String, File>,
}

impl Capture {
    fn create() -> anyhow::Result<Self> {
        let name = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("captures")
            .join(name);
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Capture {
            dir,
            started: Instant::now(),
            files: HashMap::new(),
        })
    }

    /// Appends `offset + json` to the stream's file; returns the offset.
    fn write(&mut self, stream: &str, json: &str) -> anyhow::Result<String> {
        let offset = offset(self.started.elapsed());
        if !self.files.contains_key(stream) {
            let path = self.dir.join(format!("{stream}.jsonStream"));
            let file = File::create(&path).with_context(|| format!("create {}", path.display()))?;
            self.files.insert(stream.to_owned(), file);
        }
        let file = self.files.get_mut(stream).expect("inserted above");
        writeln!(file, "{offset}{json}")?;
        Ok(offset)
    }
}

/// `HH:MM:SS.mmm`, as in the archive.
fn offset(d: Duration) -> String {
    let ms = d.as_millis();
    let s = ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        s / 3600,
        s / 60 % 60,
        s % 60,
        ms % 1000
    )
}

/// Describes a pasted token without revealing it: length, separators, and
/// how it starts, but only when that start is one of the known harmless
/// prefixes (every JWT starts with `eyJ`, an encoded cookie with `%7B`).
fn shape(pasted: &str) -> String {
    let count = |f: fn(char) -> bool| pasted.chars().filter(|c| f(*c)).count();
    let known = ["eyJ", "Bearer ", "%7B", "{\"", "\"", "loginSession"];
    let start = known
        .iter()
        .find(|k| pasted.trim_start().starts_with(*k))
        .map_or("something else".to_owned(), |k| format!("{k:?}"));
    format!(
        "{} characters, {} dots, {} spaces, {} line breaks, {} '%', starts with {start}",
        pasted.chars().count(),
        count(|c| c == '.'),
        count(|c| c == ' '),
        count(|c| c == '\n' || c == '\r'),
        count(|c| c == '%'),
    )
}
