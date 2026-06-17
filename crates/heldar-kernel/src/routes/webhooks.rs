//! Webhook subscription management + the event-type taxonomy.
//!
//! Subscriptions are the generic external-delivery surface that supersedes the single-URL alerting
//! webhook: the background engine ([`crate::services::webhooks`]) delivers events to each enabled
//! subscription with HMAC signing and at-least-once retry. Listings + the delivery log are readable by
//! any authenticated principal (`can_view`); create/update/delete/test are gated by manager+
//! (`can_manage_registry`) and written to the immutable audit log. The signing `secret` is MASKED on
//! read (surfaced only as `has_secret`) and never echoed back.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{
    WebhookDelivery, WebhookSubscription, WebhookSubscriptionCreate, WebhookSubscriptionUpdate,
    WebhookSubscriptionView,
};
use crate::services::webhooks;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/webhooks", get(list).post(create))
        .route(
            "/api/v1/webhooks/{id}",
            axum::routing::patch(update).delete(delete),
        )
        .route("/api/v1/webhooks/{id}/test", post(test))
        .route("/api/v1/webhooks/{id}/deliveries", get(list_deliveries))
        .route("/api/v1/events/types", get(event_types))
}

const VALID_SEVERITIES: &[&str] = &["info", "warning", "critical"];

fn valid_severity(s: &str) -> bool {
    VALID_SEVERITIES.contains(&s)
}

/// Validate + normalize a webhook url. Only http(s) targets are accepted (no `file://`, etc.).
fn validate_url(url: &str) -> AppResult<String> {
    let url = url.trim();
    if url.is_empty() {
        return Err(AppError::BadRequest("`url` is required".into()));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AppError::BadRequest("`url` must be an http(s) URL".into()));
    }
    Ok(url.to_string())
}

/// Validate + normalize an event-type filter. `None`/empty = all types (`["*"]`); otherwise each entry
/// must be a non-empty token (deduped, trimmed).
fn normalize_event_types(types: Option<Vec<String>>) -> AppResult<Vec<String>> {
    let Some(types) = types else {
        return Ok(vec!["*".to_string()]);
    };
    let mut out: Vec<String> = Vec::with_capacity(types.len());
    for t in types {
        let t = t.trim().to_string();
        if t.is_empty() {
            return Err(AppError::BadRequest(
                "`event_types` entries must be non-empty".into(),
            ));
        }
        if !out.contains(&t) {
            out.push(t);
        }
    }
    if out.is_empty() {
        out.push("*".to_string());
    }
    Ok(out)
}

async fn load_subscription(pool: &sqlx::SqlitePool, id: &str) -> AppResult<WebhookSubscription> {
    sqlx::query_as::<_, WebhookSubscription>("SELECT * FROM webhook_subscriptions WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("webhook subscription {id} not found")))
}

async fn list(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<WebhookSubscriptionView>>> {
    principal.require(principal.can_view(), "view webhook subscriptions")?;
    let rows = sqlx::query_as::<_, WebhookSubscription>(
        "SELECT * FROM webhook_subscriptions ORDER BY created_at ASC",
    )
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(WebhookSubscriptionView::from)
            .collect(),
    ))
}

async fn create(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<WebhookSubscriptionCreate>,
) -> AppResult<(StatusCode, Json<WebhookSubscriptionView>)> {
    principal.require(
        principal.can_manage_registry(),
        "create webhook subscriptions",
    )?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    let url = validate_url(&body.url)?;
    let min_severity = body.min_severity.unwrap_or_else(|| "info".into());
    if !valid_severity(&min_severity) {
        return Err(AppError::BadRequest(
            "`min_severity` must be info|warning|critical".into(),
        ));
    }
    let event_types = normalize_event_types(body.event_types)?;
    let secret = body
        .secret
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let enabled = body.enabled.unwrap_or(true);
    let id = format!("whs_{}", Uuid::new_v4().simple());
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO webhook_subscriptions
           (id, name, url, event_types, min_severity, secret, enabled, cursor_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(&url)
    .bind(SqlxJson(&event_types))
    .bind(&min_severity)
    .bind(secret.as_deref())
    .bind(enabled)
    // cursor_at = now: deliver events from creation forward, never replay the backlog.
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;

    let sub = load_subscription(&st.pool, &id).await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_webhook",
        "webhook",
        &id,
        json!({
            "name": name,
            "event_types": &event_types,
            "min_severity": &min_severity,
            "has_secret": secret.is_some(),
            "enabled": enabled,
        }),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(WebhookSubscriptionView::from(sub)),
    ))
}

