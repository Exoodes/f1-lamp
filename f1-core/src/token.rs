//! The F1TV access token, which unlocks the account-only streams of the live
//! feed (pit stops, car data, ...).
//!
//! It opens a paid account, so it's a password: [`F1tvToken`] never prints
//! itself, and only [`F1tvToken::bearer`] hands it out, for the one header
//! that needs it.
//!
//! F1's server doesn't reject a bad token, it just doesn't unlock anything.
//! So the lamp checks what it can itself: the token must be a JWT whose
//! payload carries an expiry (`exp`, Unix seconds). The signature isn't
//! checked; that would take RSA and F1's public key.

use std::fmt;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

/// "Expiring soon" this long before the expiry, so there's time to renew.
pub const EXPIRING_SOON_SECS: i64 = 2 * 24 * 60 * 60;

#[derive(Clone, PartialEq, Eq)]
pub struct F1tvToken {
    jwt: String,
    expires: i64,
}

impl fmt::Debug for F1tvToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "F1tvToken(***, expires {})", self.expires)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenError {
    Empty,
    /// Not three dot-separated base64url parts.
    NotAJwt,
    /// The middle part isn't base64url-encoded JSON.
    BadPayload,
    /// The payload has no `exp`.
    NoExpiry,
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TokenError::Empty => "nothing pasted",
            TokenError::NotAJwt => "not an F1TV token (expected three parts separated by dots)",
            TokenError::BadPayload => "the token's middle part can't be read",
            TokenError::NoExpiry => "the token has no expiry date",
        })
    }
}

#[derive(Deserialize)]
struct Claims {
    exp: Option<i64>,
}

/// The `loginSession` cookie: URL-encoded JSON holding the token.
#[derive(Deserialize)]
struct LoginSession {
    data: LoginData,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginData {
    subscription_token: String,
}

impl F1tvToken {
    /// Reads what the user pasted: the token itself, `Bearer <token>`, or
    /// the whole value of formula1.com's `loginSession` cookie.
    pub fn parse(pasted: &str) -> Result<Self, TokenError> {
        let text = pasted.trim();
        if text.is_empty() {
            return Err(TokenError::Empty);
        }
        let from_cookie;
        let jwt = if text.starts_with('{') || text.starts_with("%7B") || text.starts_with("%7b") {
            let json = percent_decode(text);
            let session: LoginSession =
                serde_json::from_str(&json).map_err(|_| TokenError::NotAJwt)?;
            from_cookie = session.data.subscription_token;
            from_cookie.trim()
        } else {
            text.strip_prefix("Bearer ").unwrap_or(text).trim()
        };

        let parts: Vec<&str> = jwt.split('.').collect();
        let base64url = |p: &&str| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
        };
        if parts.len() != 3 || !parts.iter().all(base64url) {
            return Err(TokenError::NotAJwt);
        }
        let payload = URL_SAFE_NO_PAD
            .decode(parts[1].trim_end_matches('='))
            .map_err(|_| TokenError::BadPayload)?;
        let claims: Claims =
            serde_json::from_slice(&payload).map_err(|_| TokenError::BadPayload)?;
        let expires = claims.exp.ok_or(TokenError::NoExpiry)?;
        Ok(F1tvToken {
            jwt: jwt.to_owned(),
            expires,
        })
    }

    /// When it stops working, as Unix seconds.
    pub fn expires(&self) -> i64 {
        self.expires
    }

    pub fn is_usable(&self, now_unix: i64) -> bool {
        now_unix < self.expires
    }

    /// The value of the `Authorization` header. The only way to get at the
    /// token: don't log what comes out.
    pub fn bearer(&self) -> String {
        format!("Bearer {}", self.jwt)
    }

    /// For storing it on the device. Same warning as [`bearer`](Self::bearer).
    pub fn to_stored(&self) -> String {
        self.jwt.clone()
    }
}

/// What the page shows. Never contains the token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TokenStatus {
    NotConfigured,
    Valid {
        expires: i64,
    },
    ExpiringSoon {
        expires: i64,
    },
    Expired {
        expired: i64,
    },
    /// The last paste wasn't a usable token; the lamp carries on without.
    Rejected {
        reason: String,
    },
}

impl TokenStatus {
    pub fn of(token: Option<&F1tvToken>, now_unix: i64) -> Self {
        match token {
            None => TokenStatus::NotConfigured,
            Some(t) if !t.is_usable(now_unix) => TokenStatus::Expired { expired: t.expires },
            Some(t) if t.expires - now_unix <= EXPIRING_SOON_SECS => {
                TokenStatus::ExpiringSoon { expires: t.expires }
            }
            Some(t) => TokenStatus::Valid { expires: t.expires },
        }
    }
}

