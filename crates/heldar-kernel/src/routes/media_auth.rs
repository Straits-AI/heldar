//! MediaMTX HTTP external-auth callback.
//!
//! MediaMTX (configured with `authMethod: http`, `authHTTPAddress` pointing here) POSTs an authorization
//! request for every publish/read/playback and the kernel replies 2xx=allow / non-2xx=deny. This is what
//! makes the browser — which streams DIRECTLY from MediaMTX at `cam_<id>` on the RTSP/HLS/WebRTC ports —
//! subject to kernel authorization, since the kernel only hands out stream URLs after its own `can_view`.
//!
//! Policy:
//! - `publish` / `api` / `metrics` / `pprof`: allow only from a loopback client (the kernel's own
//!   live-publisher ffmpeg (`services/live_publisher.rs`) publishes from `rtsp://localhost`, and the
//!   API/metrics ports are loopback-bound).
//!   This preserves the anti-stream-injection posture that `authInternalUsers` used to enforce (which is
//!   ignored under `authMethod: http`).
//! - `read` / `playback`:
//!     - when kernel auth is ENABLED → require a valid, path-scoped, kernel-minted token (see
//!       [`crate::services::live_token`]); this is the per-user gating.
//!     - when kernel auth is DISABLED (LAN-appliance default) → no per-user token, but still allow only
//!       LAN/private/overlay media clients and deny public ones, so a port-forwarded/exposed box does not
//!       serve every camera to the internet (the protection the source-IP allowlist gave, moved here).
//!
//! This endpoint takes no `Principal` (MediaMTX has no user session). It is a pure allow/deny oracle with
//! no data and no side effects; a direct caller cannot forge a token nor gain anything from a decision, so
//! it does not need the endpoint restricted to loopback (the `ip` field is the media client's address as
//! MediaMTX reports it, not the caller's).

use std::net::IpAddr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/mediamtx-auth", post(authorize))
}

/// The subset of the MediaMTX external-auth request body the kernel needs.
#[derive(Debug, Default, Deserialize)]
struct AuthRequest {
    #[serde(default)]
    ip: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    token: String,
}

fn client_ip(req: &AuthRequest) -> Option<IpAddr> {
    // MediaMTX may send `ip` or `ip:port` — strip a trailing :port if present (v6 is bracketed).
    let raw = req.ip.trim();
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Some(ip);
    }
    raw.rsplit_once(':')
        .and_then(|(h, _)| h.trim_start_matches('[').trim_end_matches(']').parse().ok())
}

