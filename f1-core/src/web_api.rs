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

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "f1-lightbox";

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
