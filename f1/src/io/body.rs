//! Reading a whole HTTP body, capped in size and time: the client's
//! responses (`http.rs`) and the web server's requests (`web.rs`).

use std::time::{Duration, Instant};

use esp_idf_svc::io::Read;

/// A body read to its end, or why it wasn't.
pub enum Body {
    Whole(Vec<u8>),
    TooLarge,
    TooSlow,
}

/// Reads `reader` to its end, refusing more than `max` bytes. `expected` (a
/// Content-Length) sizes the buffer in one go: a growing buffer briefly needs
/// twice its size in one block. The connection's timeout only limits each
/// read, so a peer sending a few bytes at a time is stopped by `within`.
pub fn read_capped<R: Read>(
    reader: &mut R,
    max: usize,
    expected: Option<usize>,
    within: Duration,
) -> Result<Body, R::Error> {
    if expected.is_some_and(|len| len > max) {
        return Ok(Body::TooLarge);
    }
    let deadline = Instant::now() + within;
    let mut body = Vec::with_capacity(expected.unwrap_or(0));
    let mut chunk = [0u8; 512];
    loop {
        if Instant::now() >= deadline {
            return Ok(Body::TooSlow);
        }
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            return Ok(Body::Whole(body));
        }
        if body.len() + n > max {
            return Ok(Body::TooLarge);
        }
        body.extend_from_slice(&chunk[..n]);
    }
}
