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

use std::time::Duration;

use anyhow::Context;
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

/// One long-poll cycle: ask for the next session; if one arrives, bridge it and report the answer (or the
/// error) back. Returns Ok on a clean cycle (work handled or nothing pending) so the caller re-polls
/// immediately; Err only on a transport failure, which the caller backs off on.
async fn poll_once(
    state: &AppState,
    client: &reqwest::Client,
    rendezvous_url: &str,
    site_id: &str,
    token: &str,
) -> anyhow::Result<()> {
    let resp = client
        .post(poll_url(rendezvous_url))
        .bearer_auth(token)
        .json(&json!({ "site_id": site_id }))
        .send()
        .await
        .context("rendezvous poll request")?;
    if resp.status() == StatusCode::NO_CONTENT {
        return Ok(()); // long-poll timed out with no work — re-poll
    }
    let session: PendingSession = resp
        .error_for_status()
        .context("rendezvous poll rejected")?
        .json()
        .await
        .context("decoding pending session")?;

    let result = bridge_to_local_whep(state, &session.camera_id, &session.sdp_offer).await;
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
    Ok(())
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

    tracing::info!(site = %site_id, rendezvous = %rendezvous_url, "webrtc rendezvous: dialing out for remote viewing");
    let mut backoff = Duration::from_secs(1);
    loop {
        match poll_once(&state, &client, rendezvous_url, site_id, &cfg.cp_token).await {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(e) => {
                tracing::warn!(site = %site_id, error = %e, "webrtc rendezvous poll failed; backing off");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
