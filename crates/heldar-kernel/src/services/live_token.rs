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
//! # The token names its SUBJECT, so it can be withdrawn
//!
//! A token used to be a pure function of `(path, exp, signature)`. It named no credential and
//! [`crate::routes::media_auth`] looked none up — MediaMTX presents a stream URL, not a session — so
//! revoking the key that opened a stream, or narrowing its `scope_cameras` off that camera, did not
//! stop it. Measured at the time: key soft-revoked, token replayed, `200 OK`.
//!
//! A `v2` token carries a [`Subject`] inside the signed payload, and the callback re-resolves it on
//! every read. Revocation, deactivation, deletion, expiry, and losing the camera from `scope_cameras`
//! all stop the stream.
//!
//! ## What this does and does not bound
//!
//! It bounds a transport to the rate at which that transport RE-PRESENTS the token. HLS re-presents
//! per playlist and per segment, so a withdrawn credential stops within seconds. **WebRTC does not**:
//! the token authorizes the WHEP negotiation, after which media flows over the established peer
//! connection and MediaMTX asks for nothing further. Closing that needs the peer connection itself
//! torn down, which lives in MediaMTX rather than here. So: revocation now bites HLS promptly, and
//! WebRTC only at the next negotiation. That is strictly better than the TTL-bounded behaviour it
//! replaces, and it is not complete — do not read "the token is bound" as "the stream stops".
//!
//! ## Why some subjects are never withdrawn
//!
//! - [`Subject::Site`] — the WebRTC rendezvous drives `ensure_live` holding a site token, not a
//!   `Principal`. There is no per-camera credential to re-check, and a remote viewer's stream must
//!   not die because some unrelated key was revoked.
//! - [`Subject::User`] — checked against `users.active` ONLY, never sessions. Sessions end on logout,
//!   idle timeout and TTL, none of which means "compromised"; killing an operator's live view because
//!   their session lapsed mid-watch is a false deny with no upside. Deactivating the user IS the
//!   operator act, and that stops it. Same rule, same reasoning, as the backup creator re-check.
//! - A database read failure — allowed, loudly. The recorder shares this SQLite, and turning a busy
//!   timeout into a black video wall is worse than the exposure it would prevent.

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

/// Who a token was minted for, so the read can be withdrawn when they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// An API key, by id. Re-resolved on every read: revoked/inactive/expired/deleted, or no longer
    /// scoped to this camera, all stop the stream.
    ApiKey(String),
    /// A user, by id. Checked against `users.active` only — never sessions.
    User(String),
    /// No withdrawable credential: auth disabled, or the site-token rendezvous path.
    Site,
}

impl Subject {
    fn tag(&self) -> &'static str {
        match self {
            Subject::ApiKey(_) => "k",
            Subject::User(_) => "u",
            Subject::Site => "s",
        }
    }
    fn id(&self) -> &str {
        match self {
            Subject::ApiKey(id) | Subject::User(id) => id,
            Subject::Site => "",
        }
    }
    /// The subject a principal streams as.
    pub fn of(principal: &crate::auth::Principal) -> Subject {
        match principal.kind {
            crate::auth::PrincipalKind::ApiKey => Subject::ApiKey(principal.id.clone()),
            crate::auth::PrincipalKind::User => Subject::User(principal.id.clone()),
            crate::auth::PrincipalKind::System => Subject::Site,
        }
    }
}