async fn update(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<WebhookSubscriptionUpdate>,
) -> AppResult<Json<WebhookSubscriptionView>> {
    principal.require(
        principal.can_manage_registry(),
        "update webhook subscriptions",
    )?;
    let cur = load_subscription(&st.pool, &id).await?;

    let name = match body.name {
        Some(n) => {
            let n = n.trim().to_string();
            if n.is_empty() {
                return Err(AppError::BadRequest("`name` must not be empty".into()));
            }
            n
        }
        None => cur.name,
    };
    let url = match body.url {
        Some(u) => validate_url(&u)?,
        None => cur.url,
    };
    let min_severity = match body.min_severity {
        Some(s) => {
            if !valid_severity(&s) {
                return Err(AppError::BadRequest(
                    "`min_severity` must be info|warning|critical".into(),
                ));
            }
            s
        }
        None => cur.min_severity,
    };
    let event_types = match body.event_types {
        Some(t) => normalize_event_types(Some(t))?,
        None => cur.event_types.0,
    };
    // Three-state secret: omitted = keep, null = clear, value = set (empty value also clears).
    let secret: Option<String> = match body.secret {
        None => cur.secret,
        Some(None) => None,
        Some(Some(s)) => {
            let s = s.trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
    };
    let enabled = body.enabled.unwrap_or(cur.enabled);

    sqlx::query(
        "UPDATE webhook_subscriptions
            SET name = ?, url = ?, event_types = ?, min_severity = ?, secret = ?, enabled = ?, updated_at = ?
          WHERE id = ?",
    )
    .bind(&name)
    .bind(&url)
    .bind(SqlxJson(&event_types))
    .bind(&min_severity)
    .bind(secret.as_deref())
    .bind(enabled)
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;

    let sub = load_subscription(&st.pool, &id).await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_webhook",
        "webhook",
        &id,
        json!({
            "name": &name,
            "event_types": &event_types,
            "min_severity": &min_severity,
            "has_secret": secret.is_some(),
            "enabled": enabled,
        }),
    )
    .await;
    Ok(Json(WebhookSubscriptionView::from(sub)))
}

async fn delete(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(
        principal.can_manage_registry(),
        "delete webhook subscriptions",
    )?;
    let res = sqlx::query("DELETE FROM webhook_subscriptions WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "webhook subscription {id} not found"
        )));
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_webhook",
        "webhook",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Result of POST /api/v1/webhooks/{id}/test — one synthetic signed delivery to the subscription's url.
#[derive(Debug, Serialize)]
struct WebhookTestResult {
    ok: bool,
    status: Option<u16>,
    error: Option<String>,
}

