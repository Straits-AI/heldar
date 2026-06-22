//! Box-side WebRTC rendezvous client (ADR 0003, P2).
//!
//! For universal remote viewing the box must be reachable from any browser on any network, but it is
//! typically behind CGNAT (no inbound). The fix — like the rest of the kernel's cloud seams — is to dial
//! OUT: this loop maintains an outbound HTTP long-poll to a public rendezvous (the private `heldar`
//! Cloudflare Worker + Durable Object — `apps/edge/`). When a browser asks to view a camera, the
//! rendezvous hands the box the browser's WebRTC SDP offer; the box bridges it to its OWN local MediaMTX
//! WHEP endpoint and returns the answer. Media then flows browser ⇄ TURN ⇄ MediaMTX (DTLS-SRTP) — never
//! through the rendezvous, never re-encoded here. The box only shuttles two SDP blobs per session.
//!
//! Pure outbound HTTP, no new crates — the only seam is `HELDAR_REMOTE_RENDEZVOUS_URL`. Strictly opt-in:
//! unset (the default) and this loop parks forever, the same posture as `fleet_register`. Reuses the
//! `HELDAR_CP_TLS_*` mTLS identity when configured (not needed for the Cloudflare Worker — it uses the
//! `HELDAR_CP_TOKEN` bearer).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::config::Config;
use crate::services::mediamtx;
use crate::state::AppState;

/// Long-poll endpoint: the box asks for the next pending viewing session (doubles as a liveness beat).
fn poll_url(rendezvous_url: &str) -> String {
    format!(
        "{}/api/v1/rendezvous/poll",
        rendezvous_url.trim_end_matches('/')
    )
}

/// Endpoint the box POSTs the WHEP answer (or a bridge error) back to, keyed by session id.
fn answer_url(rendezvous_url: &str) -> String {
    format!(
        "{}/api/v1/rendezvous/answer",
        rendezvous_url.trim_end_matches('/')
    )
}

/// A pending browser viewing session the rendezvous handed us: the camera and its recvonly SDP offer.
#[derive(Debug, Deserialize)]
struct PendingSession {
    session_id: String,
    camera_id: String,
    sdp_offer: String,
}

/// Build the outbound client, configuring mTLS (client identity + control-plane CA) when
/// `HELDAR_CP_TLS_*` is set — same material the fleet registration uses. Errors only on bad cert files.
fn build_client(cfg: &Config) -> anyhow::Result<reqwest::Client> {
    // A generous timeout: the poll is a long-poll the rendezvous holds open until work arrives or it
    // times out server-side.
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(40));
    if let Some(t) = &cfg.cp_tls {
        let cert = std::fs::read(&t.client_cert)
            .with_context(|| format!("reading client cert {}", t.client_cert.display()))?;
        let key = std::fs::read(&t.client_key)
            .with_context(|| format!("reading client key {}", t.client_key.display()))?;
        let ca = std::fs::read(&t.server_ca)
            .with_context(|| format!("reading control-plane CA {}", t.server_ca.display()))?;
        let mut identity_pem = key;
        identity_pem.extend_from_slice(&cert);
        let identity =
            reqwest::Identity::from_pem(&identity_pem).context("building client identity")?;
        let root = reqwest::Certificate::from_pem(&ca).context("parsing control-plane CA")?;
        builder = builder.identity(identity).add_root_certificate(root);
    }
    builder.build().context("building HTTP client")
}

/// Bridge a browser SDP offer to the local MediaMTX WHEP endpoint and return the answer. Reuses
/// `ensure_live` (which creates the `cam_<id>` path on demand) with `request_host = None`, so the
/// returned `webrtc_url` keeps its loopback base — exactly the address the box POSTs to its own MediaMTX.
///
/// Authorization note: the rendezvous (the `heldar` Worker, `apps/edge/`) is the sole authority on WHO may
/// view WHICH camera — it verifies a signed ticket before relaying a session here. The box only talks to
/// the rendezvous it dialed OUT to, so it trusts the session it is handed; it does not re-check the ticket.
async fn bridge_to_local_whep(
    state: &AppState,
    camera_id: &str,
    sdp_offer: &str,
) -> anyhow::Result<String> {
    let live = mediamtx::ensure_live(state, camera_id, None)
        .await
        .map_err(|e| anyhow::anyhow!("ensure_live({camera_id}) failed: {e}"))?;
    let whep = format!("{}/whep", live.webrtc_url);
    let answer = state
        .http
        .post(&whep)
        // MediaMTX answers a WHEP offer only once its on-demand HEVC→H.264 transcode has started, which
        // can exceed `state.http`'s default 10s timeout — give the cold start room (still under the poll).
        .timeout(Duration::from_secs(25))
        .header(CONTENT_TYPE, "application/sdp")
        .header(ACCEPT, "application/sdp")
        .body(sdp_offer.to_owned())
        .send()
        .await
        .context("posting offer to local WHEP")?
        .error_for_status()
        .context("local WHEP rejected the offer")?
        .text()
        .await
        .context("reading WHEP answer")?;
    Ok(answer)
}

