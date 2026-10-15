//! The web server: the page, the API it uses (the README has the list), and
//! firmware updates. Every POST needs the `X-F1-Lamp` header and the lamp's own
//! name or IP address in `Host`; an upload also proves the update key
//! (`f1_core::ota_auth`).

use std::{
    sync::{
        mpsc::{SyncSender, TrySendError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use esp_idf_svc::{
    http::{
        server::{Configuration, EspHttpConnection, EspHttpServer, Request},
        Method,
    },
    io::Write,
};
use f1_core::{
    body::{read_capped, Body},
    command::OverrideRequest,
    input::Input,
    ota_auth::UploadCheck,
    schedule::Session,
    settings::Settings,
    snapshot::{Snapshot, StateReply},
    token::TokenStatus,
    web_api::{host_refusal, post_refusal, query_number, upload_len},
    weekend::{weekend, Moved, Results, ScheduleReply},
};
use serde::de::DeserializeOwned;

use crate::io::{clock, mdns, ota, racelog, system, token_store::TokenKeeper, weblog};

const INDEX_HTML: &str = include_str!("../web/index.html");
/// Larger request bodies are refused, so a client can't exhaust the heap.
const MAX_JSON_BODY: usize = 2048;
/// A pasted F1TV token, or the whole `loginSession` cookie holding one.
const MAX_TOKEN_BODY: usize = 8192;
/// The longest a request body may take: the server has a single task, which
/// a client sending a few bytes at a time would otherwise keep to itself.
const BODY_DEADLINE: Duration = Duration::from_secs(10);

/// Every POST must carry this header (any value). A page from another site
/// can't send it without the browser asking the lamp first (a CORS
/// preflight), which the lamp never allows, so other sites can't change
/// settings or upload firmware through a browser on the home network.
const PAGE_HEADER: &str = "X-F1-Lamp";
/// A firmware upload names the nonce it got from `POST /api/ota/nonce` in
/// one header and proves the update key in the others (`f1_core::ota_auth`):
/// for its length before anything is erased, and for the whole image.
const NONCE_HEADER: &str = "X-F1-Nonce";
const START_HEADER: &str = "X-F1-Start";
const AUTH_HEADER: &str = "X-F1-Auth";

const HTML: &str = "text/html; charset=utf-8";
const JSON: &str = "application/json";
const TEXT: &str = "text/plain; charset=utf-8";

/// What the web server's handlers share with the rest of the firmware.
pub struct WebState {
    /// Written by the render loop.
    pub snapshot: Arc<Mutex<Snapshot>>,
    /// The calendar for the page's weekend; set by the net thread, like
    /// `moved` and `results`.
    pub schedule: Arc<Mutex<Vec<Session>>>,
    pub moved: Arc<Mutex<Moved>>,
    pub results: Arc<Mutex<Results>>,
    /// Commands and settings for the render loop.
    pub tx: SyncSender<Input>,
    pub tokens: Arc<TokenKeeper>,
}

/// Starts the web server. Handlers read the snapshot and hand inputs to the
/// render loop; they never block on it. Dropping the server stops it.
pub fn start(state: WebState) -> anyhow::Result<EspHttpServer<'static>> {
    let WebState {
        snapshot,
        schedule,
        moved,
        results,
        tx,
        tokens,
    } = state;
    let config = Configuration {
        stack_size: crate::config::stack::WEB,
        ..Default::default()
    };

    let mut server = EspHttpServer::new(&config)?;
    page_routes(&mut server, snapshot, schedule, moved, results)?;
    command_routes(&mut server, tx)?;
    token_routes(&mut server, tokens)?;
    ota_route(&mut server)?;
    #[cfg(feature = "player")]
    replay_routes(&mut server)?;

    log::info!("web server started");
    Ok(server)
}

/// The page itself, `GET /api/state` (what the lamp shows),
/// `GET /api/schedule` (this race weekend) and `GET /api/log` (its latest log
/// lines).
fn page_routes(
    server: &mut EspHttpServer<'static>,
    snapshot: Arc<Mutex<Snapshot>>,
    schedule: Arc<Mutex<Vec<Session>>>,
    moved: Arc<Mutex<Moved>>,
    results: Arc<Mutex<Results>>,
) -> anyhow::Result<()> {
    server.fn_handler::<anyhow::Error, _>("/", Method::Get, |req| {
        reply(req, 200, HTML, INDEX_HTML.as_bytes())
    })?;

    let follow_from = Arc::clone(&snapshot);
    server.fn_handler::<anyhow::Error, _>("/api/state", Method::Get, move |req| {
        if let Some(why) = get_refusal(&req) {
            return refuse(req, why);
        }
        // Copied out, so the lock is released before serialising.
        let state = StateReply::new(*snapshot.lock().unwrap(), Instant::now());
        reply(req, 200, JSON, &serde_json::to_vec(&state)?)
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/schedule", Method::Get, move |req| {
        if let Some(why) = get_refusal(&req) {
            return refuse(req, why);
        }
        let now = clock::unix_now();
        let follow = follow_from.lock().unwrap().settings.sessions;
        let weekend = now.and_then(|now| {
            weekend(
                &schedule.lock().unwrap(),
                now,
                follow,
                &results.lock().unwrap(),
                &moved.lock().unwrap(),
            )
        });
        reply(
            req,
            200,
            JSON,
            &serde_json::to_vec(&ScheduleReply { now, weekend })?,
        )
    })?;

    // `?after=<n>`: the race log's lines from number n on (n is the last
    // answer's `next`); without it, all.
    server.fn_handler::<anyhow::Error, _>("/api/racelog", Method::Get, |req| {
        if let Some(why) = get_refusal(&req) {
            return refuse(req, why);
        }
        let after = query_number(req.uri(), "after");
        reply(req, 200, JSON, &serde_json::to_vec(&racelog::page(after))?)
    })?;

    // `?after=<n>`: the lines after number n; without it, only where to start.
    // Logs nothing itself, or every poll would add a line.
    server.fn_handler::<anyhow::Error, _>("/api/log", Method::Get, |req| {
        if let Some(why) = get_refusal(&req) {
            return refuse(req, why);
        }
        let after = query_number(req.uri(), "after");
        reply(req, 200, JSON, &serde_json::to_vec(&weblog::page(after))?)
    })?;
    Ok(())
}

/// `POST /api/settings` (the whole `Settings`) and `POST /api/override` (a
/// flag or colour by hand), both handed to the render loop.
fn command_routes(
    server: &mut EspHttpServer<'static>,
    tx: SyncSender<Input>,
) -> anyhow::Result<()> {
    let settings_tx = tx.clone();
    server.fn_handler::<anyhow::Error, _>("/api/settings", Method::Post, move |req| {
        post_json(req, "settings", |req, settings: Settings| {
            match settings.validate() {
                Ok(()) => send(req, &settings_tx, Input::Settings(settings)),
                Err(e) => {
                    log::warn!("rejected settings: {e}");
                    reply(req, 400, TEXT, format!("bad settings: {e}").as_bytes())
                }
            }
        })
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/override", Method::Post, move |req| {
        post_json(req, "override", |req, request: OverrideRequest| {
            send(req, &tx, request.into_input())
        })
    })?;
    Ok(())
}

/// `POST /api/ota/nonce` (a one-time nonce) and `POST /api/ota` (a firmware
/// image, with the nonce and its tag). In every build, replay and showcase
/// included: whatever runs must be able to receive the next firmware, or the
/// way back needs the cable.
fn ota_route(server: &mut EspHttpServer<'static>) -> anyhow::Result<()> {
    server.fn_handler::<anyhow::Error, _>("/api/ota/nonce", Method::Post, |req| {
        if let Some(why) = refusal(&req) {
            return refuse(req, why);
        }
        reply(req, 200, TEXT, ota::issue_nonce().as_bytes())
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/ota", Method::Post, |mut req| {
        if let Some(why) = refusal(&req) {
            return refuse(req, why);
        }
        // Before the nonce: a wrong size doesn't use it up.
        let len = match upload_len(req.header("Content-Length"), ota::MAX_IMAGE) {
            Ok(len) => len,
            Err((status, why)) => {
                log::warn!("ota: refused an upload: {why}");
                return reply(req, status, TEXT, why.as_bytes());
            }
        };
        let nonce = req.header(NONCE_HEADER).and_then(ota::redeem_nonce);
        let start = req.header(START_HEADER).map(str::to_owned);
        let tag = req.header(AUTH_HEADER).map(str::to_owned);
        let (Some(nonce), Some(start), Some(tag)) = (nonce, start, tag) else {
            log::warn!("ota: refused an upload without a fresh nonce and its tags");
            return reply(
                req,
                401,
                TEXT,
                b"needs a fresh nonce from /api/ota/nonce and its tags",
            );
        };
        let tags = ota::Tags {
            start: &start,
            image: &tag,
        };
        match ota::receive(&mut req, len, UploadCheck::new(nonce), tags) {
            Ok(()) => {
                // A moment for the answer to leave before the chip restarts.
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_secs(1));
                    system::restart();
                });
                reply(req, 200, TEXT, b"updated, restarting")
            }
            Err(e) => {
                log::warn!("ota: {e:#}");
                reply(req, 400, TEXT, format!("update failed: {e:#}").as_bytes())
            }
        }
    })?;
    Ok(())
}

