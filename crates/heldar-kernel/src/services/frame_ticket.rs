//! Server-issued, per-frame ingest tickets — the unforgeable half of AI provenance.
//!
//! Before this, `frame_id` was CONSTRUCTED BY THE WORKER (`"<task>:<captured_at>"`) and the kernel
//! trusted `camera_id` / `task_type` straight out of the request body. Anything holding an integration
//! key could therefore post a batch naming any camera and any task type, and could pre-claim a
//! `frame_id` the real worker was about to use (first-writer-wins on `idx_outbox_dedup`) to SUPPRESS a
//! genuine detection.
//!
//! A ticket closes that. `GET /api/v1/cameras/{id}/frame?task=<ai_task_id>` returns the JPEG *and*, when
//! the caller holds a live lease on that task, an `x-frame-ticket` header. The ticket names the task,
//! and the kernel DERIVES `camera_id`, `task_type` and `frame_id` from it at ingest instead of trusting
//! the body — so a worker can only speak about frames it was actually handed.
//!
//! Shape (a deliberate structural copy of [`crate::services::live_token`]):
//!   `f1.{task_id}.{captured_ms}.{exp}.{b64url(sig)}`
//!   `sig = HMAC-SHA256(boot_key, "f1\n{api_key_id}\n{camera_id}\n{task_id}\n{captured_ms}\n{exp}")`
//!
//! `api_key_id` is inside the preimage but NOT inside the token: a leaked ticket is inert for any other
//! credential, because verification recomputes the MAC over the CALLER's key id. `camera_id` is likewise
//! only in the preimage — the verifier supplies it from the lease row, so a ticket cannot be re-pointed
//! at another camera.
//!
//! The signing key is process-global and random per boot (as for live tokens): tickets do not survive a
//! restart, which is harmless because the worker re-pulls a frame — and therefore a fresh ticket —
//! every cycle. No new configured secret, no key file, nothing to rotate.

use std::sync::OnceLock;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Process-wide random signing key, lazily initialized on first mint or verify.
static KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn key() -> &'static [u8; 32] {
    KEY.get_or_init(|| {
        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        k
    })
}

/// The wire prefix / MAC domain separator. Bumping it invalidates every outstanding ticket.
const VERSION: &str = "f1";

fn sign(api_key_id: &str, camera_id: &str, task_id: &str, captured_ms: i64, exp: i64) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key()).expect("HMAC accepts a key of any length");
    // Newline-separated, and every field is validated dot/newline-free before a ticket is minted, so
    // the preimage is unambiguous (no field-splitting confusion between e.g. task ids and key ids).
    mac.update(
        format!("{VERSION}\n{api_key_id}\n{camera_id}\n{task_id}\n{captured_ms}\n{exp}").as_bytes(),
    );
    mac.finalize().into_bytes().to_vec()
}

/// A verified ticket: what the kernel will use INSTEAD of the client-supplied fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameTicket {
    pub task_id: String,
    /// Capture time of the frame the ticket was issued for, in unix milliseconds. Half of the
    /// server-derived `frame_id` (`"{task_id}:{captured_ms}"`).
    pub captured_ms: i64,
    pub exp: i64,
}

impl FrameTicket {
    /// The server-derived idempotency key for this frame. Byte-identical to what the reference worker
    /// already built client-side, so there is no discontinuity at cutover.
    pub fn frame_id(&self) -> String {
        format!("{}:{}", self.task_id, self.captured_ms)
    }
}

/// Fields that must not contain the token separator (or a newline, which would make the MAC preimage
/// ambiguous). Kernel-generated ids never do; this is a belt-and-braces guard, not a validator.
fn field_is_clean(s: &str) -> bool {
    !s.is_empty() && !s.contains('.') && !s.contains('\n') && !s.contains('\r')
}

/// Mint a ticket for one frame. Returns `None` when a field cannot be encoded unambiguously — the
/// caller then simply emits no header, which degrades to "no ticket" rather than to a weak ticket.
pub fn mint(
    api_key_id: &str,
    camera_id: &str,
    task_id: &str,
    captured_ms: i64,
    now_unix: i64,
    ttl_secs: i64,
) -> Option<String> {
    if !field_is_clean(api_key_id) || !field_is_clean(camera_id) || !field_is_clean(task_id) {
        return None;
    }
    let exp = now_unix + ttl_secs.max(1);
    let sig = sign(api_key_id, camera_id, task_id, captured_ms, exp);
    Some(format!(
        "{VERSION}.{task_id}.{captured_ms}.{exp}.{}",
        B64.encode(sig)
    ))
}