/// Largest browser SDP offer we'll bridge (defensive — the rendezvous already caps it well below this).
const MAX_SDP_BYTES: usize = 512 * 1024;

/// The box's camera list (id + display name) advertised to the rendezvous on each poll, so the grid
/// viewer can enumerate cameras without reaching the box's REST API (that is the Stage C relay). Read
/// straight from local state (no self-HTTP, so it is unaffected by whether the REST API requires auth);
/// names fall back to the id. Exposes only id+name — never a stream URL or credential.
async fn camera_catalog(state: &AppState) -> Vec<serde_json::Value> {
    sqlx::query_as::<_, (String, Option<String>)>("SELECT id, name FROM cameras ORDER BY id ASC")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name)| {
            let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
            json!({ "id": id, "name": name })
        })
        .collect()
}

/// One long-poll cycle: ask for the next session; if one arrives, bridge it and report the answer (or the
/// error) back. Returns `Ok(true)` when a bridge FAILED (so the caller can rate-limit a persistent local
/// failure), `Ok(false)` on a clean cycle (work handled or nothing pending). `Err` only on a transport
/// failure talking to the rendezvous, which the caller backs off on.
async fn poll_once(
    state: &AppState,
    client: &reqwest::Client,
    rendezvous_url: &str,
    site_id: &str,
    token: &str,
) -> anyhow::Result<bool> {
    let resp = client
        .post(poll_url(rendezvous_url))
        .bearer_auth(token)
        // Piggy-back the camera list so the grid viewer can enumerate cameras (refreshed every poll).
        .json(&json!({ "site_id": site_id, "cameras": camera_catalog(state).await }))
        .send()
        .await
        .context("rendezvous poll request")?;
    if resp.status() == StatusCode::NO_CONTENT {
        return Ok(false); // long-poll timed out with no work — re-poll
    }
    let session: PendingSession = resp
        .error_for_status()
        .context("rendezvous poll rejected")?
        .json()
        .await
        .context("decoding pending session")?;

    let result = if session.sdp_offer.len() > MAX_SDP_BYTES {
        Err(anyhow::anyhow!(
            "offer too large ({} bytes)",
            session.sdp_offer.len()
        ))
    } else {
        bridge_to_local_whep(state, &session.camera_id, &session.sdp_offer).await
    };
    // `site_id` lets the rendezvous route the answer back to this box's session (the Durable Object
    // keyed by site id). `session_id` matches it to the waiting browser request.
    let body = match &result {
        Ok(sdp) => {
            json!({ "site_id": site_id, "session_id": session.session_id, "sdp_answer": sdp })
        }
        Err(e) => {
            json!({ "site_id": site_id, "session_id": session.session_id, "error": e.to_string() })
        }
    };
    if let Err(e) = &result {
        tracing::warn!(session = %session.session_id, camera = %session.camera_id, error = %e, "rendezvous: bridge to local WHEP failed");
    }
    client
        .post(answer_url(rendezvous_url))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .context("posting answer to rendezvous")?
        .error_for_status()
        .context("rendezvous rejected the answer")?;
    Ok(result.is_err())
}