/// `%7B%22a%22%7D` -> `{"a"}`. Invalid escapes stay as they are.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up token: right shape, unsigned. Never a real one in tests.
    fn jwt(payload: &str) -> String {
        format!(
            "{}.{}.c2lnbmF0dXJl",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#),
            URL_SAFE_NO_PAD.encode(payload)
        )
    }

    const EXP: i64 = 1_760_000_000;

    #[test]
    fn reads_the_expiry_from_the_payload() {
        let t = F1tvToken::parse(&jwt(&format!(r#"{{"exp":{EXP},"sub":"me"}}"#))).unwrap();
        assert_eq!(t.expires(), EXP);
    }

    #[test]
    fn debug_never_shows_the_token() {
        let raw = jwt(&format!(r#"{{"exp":{EXP}}}"#));
        let t = F1tvToken::parse(&raw).unwrap();
        let shown = format!("{t:?}");
        assert!(!shown.contains(&raw[..20]), "{shown}");
        assert!(shown.contains("***"));
    }

    #[test]
    fn bearer_is_the_header_value() {
        let raw = jwt(&format!(r#"{{"exp":{EXP}}}"#));
        assert_eq!(
            F1tvToken::parse(&raw).unwrap().bearer(),
            format!("Bearer {raw}")
        );
    }

    #[test]
    fn surrounding_whitespace_and_bearer_prefix_are_accepted() {
        let raw = jwt(&format!(r#"{{"exp":{EXP}}}"#));
        let t = F1tvToken::parse(&format!("  Bearer {raw}\n")).unwrap();
        assert_eq!(t.to_stored(), raw);
    }

    #[test]
    fn whole_login_session_cookie_is_accepted() {
        let raw = jwt(&format!(r#"{{"exp":{EXP}}}"#));
        let cookie_json =
            format!(r#"{{"data":{{"subscriptionStatus":"active","subscriptionToken":"{raw}"}}}}"#);
        let encoded: String = cookie_json
            .bytes()
            .map(|b| match b {
                b'{' | b'}' | b'"' | b':' | b',' => format!("%{b:02X}"),
                _ => (b as char).to_string(),
            })
            .collect();
        assert_eq!(F1tvToken::parse(&encoded).unwrap().to_stored(), raw);
        assert_eq!(F1tvToken::parse(&cookie_json).unwrap().to_stored(), raw);
    }

    #[test]
    fn garbage_is_not_a_jwt() {
        assert_eq!(F1tvToken::parse("garbage"), Err(TokenError::NotAJwt));
        assert_eq!(F1tvToken::parse("a.b"), Err(TokenError::NotAJwt));
        assert_eq!(F1tvToken::parse("a..c"), Err(TokenError::NotAJwt));
        assert_eq!(F1tvToken::parse("a.b c.d"), Err(TokenError::NotAJwt));
    }

    #[test]
    fn empty_paste_is_empty() {
        assert_eq!(F1tvToken::parse("  \n"), Err(TokenError::Empty));
    }

    #[test]
    fn unreadable_payload_is_reported() {
        assert_eq!(
            F1tvToken::parse("aGVhZA.!!!.c2ln"),
            Err(TokenError::NotAJwt)
        );
        assert_eq!(
            F1tvToken::parse(&format!(
                "aGVhZA.{}.c2ln",
                URL_SAFE_NO_PAD.encode("not json")
            )),
            Err(TokenError::BadPayload)
        );
    }

    #[test]
    fn token_without_expiry_is_rejected() {
        assert_eq!(
            F1tvToken::parse(&jwt(r#"{"sub":"me"}"#)),
            Err(TokenError::NoExpiry)
        );
    }

    #[test]
    fn cookie_without_a_token_is_not_a_jwt() {
        assert_eq!(
            F1tvToken::parse(r#"{"data":{"other":1}}"#),
            Err(TokenError::NotAJwt)
        );
    }

    #[test]
    fn status_at_every_boundary() {
        let t = F1tvToken::parse(&jwt(&format!(r#"{{"exp":{EXP}}}"#))).unwrap();
        let soon = EXP - EXPIRING_SOON_SECS;
        assert_eq!(TokenStatus::of(None, EXP), TokenStatus::NotConfigured);
        assert_eq!(
            TokenStatus::of(Some(&t), soon - 1),
            TokenStatus::Valid { expires: EXP }
        );
        assert_eq!(
            TokenStatus::of(Some(&t), soon),
            TokenStatus::ExpiringSoon { expires: EXP }
        );
        assert_eq!(
            TokenStatus::of(Some(&t), EXP - 1),
            TokenStatus::ExpiringSoon { expires: EXP }
        );
        assert_eq!(
            TokenStatus::of(Some(&t), EXP),
            TokenStatus::Expired { expired: EXP }
        );
        assert!(!t.is_usable(EXP));
        assert!(t.is_usable(EXP - 1));
    }

    #[test]
    fn status_json_has_no_token() {
        let json = serde_json::to_string(&TokenStatus::Valid { expires: EXP }).unwrap();
        assert_eq!(json, format!(r#"{{"state":"valid","expires":{EXP}}}"#));
        let json = serde_json::to_string(&TokenStatus::Rejected {
            reason: TokenError::NotAJwt.to_string(),
        })
        .unwrap();
        assert!(json.starts_with(r#"{"state":"rejected","reason":"#));
    }

    #[test]
    fn percent_decoding_leaves_broken_escapes_alone() {
        assert_eq!(percent_decode("%7B%22a%22%7D"), r#"{"a"}"#);
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