/// `GET /api/replay` (status) and `POST /api/replay` (a command). Without a
/// replay these don't exist, so the page sees a 404 and hides its panel.
#[cfg(feature = "player")]
fn replay_routes(server: &mut EspHttpServer<'static>) -> anyhow::Result<()> {
    use f1_core::replay::ReplayCommand;

    server.fn_handler::<anyhow::Error, _>("/api/replay", Method::Get, |req| {
        if let Some(why) = get_refusal(&req) {
            return refuse(req, why);
        }
        let Some(replay) = crate::replay::handle() else {
            return reply(req, 404, TEXT, b"no replay");
        };
        // Not 404: the page keeps asking until the session has been read.
        let Some(status) = replay.status() else {
            return reply(req, 503, TEXT, b"reading the session");
        };
        let body = serde_json::json!({
            "name": replay.name(),
            "status": status,
        });
        reply(req, 200, JSON, &serde_json::to_vec(&body)?)
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/replay", Method::Post, |req| {
        post_json(req, "replay command", |req, command: ReplayCommand| {
            let Some(replay) = crate::replay::handle() else {
                return reply(req, 404, TEXT, b"no replay");
            };
            replay.send(command)?;
            reply(req, 204, TEXT, b"")
        })
    })?;
    Ok(())
}