/// The dial-out loop. Parks forever unless `HELDAR_REMOTE_RENDEZVOUS_URL` + `HELDAR_SITE_ID` are set
/// (remote access is opt-in). Otherwise long-polls the rendezvous, bridging each viewing session to the
/// local MediaMTX, with exponential backoff on transport failure. Never returns.
pub async fn run(state: AppState) {
    let cfg = state.cfg.clone();
    let (Some(rendezvous_url), Some(site_id)) =
        (cfg.rendezvous_url.as_deref(), cfg.site_id.as_deref())
    else {
        std::future::pending::<()>().await;
        return;
    };

    let client = match build_client(&cfg) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "webrtc rendezvous disabled: bad mTLS config");
            std::future::pending::<()>().await;
            return;
        }
    };

    if cfg.cp_token.is_empty() {
        tracing::warn!(
            "webrtc rendezvous: HELDAR_CP_TOKEN is empty; the rendezvous will reject polls if it enforces a bearer (BOX_TOKEN)"
        );
    }
    tracing::info!(site = %site_id, rendezvous = %rendezvous_url, "webrtc rendezvous: dialing out for remote viewing");
    let mut backoff = Duration::from_secs(1);
    loop {
        match poll_once(&state, &client, rendezvous_url, site_id, &cfg.cp_token).await {
            Ok(false) => backoff = Duration::from_secs(1),
            // A bridge to the local MediaMTX failed (e.g. camera/transcode down) — the answer/error was
            // already reported to the browser; pause briefly so a persistent failure can't tight-loop.
            Ok(true) => tokio::time::sleep(Duration::from_secs(2)).await,
            Err(e) => {
                tracing::warn!(site = %site_id, error = %e, "webrtc rendezvous poll failed; backing off");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

/// The box-facing TURN endpoint on the rendezvous (mints ICE for the box's own MediaMTX).
fn box_turn_url(rendezvous_url: &str) -> String {
    format!("{}/api/v1/box/turn", rendezvous_url.trim_end_matches('/'))
}

/// Fetch short-lived TURN credentials from the rendezvous and shape them into a MediaMTX
/// `webrtcICEServers2` array (`[{url, username?, password?}]`).
async fn fetch_rendezvous_ice(
    client: &reqwest::Client,
    rendezvous_url: &str,
    token: &str,
) -> anyhow::Result<serde_json::Value> {
    let data: serde_json::Value = client
        .get(box_turn_url(rendezvous_url))
        .bearer_auth(token)
        .send()
        .await
        .context("rendezvous box/turn request")?
        .error_for_status()
        .context("rendezvous box/turn rejected")?
        .json()
        .await
        .context("decoding box/turn")?;
    let ice = data
        .get("iceServers")
        .ok_or_else(|| anyhow::anyhow!("box/turn response missing iceServers"))?;
    let user = ice.get("username").and_then(|v| v.as_str());
    let cred = ice.get("credential").and_then(|v| v.as_str());
    let urls = ice
        .get("urls")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("box/turn response missing iceServers.urls"))?;
    let list: Vec<serde_json::Value> = urls
        .iter()
        .filter_map(|u| u.as_str())
        .map(|u| {
            if u.starts_with("stun:") {
                json!({ "url": u })
            } else {
                json!({ "url": u, "username": user, "password": cred })
            }
        })
        .collect();
    Ok(serde_json::Value::Array(list))
}

/// Resolve the ICE servers to program into MediaMTX, and how long until the next refresh.
async fn resolve_ice(cfg: &Config, client: &reqwest::Client) -> (serde_json::Value, Duration) {
    // 1) Operator-provided (their own STUN/TURN) — static, refresh rarely.
    if let Some(raw) = &cfg.webrtc_ice_servers {
        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(v) => return (v, Duration::from_secs(12 * 3600)),
            Err(e) => {
                tracing::error!(error = %e, "HELDAR_WEBRTC_ICE_SERVERS is not valid JSON; ignoring")
            }
        }
    }
    // 2) Heldar-hosted: short-lived TURN from the rendezvous (creds expire → refresh often).
    if let Some(url) = cfg.rendezvous_url.as_deref() {
        match fetch_rendezvous_ice(client, url, &cfg.cp_token).await {
            Ok(v) => return (v, Duration::from_secs(30 * 60)),
            Err(e) => {
                tracing::warn!(error = %e, "webrtc ICE: rendezvous TURN fetch failed; using STUN only")
            }
        }
    }
    // 3) Fallback: STUN only (works for non-symmetric NAT).
    (
        json!([{ "url": "stun:stun.cloudflare.com:3478" }]),
        Duration::from_secs(30 * 60),
    )
}

/// Periodically program MediaMTX's WebRTC ICE servers for remote viewing — the operator's own
/// `HELDAR_WEBRTC_ICE_SERVERS`, else short-lived TURN fetched from the rendezvous, else STUN. Parks when
/// remote viewing is not configured (neither ICE config nor a rendezvous URL set).
pub async fn run_ice(state: AppState) {
    let cfg = state.cfg.clone();
    if cfg.webrtc_ice_servers.is_none() && cfg.rendezvous_url.is_none() {
        std::future::pending::<()>().await;
        return;
    }
    let client = match build_client(&cfg) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "webrtc ICE disabled: bad mTLS config");
            std::future::pending::<()>().await;
            return;
        }
    };
    loop {
        let (ice, cadence) = resolve_ice(&cfg, &client).await;
        match mediamtx::set_webrtc_ice_servers(&state, &ice).await {
            Ok(()) => tracing::info!("webrtc ICE: programmed MediaMTX ICE servers"),
            Err(e) => tracing::warn!(error = %e, "webrtc ICE: failed to program MediaMTX"),
        }
        tokio::time::sleep(cadence).await;
    }
}

