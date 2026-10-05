//! The F1TV token in flash, under its own key: never inside `Settings`,
//! which the page reads back. Shared by the web server (which sets it) and
//! the live thread (which sends it).
//!
//! Nothing here logs the token or returns it, except
//! [`TokenKeeper::bearer_if_usable`] for the one header that needs it.

use std::sync::Mutex;

use anyhow::Context;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use f1_core::token::{F1tvToken, TokenStatus};

/// NVS limits names to 15 characters.
const NAMESPACE: &str = "f1";
const KEY: &str = "f1tv_token";
/// A JWT is 1-2 KB; stored as a blob, which has no string length limit.
const MAX_STORED: usize = 4096;

struct Inner {
    nvs: EspNvs<NvsDefault>,
    token: Option<F1tvToken>,
    /// Why the last paste was refused, until the next one.
    rejected: Option<String>,
}

pub struct TokenKeeper {
    inner: Mutex<Inner>,
}

impl TokenKeeper {
    /// Opens the store and loads the token, if one was saved.
    pub fn new(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        let nvs = EspNvs::new(partition, NAMESPACE, true).context("open NVS namespace")?;
        let mut buf = vec![0u8; MAX_STORED];
        let token = match nvs.get_blob(KEY, &mut buf) {
            Ok(Some(bytes)) => match std::str::from_utf8(bytes).map(F1tvToken::parse) {
                Ok(Ok(token)) => {
                    log::info!("F1TV token loaded, expires {}", token.expires());
                    Some(token)
                }
                _ => {
                    log::warn!("stored F1TV token unreadable; ignoring it");
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                log::warn!("reading the F1TV token: {e}");
                None
            }
        };
        Ok(TokenKeeper {
            inner: Mutex::new(Inner {
                nvs,
                token,
                rejected: None,
            }),
        })
    }

    /// What the page shows. `now_unix` is `None` until the clock is set;
    /// the token then counts as valid.
    pub fn status(&self, now_unix: Option<i64>) -> TokenStatus {
        let inner = self.inner.lock().unwrap();
        if let Some(reason) = &inner.rejected {
            return TokenStatus::Rejected {
                reason: reason.clone(),
            };
        }
        TokenStatus::of(inner.token.as_ref(), now_unix.unwrap_or(0))
    }

    /// The `Authorization` header value while the token is usable. Don't log
    /// it.
    pub fn bearer_if_usable(&self, now_unix: Option<i64>) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        let token = inner.token.as_ref()?;
        token
            .is_usable(now_unix.unwrap_or(0))
            .then(|| token.bearer())
    }

    /// Takes what was pasted on the page. An empty paste removes the token;
    /// one that isn't a token removes it too, so the lamp goes on with the
    /// public streams and the page says why.
    pub fn set(&self, pasted: &str, now_unix: Option<i64>) -> anyhow::Result<TokenStatus> {
        {
            let mut inner = self.inner.lock().unwrap();
            if pasted.trim().is_empty() {
                inner.token = None;
                inner.rejected = None;
                inner.nvs.remove(KEY).context("remove F1TV token")?;
                log::info!("F1TV token removed");
            } else {
                match F1tvToken::parse(pasted) {
                    Ok(token) => {
                        inner
                            .nvs
                            .set_blob(KEY, token.to_stored().as_bytes())
                            .context("save F1TV token")?;
                        log::info!("F1TV token saved, expires {}", token.expires());
                        inner.token = Some(token);
                        inner.rejected = None;
                    }
                    Err(e) => {
                        // Only the reason is logged, never what was pasted.
                        log::warn!("F1TV token rejected: {e}");
                        inner.token = None;
                        inner.rejected = Some(e.to_string());
                        inner.nvs.remove(KEY).context("remove F1TV token")?;
                    }
                }
            }
        }
        Ok(self.status(now_unix))
    }
}
