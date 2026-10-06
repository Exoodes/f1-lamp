//! The steps before F1's live feed WebSocket opens, without the network.
//!
//! 1. `OPTIONS` [`NEGOTIATE_URL`]: answered 405, but it sets the load
//!    balancer's cookies, which keep the WebSocket on the same server as the
//!    negotiate ([`load_balancer_cookie`]).
//! 2. `POST` [`NEGOTIATE_POST_URL`], with the cookie (and the F1TV token if
//!    there is one): answers with a connection token ([`connection_token`]).
//! 3. The WebSocket to [`hub_url`] with that token, the cookie and the token
//!    again.

use serde::Deserialize;

pub const NEGOTIATE_URL: &str = "https://livetiming.formula1.com/signalrcore/negotiate";
pub const NEGOTIATE_POST_URL: &str =
    "https://livetiming.formula1.com/signalrcore/negotiate?negotiateVersion=1";
const HUB_URL: &str = "wss://livetiming.formula1.com/signalrcore";

/// AWS's load-balancer stickiness cookies are `AWSALB` and `AWSALBCORS`
/// (the same value, the second for cross-site requests).
const LOAD_BALANCER_COOKIE: &str = "AWSALB";

/// The `Cookie` header value to send on from `Set-Cookie` header values:
/// every load-balancer cookie, as `name=value`, joined with `; `. `None`
/// when there is none; connecting may still work, on any server.
pub fn load_balancer_cookie<'a>(set_cookies: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let cookies: Vec<&str> = set_cookies
        .into_iter()
        // `name=value; Path=/; Expires=...`: only `name=value` goes back.
        .filter_map(|c| c.split(';').next())
        .map(str::trim)
        .filter(|c| c.starts_with(LOAD_BALANCER_COOKIE) && c.contains('='))
        .collect();
    (!cookies.is_empty()).then(|| cookies.join("; "))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Negotiation {
    connection_token: String,
}

/// The connection token from the negotiate POST's answer (JSON).
pub fn connection_token(body: &[u8]) -> Result<String, serde_json::Error> {
    serde_json::from_slice::<Negotiation>(body).map(|n| n.connection_token)
}

/// Where the WebSocket connects with `connection_token`.
pub fn hub_url(connection_token: &str) -> String {
    format!("{HUB_URL}?id={connection_token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_load_balancer_cookies_are_sent_without_their_attributes() {
        let cookie = load_balancer_cookie([
            "AWSALB=abc123; Expires=Tue, 13 Oct 2026 10:00:00 GMT; Path=/",
            "AWSALBCORS=abc123; Expires=Tue, 13 Oct 2026 10:00:00 GMT; Path=/; SameSite=None; Secure",
        ]);
        assert_eq!(cookie.as_deref(), Some("AWSALB=abc123; AWSALBCORS=abc123"));
    }

    #[test]
    fn one_load_balancer_cookie_is_enough() {
        let cookie = load_balancer_cookie(["AWSALBCORS=xyz; Path=/; SameSite=None"]);
        assert_eq!(cookie.as_deref(), Some("AWSALBCORS=xyz"));
    }

    #[test]
    fn other_cookies_are_left_out() {
        let cookie = load_balancer_cookie(["session=1; Path=/", "AWSALB=a; Path=/"]);
        assert_eq!(cookie.as_deref(), Some("AWSALB=a"));
    }

    #[test]
    fn no_load_balancer_cookie_is_none() {
        assert_eq!(load_balancer_cookie(["session=1"]), None);
        assert_eq!(load_balancer_cookie([]), None);
    }

    #[test]
    fn the_connection_token_is_read_from_the_answer() {
        let body = br#"{"negotiateVersion":1,"connectionId":"x","connectionToken":"tok_123","availableTransports":[]}"#;
        assert_eq!(connection_token(body).unwrap(), "tok_123");
    }

    #[test]
    fn an_answer_without_a_token_is_an_error() {
        assert!(connection_token(br#"{"error":"nope"}"#).is_err());
        assert!(connection_token(b"not json").is_err());
    }

    #[test]
    fn the_hub_url_carries_the_token() {
        assert_eq!(
            hub_url("tok_123"),
            "wss://livetiming.formula1.com/signalrcore?id=tok_123"
        );
    }

    #[test]
    fn the_post_url_is_the_negotiate_url_with_the_version() {
        assert_eq!(
            NEGOTIATE_POST_URL,
            format!("{NEGOTIATE_URL}?negotiateVersion=1")
        );
    }
}
