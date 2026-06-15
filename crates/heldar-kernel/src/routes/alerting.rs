//! UI-configurable alerting: read/write the webhook + severity threshold the notifier delivers
//! warning/critical events to. Settings live in the `app_state` key/value table so changes take
//! effect WITHOUT a restart (the notifier re-reads them every cycle). The env
//! `HELDAR_ALERT_WEBHOOK_URL` remains a read-only fallback when nothing is stored.
//!
//! Reads (GET) are open to any authenticated principal (`can_view`); the PUT + test POST are gated by
//! manager+ (`can_manage_registry`) and written to the immutable audit log. The webhook url is MASKED
//! on read (`scheme://host` + ellipsis) so its path/token is never exposed.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{self, AlertingConfig, AlertingUpdate};
use crate::services::alerting;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/system/alerting",
            get(get_alerting).put(put_alerting),
        )
        .route("/api/v1/system/alerting/test", post(test_alerting))
}

/// Resolve the current settings (stored, with the env webhook as a fallback) into the masked view.
async fn current_config(st: &AppState) -> AlertingConfig {
    let resolved = alerting::resolve(&st.pool, st.cfg.alert_webhook_url.as_deref()).await;
    AlertingConfig {
        configured: resolved.webhook_url.is_some(),
        webhook_url_masked: resolved
            .webhook_url
            .as_deref()
            .and_then(models::mask_webhook_url),
        enabled: resolved.enabled,
        min_severity: resolved.min_severity,
    }
}

async fn get_alerting(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<AlertingConfig>> {
    principal.require(principal.can_view(), "view alerting configuration")?;
    Ok(Json(current_config(&st).await))
}

async fn put_alerting(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<AlertingUpdate>,
) -> AppResult<Json<AlertingConfig>> {
    principal.require(principal.can_manage_registry(), "configure alerting")?;

    // Validate min_severity BEFORE any write, so a bad value leaves the stored config untouched.
    if let Some(sev) = body.min_severity.as_deref() {
        if !matches!(sev.trim(), "warning" | "critical") {
            return Err(AppError::BadRequest(
                "min_severity must be `warning` or `critical`".into(),
            ));
        }
    }

    // webhook_url three-state: absent = unchanged; empty/null = clear; otherwise set (trimmed).
    if let Some(opt) = body.webhook_url {
        let trimmed = opt.as_deref().map(str::trim).unwrap_or("");
        if trimmed.is_empty() {
            alerting::clear_state(&st.pool, alerting::WEBHOOK_KEY).await?;
        } else {
            alerting::set_state(&st.pool, alerting::WEBHOOK_KEY, trimmed).await?;
        }
    }
    if let Some(enabled) = body.enabled {
        alerting::set_state(
            &st.pool,
            alerting::ENABLED_KEY,
            if enabled { "true" } else { "false" },
        )
        .await?;
    }
    if let Some(sev) = body.min_severity {
        alerting::set_state(&st.pool, alerting::MIN_SEVERITY_KEY, sev.trim()).await?;
    }

    let cfg = current_config(&st).await;
    auth::audit(
        &st.pool,
        &principal,
        "set_alerting",
        "system",
        "alerting",
        json!({
            "configured": cfg.configured,
            "enabled": cfg.enabled,
            "min_severity": cfg.min_severity,
        }),
    )
    .await;
    Ok(Json(cfg))
}

/// Result of POST /api/v1/system/alerting/test — a synthetic event delivery to the live webhook.
#[derive(Debug, Serialize)]
struct AlertingTestResult {
    ok: bool,
    status: Option<u16>,
    error: Option<String>,
}

async fn test_alerting(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<AlertingTestResult>> {
    principal.require(principal.can_manage_registry(), "test alerting")?;
    let resolved = alerting::resolve(&st.pool, st.cfg.alert_webhook_url.as_deref()).await;
    let Some(url) = resolved.webhook_url else {
        return Err(AppError::NotFound("no alert webhook is configured".into()));
    };
    // Mirror the notifier's delivery envelope so the test exercises the same shape downstream.
    let payload = json!({
        "source": "heldar-core",
        "event_type": "test",
        "severity": "warning",
        "payload": { "message": "Heldar alerting test" },
        "timestamp": Utc::now(),
    });
    auth::audit(
        &st.pool,
        &principal,
        "test_alerting",
        "system",
        "alerting",
        json!({ "configured": true }),
    )
    .await;
    let result = match st.http.post(&url).json(&payload).send().await {
        Ok(resp) => {
            let status = resp.status();
            AlertingTestResult {
                ok: status.is_success(),
                status: Some(status.as_u16()),
                error: if status.is_success() {
                    None
                } else {
                    Some(format!("webhook returned HTTP {}", status.as_u16()))
                },
            }
        }
        Err(e) => AlertingTestResult {
            ok: false,
            status: None,
            error: Some(e.to_string()),
        },
    };
    Ok(Json(result))
}
