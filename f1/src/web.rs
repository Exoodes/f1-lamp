//! The web server: the page, the API it uses (the README has the list), and
//! firmware updates. Every POST needs the `X-F1-Lamp` header; an upload also
//! needs the update key.

use std::sync::{
    mpsc::{SyncSender, TrySendError},
    Arc, Mutex,
};

use esp_idf_svc::{
    http::{
        server::{Configuration, EspHttpConnection, EspHttpServer, Request},
        Method,
    },
    io::Write,
};
use f1_core::{
    command::OverrideRequest, input::Input, settings::Settings, snapshot::Snapshot,
    token::TokenStatus,
};
use serde::de::DeserializeOwned;

use crate::io::{clock, ota, token_store::TokenKeeper, weblog};

const INDEX_HTML: &str = include_str!("../web/index.html");
/// Larger request bodies are refused, so a client can't exhaust the heap.
const MAX_BODY: usize = 2048;
/// A pasted F1TV token, or the whole `loginSession` cookie holding one.
const MAX_TOKEN_BODY: usize = 8192;

/// Every POST must carry this header (any value). A page from another site
/// can't send it without the browser asking the lamp first (a CORS
/// preflight), which the lamp never allows, so other sites can't change
/// settings or upload firmware through a browser on the home network.
const PAGE_HEADER: &str = "X-F1-Lamp";
/// A firmware upload must also carry `ota_key` from cfg.toml in this header.
const KEY_HEADER: &str = "X-F1-Key";
const OTA_KEY: &str = env!("OTA_KEY");

const HTML: &str = "text/html; charset=utf-8";
const JSON: &str = "application/json";
const TEXT: &str = "text/plain; charset=utf-8";

/// Starts the web server. Handlers read the snapshot and hand inputs to the
/// render loop; they never block on it. Dropping the server stops it.
pub fn start(
    snapshot: Arc<Mutex<Snapshot>>,
    tx: SyncSender<Input>,
    tokens: Arc<TokenKeeper>,
) -> anyhow::Result<EspHttpServer<'static>> {
    let config = Configuration {
        stack_size: crate::config::stack::WEB,
        ..Default::default()
    };

    let mut server = EspHttpServer::new(&config)?;
    page_routes(&mut server, snapshot)?;
    command_routes(&mut server, tx)?;
    token_routes(&mut server, tokens)?;
    ota_route(&mut server)?;
    #[cfg(feature = "player")]
    replay_routes(&mut server)?;

    log::info!("web server started");
    Ok(server)
}

/// The page itself, `GET /api/state` (what the lamp shows) and
/// `GET /api/log` (its latest log lines).
fn page_routes(
    server: &mut EspHttpServer<'static>,
    snapshot: Arc<Mutex<Snapshot>>,
) -> anyhow::Result<()> {
    server.fn_handler::<anyhow::Error, _>("/", Method::Get, |req| {
        reply(req, 200, HTML, INDEX_HTML.as_bytes())
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/state", Method::Get, move |req| {
        // Copied out, so the lock is released before serialising.
        let state = *snapshot.lock().unwrap();
        reply(req, 200, JSON, &serde_json::to_vec(&state)?)
    })?;

    // `?after=<n>`: the lines after number n; without it, only where to start.
    // Logs nothing itself, or every poll would add a line.
    server.fn_handler::<anyhow::Error, _>("/api/log", Method::Get, |req| {
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
            send(req, &settings_tx, Input::Settings(settings))
        })
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/override", Method::Post, move |req| {
        post_json(req, "override", |req, request: OverrideRequest| {
            send(req, &tx, request.into_input())
        })
    })?;
    Ok(())
}

/// `POST /api/ota`: a firmware image, with the update key. In every build,
/// replay and showcase included: whatever runs must be able to receive the
/// next firmware, or the way back needs the cable.
fn ota_route(server: &mut EspHttpServer<'static>) -> anyhow::Result<()> {
    server.fn_handler::<anyhow::Error, _>("/api/ota", Method::Post, |mut req| {
        if !from_page(&req) {
            return refuse(req);
        }
        if !req.header(KEY_HEADER).is_some_and(is_ota_key) {
            log::warn!("ota: refused an upload without the right key");
            return reply(req, 401, TEXT, b"wrong or missing update key");
        }
        let Some(len) = req
            .header("Content-Length")
            .and_then(|l| l.parse::<usize>().ok())
        else {
            return reply(req, 411, TEXT, b"Content-Length needed");
        };
        match ota::receive(&mut req, len) {
            Ok(()) => {
                reply(req, 200, TEXT, b"updated, restarting")?;
                // A moment for the answer to leave before the chip restarts.
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    crate::restart();
                });
                Ok(())
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
        let status = get_tokens.status(clock::unix_now());
        reply(req, 200, JSON, &serde_json::to_vec(&status)?)
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/token", Method::Post, move |mut req| {
        if !from_page(&req) {
            return refuse(req);
        }
        let Some(body) = read_body(&mut req, MAX_TOKEN_BODY)? else {
            return reply(req, 413, TEXT, b"too long for a token");
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

/// `name`'s value in the query string, if it's a number: `/x?after=12` -> 12.
fn query_number(uri: &str, name: &str) -> Option<u64> {
    let (_, query) = uri.split_once('?')?;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| value.parse().ok())
}

/// What every JSON POST checks first: the page header (else 403), a body of
/// at most `MAX_BODY` bytes (else 413), and JSON that reads as `T` (else a
/// log line and 400, both naming `what`). Then `act` gets the request and the
/// value, and answers.
fn post_json<'r, 'c, T: DeserializeOwned>(
    mut req: Request<&'r mut EspHttpConnection<'c>>,
    what: &str,
    act: impl FnOnce(Request<&'r mut EspHttpConnection<'c>>, T) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if !from_page(&req) {
        return refuse(req);
    }
    let Some(body) = read_body(&mut req, MAX_BODY)? else {
        return reply(req, 413, TEXT, b"body too large");
    };
    match serde_json::from_slice::<T>(&body) {
        Ok(value) => act(req, value),
        Err(e) => {
            log::warn!("rejected {what}: {e}");
            reply(req, 400, TEXT, format!("bad {what}: {e}").as_bytes())
        }
    }
}

/// Whether a POST carries `PAGE_HEADER`, as the lamp's own page and ota.ps1 do.
fn from_page(req: &Request<&mut EspHttpConnection<'_>>) -> bool {
    req.header(PAGE_HEADER).is_some()
}

fn refuse(req: Request<&mut EspHttpConnection<'_>>) -> anyhow::Result<()> {
    reply(req, 403, TEXT, b"missing X-F1-Lamp header")
}

/// Compares every byte whatever the first difference, so the time taken
/// doesn't tell how much of a guess was right.
fn is_ota_key(given: &str) -> bool {
    given.len() == OTA_KEY.len()
        && given
            .bytes()
            .zip(OTA_KEY.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
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

/// The request body, or `None` if it is larger than `MAX_BODY`.
fn read_body(
    req: &mut Request<&mut EspHttpConnection<'_>>,
    max: usize,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut body = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let n = req.read(&mut chunk)?;
        if n == 0 {
            return Ok(Some(body));
        }
        if body.len() + n > max {
            return Ok(None);
        }
        body.extend_from_slice(&chunk[..n]);
    }
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
