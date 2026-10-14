//! Proof that a firmware upload comes from someone with the update key,
//! without the key crossing the network: the lamp hands out a one-time
//! nonce ([`Nonces`]), the uploader sends HMAC-SHA256(key, nonce ‖
//! SHA-256(image)), and the lamp checks it once the whole image has arrived
//! ([`UploadCheck`]), before the image can boot. A recorded upload can't be
//! sent again, and an image swapped on the way doesn't match.
//!
//! Writing the image erases the spare app slot, the way back from a bad
//! update, so the uploader first proves the key for this upload's length too:
//! HMAC-SHA256(key, nonce ‖ "start" ‖ the length in decimal), checked before
//! anything is erased ([`UploadCheck::may_start`]).

use std::time::{Duration, Instant};

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

pub const NONCE_LEN: usize = 16;
/// A nonce older than this is refused: an upload starts right after asking.
pub const NONCE_TTL: Duration = Duration::from_secs(120);

/// The nonce handed out last. Asking again replaces it, and an upload uses it
/// up whether its check passes or not, so each one gets a single try.
#[derive(Debug, Default)]
pub struct Nonces {
    issued: Option<([u8; NONCE_LEN], Instant)>,
}

impl Nonces {
    pub const fn new() -> Self {
        Nonces { issued: None }
    }

    /// Hands out `random` as the new nonce, in hex.
    pub fn issue(&mut self, random: [u8; NONCE_LEN], now: Instant) -> String {
        self.issued = Some((random, now));
        to_hex(&random)
    }

    /// The nonce for one upload, if `hex` is the one handed out and still
    /// fresh.
    pub fn redeem(&mut self, hex: &str, now: Instant) -> Option<[u8; NONCE_LEN]> {
        let (nonce, issued) = self.issued.take()?;
        let fresh = now.saturating_duration_since(issued) < NONCE_TTL;
        (fresh && from_hex(hex).as_deref() == Some(&nonce[..])).then_some(nonce)
    }
}

/// Hashes an upload as it arrives, then checks its tag.
pub struct UploadCheck {
    nonce: [u8; NONCE_LEN],
    image: Sha256,
}

impl UploadCheck {
    pub fn new(nonce: [u8; NONCE_LEN]) -> Self {
        UploadCheck {
            nonce,
            image: Sha256::new(),
        }
    }

    /// Whether `tag_hex` is the start tag for a `len`-byte upload: the key
    /// is right, so the spare slot may be erased.
    pub fn may_start(&self, key: &str, len: usize, tag_hex: &str) -> bool {
        let Some(tag) = from_hex(tag_hex) else {
            return false;
        };
        start_mac(key, &self.nonce, len).verify_slice(&tag).is_ok()
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.image.update(chunk);
    }

    /// Whether `tag_hex` is HMAC-SHA256(`key`, nonce ‖ SHA-256(image)),
    /// compared in constant time.
    pub fn verify(self, key: &str, tag_hex: &str) -> bool {
        let Some(tag) = from_hex(tag_hex) else {
            return false;
        };
        let mut mac = mac(key);
        mac.update(&self.nonce);
        mac.update(&self.image.finalize());
        mac.verify_slice(&tag).is_ok()
    }
}

/// What the uploader computes; the lamp only checks.
pub fn tag(key: &str, nonce: &[u8], image: &[u8]) -> String {
    let mut mac = mac(key);
    mac.update(nonce);
    mac.update(&Sha256::digest(image));
    to_hex(&mac.finalize().into_bytes())
}

/// The start tag the uploader computes; see [`UploadCheck::may_start`].
pub fn start_tag(key: &str, nonce: &[u8], len: usize) -> String {
    to_hex(&start_mac(key, nonce, len).finalize().into_bytes())
}

fn start_mac(key: &str, nonce: &[u8], len: usize) -> Hmac<Sha256> {
    let mut mac = mac(key);
    mac.update(nonce);
    mac.update(b"start");
    mac.update(len.to_string().as_bytes());
    mac
}