/// `GET /api/token` gives the F1TV token's status; `POST /api/token` takes a
/// pasted token (plain text; empty removes it) and answers with the new
/// status. Neither ever returns the token, and the body is never logged.
fn token_routes(
    server: &mut EspHttpServer<'static>,
    tokens: Arc<TokenKeeper>,
) -> anyhow::Result<()> {
    let get_tokens = Arc::clone(&tokens);
    server.fn_handler::<anyhow::Error, _>("/api/token", Method::Get, move |req| {
        if let Some(why) = get_refusal(&req) {
            return refuse(req, why);
        }
        let status = get_tokens.status(clock::unix_now());
        reply(req, 200, JSON, &serde_json::to_vec(&status)?)
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/token", Method::Post, move |mut req| {
        if let Some(why) = refusal(&req) {
            return refuse(req, why);
        }
        let body = match read_capped(|buf| req.read(buf), MAX_TOKEN_BODY, None, BODY_DEADLINE)? {
            Body::Whole(body) => body,
            Body::TooLarge => return reply(req, 413, TEXT, b"too long for a token"),
            Body::TooSlow => return reply(req, 408, TEXT, b"body too slow"),
        };
        let pasted = String::from_utf8_lossy(&body);
        let status = tokens.set(&pasted, clock::unix_now())?;
        let code = if matches!(status, TokenStatus::Rejected { .. }) {
            400
        } else {
            200
        };
        reply(req, code, JSON, &serde_json::to_vec(&status)?)
    })?;
    Ok(())
}

/// What every JSON POST checks first: the page header (else 403), a body of
/// at most `MAX_JSON_BODY` bytes (else 413), within `BODY_DEADLINE` (else 408), and JSON that reads as `T` (else a
/// log line and 400, both naming `what`). Then `act` gets the request and the
/// value, and answers.
fn post_json<'r, 'c, T: DeserializeOwned>(
    mut req: Request<&'r mut EspHttpConnection<'c>>,
    what: &str,
    act: impl FnOnce(Request<&'r mut EspHttpConnection<'c>>, T) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if let Some(why) = refusal(&req) {
        return refuse(req, why);
    }
    let body = match read_capped(|buf| req.read(buf), MAX_JSON_BODY, None, BODY_DEADLINE)? {
        Body::Whole(body) => body,
        Body::TooLarge => return reply(req, 413, TEXT, b"body too large"),
        Body::TooSlow => return reply(req, 408, TEXT, b"body too slow"),
    };
    match serde_json::from_slice::<T>(&body) {
        Ok(value) => act(req, value),
        Err(e) => {
            log::warn!("rejected {what}: {e}");
            reply(req, 400, TEXT, format!("bad {what}: {e}").as_bytes())
        }
    }
}

/// Why a POST is refused, if it is (`web_api::post_refusal`).
fn refusal(req: &Request<&mut EspHttpConnection<'_>>) -> Option<&'static str> {
    post_refusal(
        req.header(PAGE_HEADER).is_some(),
        req.header("Host"),
        mdns::HOSTNAME,
    )
}

/// Why a GET of the API is refused, if it is (`web_api::host_refusal`): the
/// page itself is open to any `Host`, the data it reads is not.
fn get_refusal(req: &Request<&mut EspHttpConnection<'_>>) -> Option<&'static str> {
    host_refusal(req.header("Host"), mdns::HOSTNAME)
}

fn refuse(req: Request<&mut EspHttpConnection<'_>>, why: &str) -> anyhow::Result<()> {
    log::warn!("refused {:?} {}: {why}", req.method(), req.uri());
    reply(req, 403, TEXT, why.as_bytes())
}

/// Sends a complete response with the given status, content type and body.
fn reply(
    req: Request<&mut EspHttpConnection<'_>>,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    req.into_response(status, None, &[("Content-Type", content_type)])?
        .write_all(body)?;
    Ok(())
}

/// Hands an input to the render loop without blocking the server task.
fn send(
    req: Request<&mut EspHttpConnection<'_>>,
    tx: &SyncSender<Input>,
    input: Input,
) -> anyhow::Result<()> {
    match tx.try_send(input) {
        Ok(()) => reply(req, 204, TEXT, b""),
        Err(TrySendError::Full(_)) => reply(req, 503, TEXT, b"busy, try again"),
        Err(TrySendError::Disconnected(_)) => reply(req, 500, TEXT, b"render loop stopped"),
    }
}
