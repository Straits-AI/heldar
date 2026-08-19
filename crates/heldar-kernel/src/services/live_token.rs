//! Signed, short-lived read tokens for MediaMTX live-view / playback URLs.
//!
//! The browser streams video **directly** from MediaMTX (the kernel only hands back URLs), so the
//! kernel's `can_view` check on `/liveview` protects nothing unless MediaMTX itself refuses
//! unauthenticated reads. MediaMTX is configured with HTTP external auth pointed at
//! [`crate::routes::media_auth`]; for a read it calls the kernel back, and when kernel auth is enabled
//! the kernel authorizes the read only if the URL carries a valid token minted here.
//!
//! A token is `HMAC-SHA256(key, "{path}\n{exp}")`, rendered `v1.{exp}.{b64url(sig)}` and appended as
//! `?token=…` to the stream URL by `mediamtx::ensure_live` (after `can_view` passes). The signing key
//! is process-global and random per boot: tokens expire on restart, which is harmless because the
//! dashboard re-fetches `/liveview` (and re-mints) when it (re)opens a stream.
//!
//! # KNOWN LIMIT: this token OUTLIVES the credential that minted it
//!
//! Read [`verify`]: the decision is a pure function of `(path, exp, signature)`. It names no
//! credential, and [`crate::routes::media_auth`] performs no lookup of one — MediaMTX has no session
//! to present. So revoking the API key that opened a stream, or narrowing its `scope_cameras` off
//! that camera, does NOT stop the stream. Measured (auth enabled, key soft-revoked through
//! `PATCH /api/v1/api-keys/{id}`, token replayed to `POST /internal/mediamtx-auth`): `200 OK`.
//!
//! The exposure is bounded by `HELDAR_LIVEVIEW_TOKEN_TTL_SECS`, default **3600 s**, and only for
//! transports that re-present the token per request — HLS does (playlist + each segment), so it
//! stops within the TTL. WebRTC does NOT: the token authorizes the WHEP negotiation, after which
//! media flows over the established peer connection and no further authorization is asked for at
//! all. An operator who needs revocation to bite promptly should lower the TTL; restarting the
//! kernel invalidates every outstanding token at once (the signing key is per boot).
//!
//! Closing this properly means binding the token to its principal and having the callback re-resolve
//! it, which is a change to the live plane — `ensure_live` is also driven by
//! `services::webrtc_rendezvous`, which holds a site token rather than a `Principal`, and a user
//! session that idles out mid-watch would start killing live view. It is deliberately NOT done as a
//! side effect of an audit pass. Recorded here so the next reader finds the analysis rather than
//! repeating it; the recorded-media plane (`/media/*`) has the opposite property and re-authorizes
//! every single request — see `services::media_scope::guard`.

use std::sync::OnceLock;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Process-wide random signing key, lazily initialized on first use (mint or verify). Mint and verify
/// both run in the same kernel process, so a per-process key is consistent for the life of the boot.
static KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn key() -> &'static [u8; 32] {
    KEY.get_or_init(|| {
        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        k
    })
}

fn sign(path: &str, exp_unix: i64) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key()).expect("HMAC accepts a key of any length");
    mac.update(path.as_bytes());
    mac.update(b"\n");
    mac.update(exp_unix.to_string().as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Mint a read token for `path` (the MediaMTX path, e.g. `cam_<id>`) valid until `now + ttl_secs`.
pub fn mint(path: &str, now_unix: i64, ttl_secs: i64) -> String {
    let exp = now_unix + ttl_secs.max(1);
    format!("v1.{exp}.{}", B64.encode(sign(path, exp)))
}

/// Verify a read token for `path` at wall-clock `now_unix`. Constant-time signature comparison; rejects
/// a malformed, mismatched, wrong-path, or expired token.
pub fn verify(path: &str, token: &str, now_unix: i64) -> bool {
    let mut parts = token.splitn(3, '.');
    if parts.next() != Some("v1") {
        return false;
    }
    let Some(exp) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
        return false;
    };
    let Some(sig_b64) = parts.next() else {
        return false;
    };
    if now_unix >= exp {
        return false;
    }
    let Ok(sig) = B64.decode(sig_b64) else {
        return false;
    };
    // `verify_slice` is constant-time and also checks the length.
    let mut mac = HmacSha256::new_from_slice(key()).expect("HMAC accepts a key of any length");
    mac.update(path.as_bytes());
    mac.update(b"\n");
    mac.update(exp.to_string().as_bytes());
    mac.verify_slice(&sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_within_ttl() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600);
        assert!(verify("cam_abc", &t, now));
        assert!(verify("cam_abc", &t, now + 3599));
    }

    #[test]
    fn rejects_after_expiry() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600);
        assert!(!verify("cam_abc", &t, now + 3601));
    }

    #[test]
    fn rejects_wrong_path() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600);
        assert!(!verify("cam_xyz", &t, now)); // token is path-scoped
    }

    #[test]
    fn rejects_tampered_or_malformed() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600);
        assert!(!verify("cam_abc", &format!("{t}x"), now)); // garbled sig
        assert!(!verify("cam_abc", "v1.9999999999.deadbeef", now)); // wrong sig
        assert!(!verify("cam_abc", "garbage", now));
        assert!(!verify("cam_abc", "v2.9999999999.aa", now)); // wrong version
        assert!(!verify("cam_abc", "", now));
    }
}