fn is_loopback(req: &AuthRequest) -> bool {
    client_ip(req).map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// A LAN/private/overlay media client: loopback, RFC1918, CGNAT/overlay (100.64/10, used by Tailscale/
/// NetBird), or IPv6 ULA/link-local. Mirrors the source-IP allowlist that previously lived in
/// mediamtx.yml. A public address (or an unparseable one) is rejected.
fn is_lan_client(req: &AuthRequest) -> bool {
    let Some(ip) = client_ip(req) else {
        return false;
    };
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64.0.0/10 CGNAT/overlay
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

fn token_of(req: &AuthRequest) -> Option<String> {
    if !req.token.is_empty() {
        return Some(req.token.clone());
    }
    // `query` is the raw stream-URL query string, e.g. "token=v1.123.abc&x=y".
    req.query.split('&').find_map(|kv| {
        kv.strip_prefix("token=")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

async fn authorize(State(st): State<AppState>, Json(req): Json<AuthRequest>) -> StatusCode {
    let allowed = match req.action.as_str() {
        // Publishing + the control/metrics surfaces stay loopback-only.
        "publish" | "api" | "metrics" | "pprof" => is_loopback(&req),
        // Reads/playback: token-gated when auth is on; LAN-IP-gated when auth is off (LAN default).
        //
        // Two questions, not one: is the token authentic, and does its subject STILL stand? The
        // signature alone used to be the whole answer, which is why a revoked key kept streaming to
        // the TTL. `verify` returns the subject precisely so this arm cannot forget to ask the
        // second question.
        //
        // What this bounds: a transport is re-authorized as often as it re-presents the token. HLS
        // does that per segment, so withdrawal bites in seconds; an ESTABLISHED WebRTC session is
        // negotiated once and keeps flowing regardless. See `services::live_token`.
        "read" | "playback" => {
            if st.cfg.auth_enabled {
                match token_of(&req).and_then(|t| {
                    crate::services::live_token::verify(
                        &req.path,
                        &t,
                        chrono::Utc::now().timestamp(),
                    )
                }) {
                    Some(subject) => subject_still_stands(&st, &subject, &req.path).await,
                    None => false,
                }
            } else {
                is_lan_client(&req)
            }
        }
        _ => false,
    };
    if allowed {
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    }
}

/// Does the credential that opened this stream still stand, and still hold this camera?
///
/// Fail-OPEN on a database read failure, deliberately and loudly: the recorder shares this SQLite and
/// a busy timeout under write load is routine. Turning routine contention into a black video wall
/// across the whole site is a worse failure than the exposure it would prevent — the same trade the
/// backup creator re-check makes, for the same reason.
async fn subject_still_stands(
    st: &AppState,
    subject: &crate::services::live_token::Subject,
    path: &str,
) -> bool {
    use crate::services::live_token::Subject;
    match subject {
        // No withdrawable credential behind it: auth-disabled boxes and the site-token rendezvous.
        Subject::Site => true,
        Subject::User(id) => {
            match sqlx::query_scalar::<_, bool>("SELECT active FROM users WHERE id = ?")
                .bind(id)
                .fetch_optional(&st.pool)
                .await
            {
                Ok(Some(active)) => active,
                // Deleted: withdrawn.
                Ok(None) => false,
                Err(e) => {
                    tracing::warn!(user = %id, error = %e, "media_auth: could not re-check the streaming user; allowing");
                    true
                }
            }
        }
        Subject::ApiKey(id) => {
            match crate::auth::api_key_principal_now(&st.pool, id, st.cfg.machine_auth).await {
                // Still a valid credential — but it must ALSO still hold this camera, or narrowing
                // `scope_cameras` would leave the stream running. `path` is the MediaMTX path
                // (`cam_<camera_id>`), which is what the token is scoped to.
                Ok(Some(principal)) => match path.strip_prefix("cam_") {
                    Some(camera_id) => principal.camera_allowed(camera_id),
                    // A path shape this kernel did not mint a camera token for.
                    None => false,
                },
                // Revoked, deactivated, expired or deleted.
                Ok(None) => false,
                Err(e) => {
                    tracing::warn!(api_key = %id, error = %e, "media_auth: could not re-check the streaming key; allowing");
                    true
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(action: &str, ip: &str, query: &str) -> AuthRequest {
        AuthRequest {
            ip: ip.into(),
            action: action.into(),
            path: "cam_1".into(),
            query: query.into(),
            token: String::new(),
        }
    }

    #[test]
    fn publish_is_loopback_only() {
        assert!(is_loopback(&req("publish", "127.0.0.1", "")));
        assert!(is_loopback(&req("publish", "::1", "")));
        assert!(!is_loopback(&req("publish", "192.168.1.5", "")));
        assert!(!is_loopback(&req("publish", "8.8.8.8", "")));
    }

    #[test]
    fn lan_client_classification() {
        assert!(is_lan_client(&req("read", "127.0.0.1", "")));
        assert!(is_lan_client(&req("read", "192.168.1.5", "")));
        assert!(is_lan_client(&req("read", "10.0.0.9", "")));
        assert!(is_lan_client(&req("read", "100.101.102.103", ""))); // Tailscale/NetBird CGNAT
        assert!(is_lan_client(&req("read", "fd00::1", "")));
        assert!(!is_lan_client(&req("read", "8.8.8.8", ""))); // public → denied on the LAN default
        assert!(!is_lan_client(&req("read", "203.0.113.7", "")));
        assert!(!is_lan_client(&req("read", "garbage", "")));
    }

    #[test]
    fn ip_port_forms_parse() {
        assert!(is_lan_client(&req("read", "192.168.1.5:54321", "")));
        assert!(is_loopback(&req("publish", "127.0.0.1:9000", "")));
    }

    #[test]
    fn token_extracted_from_query_and_field() {
        assert_eq!(
            token_of(&req("read", "1.2.3.4", "token=v1.9.abc&x=1")).as_deref(),
            Some("v1.9.abc")
        );
        let mut r = req("read", "1.2.3.4", "");
        r.token = "v1.9.def".into();
        assert_eq!(token_of(&r).as_deref(), Some("v1.9.def"));
        assert!(token_of(&req("read", "1.2.3.4", "x=1")).is_none());
    }
}