// ---- Stage C: authenticated read-only HTTP relay ----
//
// A SECOND outbound channel so an authenticated remote browser can drive the kernel's REST API
// (read-only in Stage C). The rendezvous hands the box a `RelayJob` — an HTTP request the browser made,
// carrying the user's REAL kernel Bearer — and the box REPLAYS it against its own local kernel
// (`127.0.0.1:api_port`). The kernel runs its NORMAL auth + RBAC, so the relay is a dumb, allowlisted
// pipe — never an auth-bypass, never a fabricated principal. Independent of the WHEP channel (separate
// poll → no head-of-line blocking). FAIL-SAFE: this loop refuses to run unless kernel auth is ENABLED
// and a real user exists, so the REST API is never exposed remotely while it would answer as the
// synthetic auth-off admin.

fn relay_poll_url(u: &str) -> String {
    format!("{}/api/v1/relay/poll", u.trim_end_matches('/'))
}
fn relay_respond_url(u: &str) -> String {
    format!("{}/api/v1/relay/respond", u.trim_end_matches('/'))
}

/// An HTTP request the rendezvous asks the box to replay against its local kernel.
#[derive(Debug, Deserialize)]
struct RelayJob {
    job_id: String,
    method: String,
    path: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body_b64: Option<String>,
}

/// Concurrent relay pollers, so a few dashboard reads can be in flight at once (vs fully serialized).
const RELAY_POLLERS: usize = 4;
/// Cap on a relayed request/response body (defensive; Stage C is small JSON + the odd snapshot).
const MAX_RELAY_BODY: usize = 8 * 1024 * 1024;

/// What the box will replay for the remote dashboard (Stage B): the full REST + media surface, all HTTP
/// methods. The kernel's own auth + RBAC (run on the forwarded per-user Bearer) is the real authorization
/// gate; this allowlist is defense in depth — it pins the surface to `/api/v1/*` + `/media/*`, blocks
/// path traversal/smuggling, and never relays the Worker-internal/metrics surfaces.
fn relay_allowed(method: &str, path: &str) -> bool {
    if !path.starts_with('/') || path.contains("..") || path.contains("//") || path.contains('@') {
        return false;
    }
    const DENY: &[&str] = &["/api/v1/relay", "/api/v1/rendezvous", "/metrics"];
    if DENY
        .iter()
        .any(|d| path == *d || path.starts_with(&format!("{d}/")))
    {
        return false;
    }
    if !(path.starts_with("/api/v1/") || path.starts_with("/media/")) {
        return false;
    }
    matches!(method, "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE")
}

/// Request headers the box forwards from the browser to the local kernel (everything else stripped, so
/// a client cannot smuggle X-Forwarded-For / trust headers).
fn forward_request_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "accept"
            | "content-type"
            | "range"
            | "if-none-match"
            | "if-modified-since"
    )
}
/// Response headers the box passes back through the relay (Set-Cookie deliberately NOT forwarded — the
/// box's own cookie is meaningless cross-origin and the Worker manages the browser session).
fn forward_response_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "content-type"
            | "content-length"
            | "content-range"
            | "accept-ranges"
            | "cache-control"
            | "etag"
            | "last-modified"
    )
}