fn mac(key: &str) -> Hmac<Sha256> {
    // HMAC takes a key of any length.
    Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("any key length")
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "a-key-of-at-least-16";
    const IMAGE: &[u8] = b"\xe9 pretend this is firmware";
    const RANDOM: [u8; NONCE_LEN] = [7; NONCE_LEN];

    fn upload(nonce: [u8; NONCE_LEN], image: &[u8]) -> UploadCheck {
        let mut check = UploadCheck::new(nonce);
        for chunk in image.chunks(5) {
            check.update(chunk);
        }
        check
    }

    #[test]
    fn the_right_tag_passes() {
        let tag = tag(KEY, &RANDOM, IMAGE);
        assert!(upload(RANDOM, IMAGE).verify(KEY, &tag));
        assert!(upload(RANDOM, IMAGE).verify(KEY, &tag.to_uppercase()));
    }

    #[test]
    fn a_wrong_key_another_nonce_or_another_image_fails() {
        let tag = tag(KEY, &RANDOM, IMAGE);
        assert!(!upload(RANDOM, IMAGE).verify("another-key-entirely", &tag));
        assert!(!upload([8; NONCE_LEN], IMAGE).verify(KEY, &tag));
        assert!(!upload(RANDOM, b"\xe9 something else").verify(KEY, &tag));
    }

    #[test]
    fn a_tag_that_isnt_hex_fails() {
        for tag in ["", "zz", "abc", "é0"] {
            assert!(!upload(RANDOM, IMAGE).verify(KEY, tag), "{tag:?}");
        }
    }

    #[test]
    fn the_tag_is_what_ota_ps1_computes() {
        // From Python's hmac/hashlib, the same formula as ota.ps1.
        assert_eq!(
            tag(KEY, &RANDOM, IMAGE),
            "9ed4f6c97248300ecb92d6dcaf5113719cc258beb77726678f3170bc7a5083b5"
        );
    }

    #[test]
    fn the_right_start_tag_lets_the_upload_start() {
        let tag = start_tag(KEY, &RANDOM, IMAGE.len());
        assert!(UploadCheck::new(RANDOM).may_start(KEY, IMAGE.len(), &tag));
    }

    #[test]
    fn a_wrong_key_nonce_or_length_doesnt_start_the_upload() {
        let tag = start_tag(KEY, &RANDOM, IMAGE.len());
        let check = UploadCheck::new(RANDOM);
        assert!(!check.may_start("another-key-entirely", IMAGE.len(), &tag));
        assert!(!check.may_start(KEY, IMAGE.len() + 1, &tag));
        assert!(!UploadCheck::new([8; NONCE_LEN]).may_start(KEY, IMAGE.len(), &tag));
        assert!(!check.may_start(KEY, IMAGE.len(), "zz"));
    }

    #[test]
    fn the_image_tag_doesnt_start_an_upload() {
        let tag = tag(KEY, &RANDOM, IMAGE);
        assert!(!UploadCheck::new(RANDOM).may_start(KEY, IMAGE.len(), &tag));
    }

    #[test]
    fn the_start_tag_is_what_ota_ps1_computes() {
        // From Python's hmac/hashlib: nonce + b"start" + b"26".
        assert_eq!(
            start_tag(KEY, &RANDOM, IMAGE.len()),
            "c2dd625df5bec31c17647e824f1b2edf35f91e1e404015b9e4dba445356e4b67"
        );
    }

    #[test]
    fn a_nonce_works_once() {
        let t0 = Instant::now();
        let mut nonces = Nonces::default();
        let hex = nonces.issue(RANDOM, t0);
        assert_eq!(nonces.redeem(&hex, t0), Some(RANDOM));
        assert_eq!(nonces.redeem(&hex, t0), None);
    }

    #[test]
    fn a_wrong_guess_uses_the_nonce_up() {
        let t0 = Instant::now();
        let mut nonces = Nonces::default();
        let hex = nonces.issue(RANDOM, t0);
        assert_eq!(nonces.redeem("00", t0), None);
        assert_eq!(nonces.redeem(&hex, t0), None);
    }

    #[test]
    fn a_stale_nonce_is_refused() {
        let t0 = Instant::now();
        let mut nonces = Nonces::default();
        let hex = nonces.issue(RANDOM, t0);
        assert_eq!(nonces.redeem(&hex, t0 + NONCE_TTL), None);
    }

    #[test]
    fn asking_again_replaces_the_nonce() {
        let t0 = Instant::now();
        let mut nonces = Nonces::default();
        let old = nonces.issue(RANDOM, t0);
        let new = nonces.issue([9; NONCE_LEN], t0);
        assert_eq!(nonces.redeem(&old, t0), None);
        nonces.issue([9; NONCE_LEN], t0);
        assert_eq!(nonces.redeem(&new, t0), Some([9; NONCE_LEN]));
    }

    #[test]
    fn nothing_issued_redeems_nothing() {
        assert_eq!(Nonces::default().redeem("00", Instant::now()), None);
    }
}
