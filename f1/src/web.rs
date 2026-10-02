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
use f1_core::{command::OverrideRequest, input::Input, settings::Settings, snapshot::Snapshot};

const INDEX_HTML: &str = include_str!("../web/index.html");
const STACK_SIZE: usize = 10 * 1024;
/// Larger request bodies are refused, so a client can't exhaust the heap.
const MAX_BODY: usize = 2048;

const HTML: &str = "text/html; charset=utf-8";
const JSON: &str = "application/json";
const TEXT: &str = "text/plain; charset=utf-8";

/// Starts the web server. Handlers read the snapshot and hand inputs to the
/// render loop; they never block on it. Dropping the server stops it.
pub fn start(
    snapshot: Arc<Mutex<Snapshot>>,
    tx: SyncSender<Input>,
) -> anyhow::Result<EspHttpServer<'static>> {
    let config = Configuration {
        stack_size: STACK_SIZE,
        ..Default::default()
    };

    let mut server = EspHttpServer::new(&config)?;

    server.fn_handler::<anyhow::Error, _>("/", Method::Get, |req| {
        reply(req, 200, HTML, INDEX_HTML.as_bytes())
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/state", Method::Get, move |req| {
        // Copied out, so the lock is released before serialising.
        let state = *snapshot.lock().unwrap();
        reply(req, 200, JSON, &serde_json::to_vec(&state)?)
    })?;

    let settings_tx = tx.clone();
    server.fn_handler::<anyhow::Error, _>("/api/settings", Method::Post, move |mut req| {
        let Some(body) = read_body(&mut req)? else {
            return reply(req, 413, TEXT, b"body too large");
        };
        match serde_json::from_slice::<Settings>(&body) {
            Ok(settings) => send(req, &settings_tx, Input::Settings(settings)),
            Err(e) => {
                log::warn!("rejected settings: {e}");
                reply(req, 400, TEXT, format!("bad settings: {e}").as_bytes())
            }
        }
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/override", Method::Post, move |mut req| {
        let Some(body) = read_body(&mut req)? else {
            return reply(req, 413, TEXT, b"body too large");
        };
        match serde_json::from_slice::<OverrideRequest>(&body) {
            Ok(request) => send(req, &tx, request.into_input()),
            Err(e) => {
                log::warn!("rejected override: {e}");
                reply(req, 400, TEXT, format!("bad override: {e}").as_bytes())
            }
        }
    })?;

    log::info!("web server started");
    Ok(server)
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
fn read_body(req: &mut Request<&mut EspHttpConnection<'_>>) -> anyhow::Result<Option<Vec<u8>>> {
    let mut body = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let n = req.read(&mut chunk)?;
        if n == 0 {
            return Ok(Some(body));
        }
        if body.len() + n > MAX_BODY {
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
