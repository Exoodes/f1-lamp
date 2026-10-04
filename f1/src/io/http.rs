use std::time::Duration;

use anyhow::Context;
use esp_idf_svc::http::{
    client::{Configuration, EspHttpConnection},
    Method,
};
use serde::de::DeserializeOwned;

/// Larger responses are refused, so a surprise can't exhaust the heap.
const MAX_BODY: usize = 32 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
/// How much of an error response's body goes into the error message.
const ERROR_DETAIL_MAX: usize = 256;

/// GETs `url` over HTTPS and parses the JSON body into `T`. `None` when the
/// server answers 404: OpenF1 does that for an empty result instead of `[]`.
pub fn get_json<T: DeserializeOwned>(url: &str) -> anyhow::Result<Option<T>> {
    let config = Configuration {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(TIMEOUT),
        ..Default::default()
    };

    let mut connection = EspHttpConnection::new(&config).context("create HTTP client")?;
    connection
        .initiate_request(Method::Get, url, &[("accept", "application/json")])
        .with_context(|| format!("send GET {url}"))?;
    connection
        .initiate_response()
        .with_context(|| format!("no response from {url}"))?;

    match connection.status() {
        200 => {}
        404 => return Ok(None),
        // The body usually says why, e.g. OpenF1's "Live F1 session in progress".
        status => anyhow::bail!(
            "GET {url}: HTTP {status}: {}",
            error_detail(&mut connection)
        ),
    }

    let body = read_body(&mut connection).with_context(|| format!("read body of {url}"))?;
    let value = serde_json::from_slice(&body).with_context(|| format!("parse JSON from {url}"))?;
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