/// The subject is INSIDE the signed payload, not merely alongside it — otherwise a holder could swap
/// `k.<their-revoked-key>` for `s.` and mint themselves an unwithdrawable stream.
fn sign(path: &str, exp_unix: i64, subject: &Subject) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key()).expect("HMAC accepts a key of any length");
    mac.update(path.as_bytes());
    mac.update(b"\n");
    mac.update(exp_unix.to_string().as_bytes());
    mac.update(b"\n");
    mac.update(subject.tag().as_bytes());
    mac.update(b"\n");
    mac.update(subject.id().as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Mint a read token for `path` (the MediaMTX path, e.g. `cam_<id>`) valid until `now + ttl_secs`,
/// bound to `subject`.
///
/// The id is base64url-encoded so it cannot contain the `.` used as the field separator.
pub fn mint(path: &str, now_unix: i64, ttl_secs: i64, subject: &Subject) -> String {
    let exp = now_unix + ttl_secs.max(1);
    format!(
        "v2.{exp}.{}.{}.{}",
        subject.tag(),
        B64.encode(subject.id().as_bytes()),
        B64.encode(sign(path, exp, subject))
    )
}

/// Verify a read token for `path` at wall-clock `now_unix`, returning the SUBJECT it was minted for.
///
/// Signature, path and expiry only — the caller must then decide whether that subject still stands
/// (see [`crate::routes::media_auth`]). Returning the subject rather than a bool is what forces that
/// second question to be asked: a `bool` here is exactly the shape that let the old token outlive its
/// credential.
///
/// `v1` tokens (no subject) are REFUSED. They are process-lifetime only — the signing key is random
/// per boot — so the sole way to hold one is across a restart that already invalidated it.
pub fn verify(path: &str, token: &str, now_unix: i64) -> Option<Subject> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 5 || parts[0] != "v2" {
        return None;
    }
    let exp = parts[1].parse::<i64>().ok()?;
    if now_unix >= exp {
        return None;
    }
    let id = String::from_utf8(B64.decode(parts[3]).ok()?).ok()?;
    let subject = match parts[2] {
        "k" => Subject::ApiKey(id),
        "u" => Subject::User(id),
        "s" => Subject::Site,
        _ => return None,
    };
    let sig = B64.decode(parts[4]).ok()?;
    // `verify_slice` is constant-time and also checks the length.
    let mut mac = HmacSha256::new_from_slice(key()).expect("HMAC accepts a key of any length");
    mac.update(path.as_bytes());
    mac.update(b"\n");
    mac.update(exp.to_string().as_bytes());
    mac.update(b"\n");
    mac.update(subject.tag().as_bytes());
    mac.update(b"\n");
    mac.update(subject.id().as_bytes());
    mac.verify_slice(&sig).ok()?;
    Some(subject)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subj() -> Subject {
        Subject::ApiKey("key_abc".into())
    }

    #[test]
    fn round_trips_within_ttl() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600, &subj());
        assert_eq!(verify("cam_abc", &t, now), Some(subj()));
        assert_eq!(verify("cam_abc", &t, now + 3599), Some(subj()));
    }

    #[test]
    fn rejects_after_expiry() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600, &subj());
        assert_eq!(verify("cam_abc", &t, now + 3601), None);
    }

    #[test]
    fn rejects_wrong_path() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600, &subj());
        assert_eq!(verify("cam_xyz", &t, now), None); // token is path-scoped
    }

    #[test]
    fn rejects_tampered_or_malformed() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600, &subj());
        assert_eq!(verify("cam_abc", &format!("{t}x"), now), None);
        assert_eq!(verify("cam_abc", "", now), None);
        assert_eq!(verify("cam_abc", "v2.x.k.aaa.bbb", now), None);
    }

    /// The subject is INSIDE the signature. Swapping the tag or the id — the move that would turn a
    /// revoked key's token into an unwithdrawable `Site` one — must not verify.
    #[test]
    fn the_subject_cannot_be_swapped() {
        let now = 1_000_000;
        let t = mint("cam_abc", now, 3600, &subj());
        let parts: Vec<&str> = t.split('.').collect();
        // same signature, claim to be the site
        let forged_site = format!("v2.{}.s..{}", parts[1], parts[4]);
        assert_eq!(verify("cam_abc", &forged_site, now), None);
        // same signature, claim to be a different key
        let other = B64.encode(b"key_someone_else");
        let forged_id = format!("v2.{}.k.{}.{}", parts[1], other, parts[4]);
        assert_eq!(verify("cam_abc", &forged_id, now), None);
    }

    /// Each subject kind round-trips as itself — the callback branches on this, so a kind that
    /// decoded as the wrong variant would silently pick the wrong re-check (or skip it).
    #[test]
    fn every_subject_kind_round_trips() {
        let now = 1_000_000;
        for s in [
            Subject::ApiKey("key_1".into()),
            Subject::User("usr_1".into()),
            Subject::Site,
        ] {
            let t = mint("cam_abc", now, 3600, &s);
            assert_eq!(verify("cam_abc", &t, now), Some(s));
        }
    }

    /// A `v1` token — the unbound shape — is refused rather than honoured for compatibility. They
    /// only exist within one boot (the signing key is random per process), so nothing legitimate
    /// holds one across the upgrade that introduced this.
    #[test]
    fn the_unbound_v1_shape_is_refused() {
        let now = 1_000_000;
        let exp = now + 3600;
        let legacy = format!(
            "v1.{exp}.{}",
            B64.encode(sign("cam_abc", exp, &Subject::Site))
        );
        assert_eq!(verify("cam_abc", &legacy, now), None);
    }
}