/// Parse a ticket's PUBLIC fields without checking the signature.
///
/// Used only to learn which `task_id` a ticket claims, so the caller can load that task's lease and
/// obtain the `camera_id` the MAC must be recomputed over. Never trust the result without a subsequent
/// [`verify`].
pub fn peek(raw: &str) -> Option<FrameTicket> {
    let parts: Vec<&str> = raw.trim().split('.').collect();
    if parts.len() != 5 || parts[0] != VERSION {
        return None;
    }
    Some(FrameTicket {
        task_id: parts[1].to_string(),
        captured_ms: parts[2].parse().ok()?,
        exp: parts[3].parse().ok()?,
    })
}

/// Verify a ticket against the CALLER's key id and the camera the lease says the task runs on.
///
/// Constant-time signature comparison. Rejects malformed, tampered, wrong-key, wrong-camera,
/// wrong-task and expired tickets alike, with no distinguishing signal.
pub fn verify(raw: &str, api_key_id: &str, camera_id: &str, now_unix: i64) -> Option<FrameTicket> {
    let parts: Vec<&str> = raw.trim().split('.').collect();
    if parts.len() != 5 || parts[0] != VERSION {
        return None;
    }
    let ticket = FrameTicket {
        task_id: parts[1].to_string(),
        captured_ms: parts[2].parse().ok()?,
        exp: parts[3].parse().ok()?,
    };
    if now_unix >= ticket.exp {
        return None;
    }
    let sig = B64.decode(parts[4]).ok()?;
    let mut mac = HmacSha256::new_from_slice(key()).expect("HMAC accepts a key of any length");
    mac.update(
        format!(
            "{VERSION}\n{api_key_id}\n{camera_id}\n{}\n{}\n{}",
            ticket.task_id, ticket.captured_ms, ticket.exp
        )
        .as_bytes(),
    );
    // `verify_slice` is constant-time and length-checked.
    mac.verify_slice(&sig).ok()?;
    Some(ticket)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn t() -> String {
        mint("key_a", "cam1", "ai_1", 1_700_000_000_123, NOW, 120).unwrap()
    }

    #[test]
    fn round_trips_within_ttl_and_derives_the_frame_id() {
        let raw = t();
        let v = verify(&raw, "key_a", "cam1", NOW).unwrap();
        assert_eq!(v.task_id, "ai_1");
        assert_eq!(v.captured_ms, 1_700_000_000_123);
        assert_eq!(v.frame_id(), "ai_1:1700000000123");
        assert!(verify(&raw, "key_a", "cam1", NOW + 119).is_some());
    }

    /// The headline binding: a ticket minted for key A is INERT for key B. This is the test that fails
    /// if `api_key_id` is ever dropped from the MAC preimage.
    #[test]
    fn a_ticket_is_bound_to_the_credential_it_was_issued_to() {
        let raw = t();
        assert!(verify(&raw, "key_b", "cam1", NOW).is_none());
    }

    /// ...and to the camera the lease named, so it cannot be re-pointed at another lane.
    #[test]
    fn a_ticket_is_bound_to_its_camera() {
        let raw = t();
        assert!(verify(&raw, "key_a", "cam2", NOW).is_none());
    }

    #[test]
    fn rejects_after_expiry() {
        let raw = t();
        assert!(verify(&raw, "key_a", "cam1", NOW + 120).is_none());
        assert!(verify(&raw, "key_a", "cam1", NOW + 10_000).is_none());
    }

    #[test]
    fn rejects_tampered_and_malformed() {
        let raw = t();
        assert!(verify(&format!("{raw}x"), "key_a", "cam1", NOW).is_none());
        assert!(verify("f1.ai_1.1.9999999999.deadbeef", "key_a", "cam1", NOW).is_none());
        assert!(verify("garbage", "key_a", "cam1", NOW).is_none());
        assert!(verify("f2.ai_1.1.9999999999.aa", "key_a", "cam1", NOW).is_none());
        assert!(verify("", "key_a", "cam1", NOW).is_none());
        // Swapping in another task id breaks the MAC even though the shape is right.
        let swapped = raw.replacen("ai_1", "ai_2", 1);
        assert!(verify(&swapped, "key_a", "cam1", NOW).is_none());
    }

    #[test]
    fn peek_reads_public_fields_without_authenticating_them() {
        let raw = t();
        let p = peek(&raw).unwrap();
        assert_eq!(p.task_id, "ai_1");
        assert_eq!(p.captured_ms, 1_700_000_000_123);
        assert!(peek("nonsense").is_none());
    }

    /// A field that would make the token ambiguous yields NO ticket rather than a weak one.
    #[test]
    fn refuses_to_mint_over_ambiguous_fields() {
        assert!(mint("key.a", "cam1", "ai_1", 1, NOW, 120).is_none());
        assert!(mint("key_a", "cam.1", "ai_1", 1, NOW, 120).is_none());
        assert!(mint("key_a", "cam1", "ai.1", 1, NOW, 120).is_none());
        assert!(mint("", "cam1", "ai_1", 1, NOW, 120).is_none());
    }
}