/// Replay one relay job against the local kernel; returns (status, response headers, base64 body).
async fn replay_relay_job(
    state: &AppState,
    job: &RelayJob,
) -> (u16, HashMap<String, String>, String) {
    // Canonicalize the path exactly as the HTTP client will before authorizing it. `url` resolves
    // `.`/`..`/`%2e%2e` dot-segments while parsing, so the raw `job.path` the allowlist sees can differ
    // from what actually goes on the wire. We therefore parse first, then (a) confirm the result still
    // points at our own loopback origin — `join()` on a `//host`/absolute-URL path could otherwise swap
    // the host (SSRF) — and (b) run the allowlist on the CANONICAL path, forwarding the parsed URL so
    // "authorized" is byte-for-byte "sent". Without this, `/api/v1/%2e%2e/%2e%2e/metrics` passes the
    // raw-string check yet is sent as `/metrics`, escaping the `/api/v1/`+`/media/` pin and the DENY list.
    let base = format!("http://127.0.0.1:{}", state.cfg.api_port);
    let parsed = match reqwest::Url::parse(&base).and_then(|b| b.join(&job.path)) {
        Ok(u) => u,
        Err(_) => {
            return (
                400,
                HashMap::new(),
                B64.encode(br#"{"error":"bad relay path"}"#),
            );
        }
    };
    let same_origin = parsed.scheme() == "http"
        && parsed.host_str() == Some("127.0.0.1")
        && parsed.port() == Some(state.cfg.api_port);
    if !same_origin || !relay_allowed(&job.method, parsed.path()) {
        return (
            403,
            HashMap::new(),
            B64.encode(br#"{"error":"relay path not allowed"}"#),
        );
    }
    let method = reqwest::Method::from_bytes(job.method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = state
        .http
        .request(method, parsed)
        .timeout(Duration::from_secs(20));
    for (k, v) in &job.headers {
        if forward_request_header(k) {
            req = req.header(k, v);
        }
    }
    if let Some(b) = &job.body_b64 {
        if let Ok(bytes) = B64.decode(b) {
            if bytes.len() <= MAX_RELAY_BODY {
                req = req.body(bytes);
            }
        }
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let mut headers = HashMap::new();
            for (k, v) in resp.headers() {
                if forward_response_header(k.as_str()) {
                    if let Ok(vs) = v.to_str() {
                        headers.insert(k.as_str().to_string(), vs.to_string());
                    }
                }
            }
            // Refuse to buffer an over-large upstream response: a forwarded request for a big media
            // object (e.g. a full recording fetched without a Range header) would otherwise make each
            // poller buffer the whole body in memory and could OOM the box. Large media is served with
            // a Content-Length, so reject before reading; the browser fetches video via Range requests.
            if resp
                .content_length()
                .is_some_and(|len| len > MAX_RELAY_BODY as u64)
            {
                return (
                    413,
                    HashMap::new(),
                    B64.encode(br#"{"error":"relay response too large; use range requests"}"#),
                );
            }
            let body = resp.bytes().await.unwrap_or_default();
            let slice = if body.len() > MAX_RELAY_BODY {
                &body[..MAX_RELAY_BODY]
            } else {
                &body[..]
            };
            (status, headers, B64.encode(slice))
        }
        Err(e) => (
            502,
            HashMap::new(),
            B64.encode(format!(r#"{{"error":"relay upstream: {e}"}}"#).as_bytes()),
        ),
    }
}

/// One relay poller cycle: long-poll for a job, replay it, post the response. `Err` only on a transport
/// failure with the rendezvous (the caller backs off).
async fn relay_poll_once(
    state: &AppState,
    client: &reqwest::Client,
    rendezvous_url: &str,
    site_id: &str,
    token: &str,
) -> anyhow::Result<()> {
    let resp = client
        .post(relay_poll_url(rendezvous_url))
        .bearer_auth(token)
        .json(&json!({ "site_id": site_id, "auth_enforced": true }))
        .send()
        .await
        .context("relay poll request")?;
    if resp.status() == StatusCode::NO_CONTENT {
        return Ok(());
    }
    let job: RelayJob = resp
        .error_for_status()
        .context("relay poll rejected")?
        .json()
        .await
        .context("decoding relay job")?;
    let (status, headers, body_b64) = replay_relay_job(state, &job).await;
    client
        .post(relay_respond_url(rendezvous_url))
        .bearer_auth(token)
        .json(&json!({
            "site_id": site_id,
            "job_id": job.job_id,
            "status": status,
            "headers": headers,
            "body_b64": body_b64,
        }))
        .send()
        .await
        .context("posting relay response")?
        .error_for_status()
        .context("rendezvous rejected relay response")?;
    Ok(())
}

/// The relay dial-out loop (Stage C). Parks unless remote viewing is configured AND kernel auth is
/// enabled AND a real (active) user exists — so the REST API is never relayed while auth is off. Runs a
/// small pool of concurrent pollers for responsiveness.
pub async fn run_relay(state: AppState) {
    let cfg = state.cfg.clone();
    let (Some(rendezvous_url), Some(site_id)) = (cfg.rendezvous_url.clone(), cfg.site_id.clone())
    else {
        std::future::pending::<()>().await;
        return;
    };
    if !cfg.auth_enabled {
        tracing::warn!(
            "webrtc relay disabled: kernel auth is OFF (HELDAR_AUTH_ENABLED=false). The remote REST \
             relay refuses to run until auth is enabled, so the open API is never exposed remotely."
        );
        std::future::pending::<()>().await;
        return;
    }
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE active = 1")
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);
    if users == 0 {
        tracing::warn!("webrtc relay disabled: kernel auth is on but no active users exist yet");
        std::future::pending::<()>().await;
        return;
    }
    let client = match build_client(&cfg) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "webrtc relay disabled: bad mTLS config");
            std::future::pending::<()>().await;
            return;
        }
    };
    tracing::info!(site = %site_id, "webrtc relay: dialing out for the authenticated remote dashboard (read-only)");
    let mut tasks = Vec::new();
    for _ in 0..RELAY_POLLERS {
        let state = state.clone();
        let client = client.clone();
        let rendezvous_url = rendezvous_url.clone();
        let site_id = site_id.clone();
        let token = cfg.cp_token.clone();
        tasks.push(tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            loop {
                match relay_poll_once(&state, &client, &rendezvous_url, &site_id, &token).await {
                    Ok(()) => backoff = Duration::from_secs(1),
                    Err(e) => {
                        tracing::warn!(error = %e, "webrtc relay poll failed; backing off");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                    }
                }
            }
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_allowlist_pins_surface_and_blocks_internal_and_traversal() {
        // the full REST + media surface, all methods (kernel RBAC is the real gate)
        assert!(relay_allowed("GET", "/api/v1/cameras"));
        assert!(relay_allowed("POST", "/api/v1/cameras"));
        assert!(relay_allowed("PATCH", "/api/v1/cameras/cam2"));
        assert!(relay_allowed("DELETE", "/api/v1/cameras/cam2"));
        assert!(relay_allowed("GET", "/media/recordings/x.mp4"));
        assert!(relay_allowed("POST", "/api/v1/auth/login"));
        // off-surface, Worker-internal, metrics, traversal, smuggling are refused
        assert!(!relay_allowed("GET", "/healthz"));
        assert!(!relay_allowed("GET", "/api/v1/relay/poll"));
        assert!(!relay_allowed("GET", "/api/v1/rendezvous/poll"));
        assert!(!relay_allowed("GET", "/metrics"));
        assert!(!relay_allowed("GET", "/api/v1/../secrets"));
        assert!(!relay_allowed("GET", "/api/v1//cameras"));
        assert!(!relay_allowed("TRACE", "/api/v1/cameras"));
    }

    /// Regression for the relay allowlist bypass: the HTTP client normalizes `%2e%2e` to `..` and
    /// removes dot-segments, so a raw path with no literal ".." can still be SENT as an escaped path.
    /// `replay_relay_job` defends by canonicalizing via `Url::join` and running the allowlist on the
    /// canonical path — this pins that the canonical form of the known bypasses is refused.
    #[test]
    fn relay_allowlist_runs_on_canonical_path_not_raw() {
        let canon = |p: &str| {
            reqwest::Url::parse("http://127.0.0.1:8088")
                .unwrap()
                .join(p)
                .unwrap()
                .path()
                .to_string()
        };
        // The attack path passes the naive raw-string check (no literal "..") ...
        assert!(!"/api/v1/%2e%2e/%2e%2e/metrics".contains(".."));
        // ... but the client canonicalizes it to an off-surface path the allowlist rejects.
        assert_eq!(canon("/api/v1/%2e%2e/%2e%2e/metrics"), "/metrics");
        assert!(!relay_allowed(
            "GET",
            &canon("/api/v1/%2e%2e/%2e%2e/metrics")
        ));
        assert!(!relay_allowed(
            "GET",
            &canon("/api/v1/cameras/%2e%2e/relay/poll")
        ));
        assert!(!relay_allowed(
            "POST",
            &canon("/api/v1/cameras/%2e%2e/%2e%2e/healthz")
        ));
        // Legitimate paths still pass after canonicalization.
        assert!(relay_allowed("GET", &canon("/api/v1/cameras")));
        assert!(relay_allowed(
            "GET",
            &canon("/media/recordings/cam2/seg.mp4")
        ));
    }

    #[test]
    fn endpoints_append_paths_and_trim_trailing_slash() {
        assert_eq!(
            poll_url("https://rv.example.com"),
            "https://rv.example.com/api/v1/rendezvous/poll"
        );
        assert_eq!(
            answer_url("https://rv.example.com/"),
            "https://rv.example.com/api/v1/rendezvous/answer"
        );
    }
}
