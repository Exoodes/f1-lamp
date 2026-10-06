//! HTTPS requests (OpenF1, the live feed's negotiate), each read whole and
//! capped in size, and the lock that keeps two TLS handshakes from running at
//! once.

use std::{sync::Mutex, time::Duration};

use anyhow::Context;
use esp_idf_svc::http::{
    client::{Configuration, EspHttpConnection},
    Method,
};
use serde::de::DeserializeOwned;

/// Larger responses are refused, so a surprise can't exhaust the heap.
const MAX_BODY: usize = 32 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
/// Room for the request line and headers. The default 512 bytes can't hold
/// `Authorization: Bearer <F1TV token>`: the token alone is 1-2 KB.
const TX_BUFFER: usize = 4096;
/// How much of an error response's body goes into the error message.
const ERROR_DETAIL_MAX: usize = 256;

/// Held during every TLS handshake. Each one needs tens of KB of heap for a
/// moment; the live feed's WebSocket and an HTTPS request doing theirs at the
/// same time could run out.
pub static TLS: Mutex<()> = Mutex::new(());

/// An HTTP response, read in full.
pub struct Reply {
    pub status: u16,
    /// The `Set-Cookie` header. With several, the client keeps the last one.
    #[cfg_attr(feature = "player", allow(dead_code))]
    pub set_cookie: Option<String>,
    pub body: Vec<u8>,
}

/// Sends a request without a body and reads the whole response, whatever
/// its status.
pub fn request(method: Method, url: &str, headers: &[(&str, &str)]) -> anyhow::Result<Reply> {
    let _tls = TLS.lock().unwrap_or_else(|e| e.into_inner());
    let config = Configuration {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(TIMEOUT),
        buffer_size_tx: Some(TX_BUFFER),
        ..Default::default()
    };

    let mut connection = EspHttpConnection::new(&config).context("create HTTP client")?;
    connection
        .initiate_request(method, url, headers)
        .with_context(|| format!("send {method:?} {url}"))?;
    connection
        .initiate_response()
        .with_context(|| format!("no response from {url}"))?;

    let status = connection.status();
    let set_cookie = connection.header("Set-Cookie").map(str::to_owned);
    let body = if (200..300).contains(&status) {
        read_body(&mut connection).with_context(|| format!("read body of {url}"))?
    } else {
        error_detail(&mut connection).into_bytes()
    };
    Ok(Reply {
        status,
        set_cookie,
        body,
    })
}

/// GETs `url` over HTTPS and parses the JSON body into `T`. `None` when the
/// server answers 404: OpenF1 does that for an empty result instead of `[]`.
pub fn get_json<T: DeserializeOwned>(url: &str) -> anyhow::Result<Option<T>> {
    let reply = request(Method::Get, url, &[("accept", "application/json")])?;
    match reply.status {
        200 => {}
        404 => return Ok(None),
        // The body usually says why, e.g. OpenF1's "Live F1 session in progress".
        status => anyhow::bail!(
            "GET {url}: HTTP {status}: {}",
            String::from_utf8_lossy(&reply.body)
        ),
    }
    let value =
        serde_json::from_slice(&reply.body).with_context(|| format!("parse JSON from {url}"))?;
    Ok(Some(value))
}

/// The start of an error response's body, for the log. Best effort: whatever
/// could be read, possibly empty.
fn error_detail(connection: &mut EspHttpConnection) -> String {
    let mut buf = [0u8; ERROR_DETAIL_MAX];
    let mut len = 0;
    while len < buf.len() {
        match connection.read(&mut buf[len..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => len += n,
        }
    }
    // Lossy: a cut through a multi-byte character becomes '�' instead of failing.
    String::from_utf8_lossy(&buf[..len]).trim().to_string()
}

fn read_body(connection: &mut EspHttpConnection) -> anyhow::Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let n = connection.read(&mut chunk)?;
        if n == 0 {
            return Ok(body);
        }
        if body.len() + n > MAX_BODY {
            anyhow::bail!("body larger than {MAX_BODY} bytes");
        }
        body.extend_from_slice(&chunk[..n]);
    }
}