async fn test(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<WebhookTestResult>> {
    principal.require(
        principal.can_manage_registry(),
        "test webhook subscriptions",
    )?;
    let sub = load_subscription(&st.pool, &id).await?;

    let delivery_id = format!("whd_{}", Uuid::new_v4().simple());
    let body = json!({
        "id": &delivery_id,
        "camera_id": serde_json::Value::Null,
        "site_id": st.cfg.site_id.clone(),
        "event_type": "test",
        "severity": "info",
        "timestamp": Utc::now(),
        "payload": { "message": "Heldar webhook test" },
    });
    let res = webhooks::send_event(
        &st.http,
        &sub.url,
        &delivery_id,
        "test",
        sub.secret.as_deref(),
        &body,
    )
    .await;

    // Record the synthetic delivery (event_id NULL, so it never counts toward real-event retry bounds).
    webhooks::record_delivery(
        &st.pool,
        &delivery_id,
        &sub.id,
        None,
        Some("test"),
        res.ok,
        1,
        res.status.map(i64::from),
        res.error.as_deref(),
    )
    .await;

    auth::audit(
        &st.pool,
        &principal,
        "test_webhook",
        "webhook",
        &id,
        json!({ "ok": res.ok, "status": res.status }),
    )
    .await;
    Ok(Json(WebhookTestResult {
        ok: res.ok,
        status: res.status,
        error: res.error,
    }))
}

#[derive(Debug, Deserialize)]
struct DeliveriesQuery {
    limit: Option<i64>,
}

async fn list_deliveries(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Query(q): Query<DeliveriesQuery>,
) -> AppResult<Json<Vec<WebhookDelivery>>> {
    principal.require(principal.can_view(), "view webhook deliveries")?;
    let _ = load_subscription(&st.pool, &id).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let rows = sqlx::query_as::<_, WebhookDelivery>(
        "SELECT * FROM webhook_deliveries WHERE subscription_id = ? ORDER BY created_at DESC LIMIT ?",
    )
    .bind(&id)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

/// One known event type plus a one-line description (the built-in taxonomy).
#[derive(Debug, Serialize)]
struct EventTypeInfo {
    event_type: &'static str,
    description: &'static str,
}

/// The built-in event-type taxonomy emitted via `repo::log_event` across the kernel + bundled apps.
/// Apps and AI workers may additionally emit their own custom `event_type` strings (the AI ingest path
/// passes a worker-supplied type straight through), so this list is descriptive, not exhaustive.
async fn event_types(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<serde_json::Value>>> {
    principal.require(principal.can_view(), "view event types")?;
    let types = vec![
        EventTypeInfo {
            event_type: "camera_offline",
            description: "A camera's recorder lost its RTSP connection (camera went offline).",
        },
        EventTypeInfo {
            event_type: "recorder_error",
            description: "A camera's recorder process errored or its segments went stale.",
        },
        EventTypeInfo {
            event_type: "recording_gap",
            description: "A hole was detected between consecutive recorded segments.",
        },
        EventTypeInfo {
            event_type: "sampler_offline",
            description: "An AI frame sampler for a camera went offline.",
        },
        EventTypeInfo {
            event_type: "retention_delete",
            description: "Old segments were pruned by the retention sweeper (by age).",
        },
        EventTypeInfo {
            event_type: "disk_pressure",
            description:
                "Recording storage is under pressure (per-camera quota, size cap, or free-space floor).",
        },
        EventTypeInfo {
            event_type: "disk_smart_warning",
            description: "A SMART self-assessment reported a disk health warning.",
        },
        EventTypeInfo {
            event_type: "raid_degraded",
            description: "A Linux md/RAID array reported a degraded or down member.",
        },
        EventTypeInfo {
            event_type: "zone_enter",
            description: "A tracked detection entered a configured zone.",
        },
        EventTypeInfo {
            event_type: "zone_exit",
            description: "A tracked detection left a configured zone.",
        },
        EventTypeInfo {
            event_type: "zone_dwell",
            description: "A tracked detection dwelled inside a zone past its dwell threshold.",
        },
        EventTypeInfo {
            event_type: "entry_matched",
            description: "Access control: an entry matched the registry and was authorized.",
        },
        EventTypeInfo {
            event_type: "entry_exception",
            description: "Access control: an entry needs operator review (unmatched/low-confidence).",
        },
        EventTypeInfo {
            event_type: "entry_unmatched",
            description: "Access control: an entry did not match any registry record.",
        },
        EventTypeInfo {
            event_type: "entry_blocked",
            description: "Access control: an entry matched a watchlist/blocklist and was denied.",
        },
    ];
    // Start with the built-in taxonomy, then UNION in event types actually observed in the events table
    // (plugin/app-emitted types like `wasm.*`, which the static list can't know), so they appear in the
    // webhook subscription picker. Capped + best-effort: a query failure just returns the static set.
    let known: std::collections::HashSet<&str> = types.iter().map(|t| t.event_type).collect();
    let mut out: Vec<serde_json::Value> = types
        .iter()
        .map(|t| json!({ "event_type": t.event_type, "description": t.description }))
        .collect();
    if let Ok(rows) = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT event_type FROM events ORDER BY event_type LIMIT 500",
    )
    .fetch_all(&st.pool)
    .await
    {
        for ty in rows.into_iter().filter(|t| !known.contains(t.as_str())) {
            out.push(json!({ "event_type": ty, "description": "Observed at runtime (plugin/app-emitted)." }));
        }
    }
    Ok(Json(out))
}
