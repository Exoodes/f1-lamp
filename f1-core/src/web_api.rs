//! Rules for requests to the lamp's web server that need no hardware.

use std::net::{Ipv4Addr, Ipv6Addr};

/// Whether `host` (a request's `Host` header) names the lamp: its mDNS name
/// `<hostname>.local`, the bare `hostname`, or an IP address, with or without
/// port 80. A page that a DNS-rebinding attack points at the lamp can send
/// any header but this one carries its own domain, so other names are
/// refused, even ones that start with the lamp's name.
pub fn is_lamp_host(host: &str, hostname: &str) -> bool {
    let name = host.strip_suffix(":80").unwrap_or(host);
    let bracketed_v6 = name
        .strip_prefix('[')
        .and_then(|n| n.strip_suffix(']'))
        .is_some_and(|n| n.parse::<Ipv6Addr>().is_ok());
    name.eq_ignore_ascii_case(hostname)
        || name
            .strip_suffix(".local")
            .is_some_and(|n| n.eq_ignore_ascii_case(hostname))
        || name.parse::<Ipv4Addr>().is_ok()
        || bracketed_v6
}

/// Why a request to the API is refused, if it is: its `Host` must name the
/// lamp (see [`is_lamp_host`]). GETs check only this: a page on another
/// domain can't read the answers, even through DNS rebinding.
pub fn host_refusal(host: Option<&str>, hostname: &str) -> Option<&'static str> {
    if host.is_some_and(|host| is_lamp_host(host, hostname)) {
        None
    } else {
        Some("unknown Host: use f1-lightbox.local or the lamp's IP address")
    }
}

/// Why a POST is refused, if it is: it must carry the page's own header
/// (`X-F1-Lamp`), as the lamp's page and ota.ps1 do, and pass
/// [`host_refusal`].
pub fn post_refusal(
    has_page_header: bool,
    host: Option<&str>,
    hostname: &str,
) -> Option<&'static str> {
    if !has_page_header {
        return Some("missing X-F1-Lamp header");
    }
    host_refusal(host, hostname)
}

/// A firmware upload's length from its `Content-Length`, or why it's
/// refused, with the status to answer: 411 without one, 400 for one that
/// isn't a number or is 0, 413 above `max` (an app slot).
pub fn upload_len(content_length: Option<&str>, max: usize) -> Result<usize, (u16, String)> {
    let header = content_length.ok_or((411, "Content-Length needed".to_owned()))?;
    let len: usize = header
        .trim()
        .parse()
        .map_err(|_| (400, format!("bad Content-Length: {header}")))?;
    match len {
        0 => Err((400, "an empty firmware image".to_owned())),
        len if len > max => Err((
            413,
            format!("{len} bytes: a firmware image must be at most {max} bytes"),
        )),
        len => Ok(len),
    }
}

/// `name`'s value in the query string, if it's a number: `/x?after=12` -> 12.
pub fn query_number(uri: &str, name: &str) -> Option<u64> {
    let (_, query) = uri.split_once('?')?;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "f1-lightbox";

    #[test]
    fn a_post_from_the_page_to_the_lamp_is_taken() {
        assert_eq!(post_refusal(true, Some("f1-lightbox.local"), NAME), None);
    }

    #[test]
    fn a_post_without_the_page_header_is_refused() {
        assert_eq!(
            post_refusal(false, Some("f1-lightbox.local"), NAME),
            Some("missing X-F1-Lamp header")
        );
    }

    #[test]
    fn a_post_to_another_host_or_without_one_is_refused() {
        for host in [Some("evil.example"), None] {
            let why = post_refusal(true, host, NAME).unwrap();
            assert!(why.starts_with("unknown Host"), "{host:?}: {why}");
        }
    }

    #[test]
    fn a_get_needs_only_the_lamp_as_host() {
        assert_eq!(host_refusal(Some("192.168.1.54"), NAME), None);
        for host in [Some("x.example"), None] {
            let why = host_refusal(host, NAME).unwrap();
            assert!(why.starts_with("unknown Host"), "{host:?}: {why}");
        }
    }

    #[test]
    fn an_upload_length_up_to_the_slot_is_taken() {
        assert_eq!(upload_len(Some("1753136"), 0x1F_0000), Ok(1_753_136));
        assert_eq!(upload_len(Some("2031616"), 0x1F_0000), Ok(0x1F_0000));
    }

    #[test]
    fn a_missing_empty_broken_or_too_long_upload_length_is_refused() {
        let status = |header| upload_len(header, 0x1F_0000).unwrap_err().0;
        assert_eq!(status(None), 411);
        assert_eq!(status(Some("abc")), 400);
        assert_eq!(status(Some("-1")), 400);
        assert_eq!(status(Some("0")), 400);
        assert_eq!(status(Some("2031617")), 413);
    }

    #[test]
    fn query_number_reads_the_named_number() {
        assert_eq!(query_number("/api/log?after=12", "after"), Some(12));
        assert_eq!(query_number("/api/log?x=1&after=7", "after"), Some(7));
    }

    #[test]
    fn query_number_is_none_when_missing_or_not_a_number() {
        assert_eq!(query_number("/api/log", "after"), None);
        assert_eq!(query_number("/api/log?before=3", "after"), None);
        assert_eq!(query_number("/api/log?after=", "after"), None);
        assert_eq!(query_number("/api/log?after=-1", "after"), None);
        assert_eq!(query_number("/api/log?after=abc", "after"), None);
        assert_eq!(query_number("/api/log?after", "after"), None);
    }

    #[test]
    fn the_mdns_name_and_the_bare_name_are_the_lamp() {
        assert!(is_lamp_host("f1-lightbox.local", NAME));
        assert!(is_lamp_host("F1-Lightbox.local", NAME));
        assert!(is_lamp_host("f1-lightbox", NAME));
        assert!(is_lamp_host("f1-lightbox.local:80", NAME));
    }

    #[test]
    fn ip_addresses_are_the_lamp() {
        assert!(is_lamp_host("192.168.1.54", NAME));
        assert!(is_lamp_host("192.168.1.54:80", NAME));
        assert!(is_lamp_host("[fe80::1]", NAME));
        assert!(is_lamp_host("[fe80::1]:80", NAME));
    }

    #[test]
    fn other_domains_are_refused_even_with_the_lamps_name_in_them() {
        for host in [
            "evil.com",
            "f1-lightbox.evil.com",
            "f1-lightbox.local.evil.com",
            "evil.com.local",
            "f1-lightbox.fritz.box",
        ] {
            assert!(!is_lamp_host(host, NAME), "{host}");
        }
    }

    #[test]
    fn other_ports_and_broken_addresses_are_refused() {
        assert!(!is_lamp_host("f1-lightbox.local:8080", NAME));
        assert!(!is_lamp_host("192.168.1.54:8080", NAME));
        assert!(!is_lamp_host("192.168.1", NAME));
        assert!(!is_lamp_host("fe80::1", NAME));
        assert!(!is_lamp_host("", NAME));
    }
}
