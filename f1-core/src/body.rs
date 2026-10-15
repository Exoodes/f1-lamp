//! Reading a whole HTTP body, capped in size and time: the firmware's HTTPS
//! responses and the web server's requests. Reads through a closure, so it
//! needs no connection type and runs in tests.

use std::time::{Duration, Instant};

/// A body read to its end, or why it wasn't.
#[derive(Debug, PartialEq, Eq)]
pub enum Body {
    Whole(Vec<u8>),
    TooLarge,
    TooSlow,
}

/// Reads with `read` (like `Read::read`: 0 bytes means the end) to the end,
/// refusing more than `max` bytes. `expected` (a Content-Length) sizes the
/// buffer in one go: a growing buffer briefly needs twice its size in one
/// block. The connection's timeout only limits each read, so a peer sending
/// a few bytes at a time is stopped by `within`. The deadline is checked
/// between reads, so the last read can end up to one connection timeout
/// later.
pub fn read_capped<E>(
    mut read: impl FnMut(&mut [u8]) -> Result<usize, E>,
    max: usize,
    expected: Option<usize>,
    within: Duration,
) -> Result<Body, E> {
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
        let n = read(&mut chunk)?;
        if n == 0 {
            return Ok(Body::Whole(body));
        }
        if body.len() + n > max {
            return Ok(Body::TooLarge);
        }
        body.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(60);

    /// A reader handing out `data` in pieces of at most `piece` bytes.
    fn reader(data: &[u8], piece: usize) -> impl FnMut(&mut [u8]) -> Result<usize, ()> + '_ {
        let mut at = 0;
        move |buf| {
            let n = piece.min(buf.len()).min(data.len() - at);
            buf[..n].copy_from_slice(&data[at..at + n]);
            at += n;
            Ok(n)
        }
    }

    #[test]
    fn a_body_in_pieces_is_read_whole() {
        let data: Vec<u8> = (0..=255).cycle().take(2000).collect();
        assert_eq!(
            read_capped(reader(&data, 300), 4096, None, LONG),
            Ok(Body::Whole(data))
        );
    }

    #[test]
    fn a_body_of_exactly_max_bytes_is_taken() {
        let data = [7u8; 100];
        assert_eq!(
            read_capped(reader(&data, 64), 100, Some(100), LONG),
            Ok(Body::Whole(data.to_vec()))
        );
    }

    #[test]
    fn one_byte_over_max_is_too_large() {
        let data = [7u8; 101];
        assert_eq!(
            read_capped(reader(&data, 64), 100, None, LONG),
            Ok(Body::TooLarge)
        );
    }

    #[test]
    fn an_announced_length_over_max_is_refused_before_reading() {
        let never = |_: &mut [u8]| -> Result<usize, ()> { panic!("read") };
        assert_eq!(read_capped(never, 100, Some(101), LONG), Ok(Body::TooLarge));
    }

    #[test]
    fn a_body_past_the_deadline_is_too_slow() {
        let data = [7u8; 10];
        assert_eq!(
            read_capped(reader(&data, 1), 100, None, Duration::ZERO),
            Ok(Body::TooSlow)
        );
    }

    #[test]
    fn a_read_error_is_passed_on() {
        let failing = |_: &mut [u8]| -> Result<usize, &str> { Err("reset") };
        assert_eq!(read_capped(failing, 100, None, LONG), Err("reset"));
    }
}
