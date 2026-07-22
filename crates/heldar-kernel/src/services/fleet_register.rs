//! Edge-side fleet self-registration.
//!
//! When this node is configured with a control-plane URL (`HELDAR_CP_URL`), its own reachable URL
//! (`HELDAR_PUBLIC_BASE_URL`) and a site id (`HELDAR_SITE_ID`), it POSTs its identity to the control
//! plane's `POST /api/v1/fleet/nodes` on boot and on a heartbeat cadence. The control plane then drains
//! this node's outbox without any static config or restart — "add a node and it joins the fleet".
//!
//! Registration is idempotent (the control plane upserts on node id), so the heartbeat also re-teaches
//! a control plane that restarted or lost its registry. The reported token is the bearer the control
//! plane presents when draining this node's outbox; when this node runs with auth disabled (the LAN
//! default) it can be empty.
//!
//! This is a pure outbound HTTP client — the kernel has NO code dependency on the control-plane crate;
//! the only seam is the configurable URL. The fleet is strictly opt-in: with `HELDAR_CP_URL` unset (the
//! default) this loop parks forever and the node never phones home.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde_json::{json, Value};

use crate::config::Config;

/// The control-plane registration endpoint for a given base URL (trailing slash tolerated).
fn register_url(cp_url: &str) -> String {
    format!("{}/api/v1/fleet/nodes", cp_url.trim_end_matches('/'))
}

/// The registration body this node reports: its fleet identity, the URL the control plane should reach
/// it on, and the bearer the control plane presents when draining this node's outbox.
fn register_body(site_id: &str, base_url: &str, token: &str) -> Value {
    json!({ "id": site_id, "base_url": base_url, "token": token })
}

/// Build the HTTP client used to register, configuring mTLS (client identity + control-plane CA) when
/// `HELDAR_CP_TLS_*` is set. Errors only on unreadable/invalid cert material.
fn build_client(cfg: &Config) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(10));
    if let Some(t) = &cfg.cp_tls {
        let cert = std::fs::read(&t.client_cert)
            .with_context(|| format!("reading client cert {}", t.client_cert.display()))?;
        let key = std::fs::read(&t.client_key)
            .with_context(|| format!("reading client key {}", t.client_key.display()))?;
        let ca = std::fs::read(&t.server_ca)
            .with_context(|| format!("reading control-plane CA {}", t.server_ca.display()))?;
        // reqwest's PEM identity wants key + cert chain in one buffer.
        let mut identity_pem = key;
        identity_pem.extend_from_slice(&cert);
        let identity =
            reqwest::Identity::from_pem(&identity_pem).context("building client identity")?;
        let root = reqwest::Certificate::from_pem(&ca).context("parsing control-plane CA")?;
        builder = builder.identity(identity).add_root_certificate(root);
    }
    builder.build().context("building HTTP client")
}

/// Edge-side self-registration loop. Parks forever unless fully configured for the fleet (control-plane
/// URL + this node's site id + this node's reachable URL); otherwise POSTs its identity on boot and on
/// every heartbeat. Never returns (returning would have the supervisor respawn it).
pub async fn run(cfg: Arc<Config>) {
    let (Some(cp_url), Some(site_id), Some(base_url)) = (
        cfg.cp_url.as_deref(),
        cfg.site_id.as_deref(),
        cfg.public_base_url.as_deref(),
    ) else {
        // Not configured for the fleet (any of CP URL / site id / reachable URL missing): park.
        std::future::pending::<()>().await;
        return;
    };

    let client = match build_client(&cfg) {
        Ok(c) => c,
        Err(e) => {
            // A persistent cert-config error: park rather than tight-loop respawning.
            tracing::error!(error = %e, "fleet self-registration disabled: bad mTLS config");
            std::future::pending::<()>().await;
            return;
        }
    };
    let url = register_url(cp_url);
    let body = register_body(site_id, base_url, &cfg.cp_token);
    // `interval`'s first tick fires immediately → register on boot, then heartbeat on cadence.
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.cp_register_interval_s.max(1)));
    loop {
        tick.tick().await;
        match client.post(&url).json(&body).send().await {
            Ok(r) if r.status().is_success() => {
                tracing::debug!(node = %site_id, control_plane = %cp_url, "registered with fleet control plane")
            }
            Ok(r) => {
                tracing::warn!(node = %site_id, status = %r.status(), "fleet registration rejected")
            }
            Err(e) => {
                tracing::warn!(node = %site_id, error = %e, "fleet registration failed; retry next heartbeat")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_url_appends_path_and_trims_trailing_slash() {
        assert_eq!(
            register_url("http://cp.lan:9100"),
            "http://cp.lan:9100/api/v1/fleet/nodes"
        );
        assert_eq!(
            register_url("http://cp.lan:9100/"),
            "http://cp.lan:9100/api/v1/fleet/nodes"
        );
    }

    #[test]
    fn register_body_carries_identity_url_and_token() {
        let b = register_body("site-a", "https://edge-a.lan", "tok");
        assert_eq!(b["id"], "site-a");
        assert_eq!(b["base_url"], "https://edge-a.lan");
        assert_eq!(b["token"], "tok");
    }
}
