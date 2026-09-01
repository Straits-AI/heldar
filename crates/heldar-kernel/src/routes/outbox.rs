//! Fleet outbox foundation (open-core seam, Stage 0): the appliance-side read API over the durable,
//! ordered transactional outbox (`outbox` table) plus a tiny unauthenticated site-identity endpoint.
//!
//! `GET /api/v1/outbox?since_seq=&limit=` is the cursor a future edge->cloud uplink (or an
//! out-of-process app) polls to drain committed detection batches in `seq` order WITHOUT running a
//! message broker on the box — the DB is the log. It is admin-only and audited. `GET /api/v1/site`
//! reports this node's identity (`HELDAR_SITE_ID`, build version, boot time) so a fleet controller can
//! correlate outbox cursors with the site they came from; it carries no secrets and needs no auth.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, Cap, Principal};
use crate::error::AppResult;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/outbox", get(list_outbox))
        .route("/api/v1/site", get(site_info))
}

/// One durable outbox row (a committed detection batch). Mirrors the `outbox` table (migration 0006).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct OutboxEntry {
    seq: i64,
    topic: String,
    camera_id: Option<String>,
    site_id: Option<String>,
    frame_id: Option<String>,
    task_type: Option<String>,
    detection_count: i64,
    created_at: DateTime<Utc>,
}

/// A page of outbox rows plus the cursor to continue from (pass `next_seq` as the next `since_seq`).
#[derive(Debug, Serialize)]
pub struct OutboxPage {
    entries: Vec<OutboxEntry>,
    /// Highest `seq` in this page; null when the page is empty (caller is caught up).
    next_seq: Option<i64>,
    count: usize,
}

#[derive(Debug, Deserialize)]
pub struct OutboxQuery {
    /// Return rows with `seq` strictly greater than this cursor (default 0 = from the start).
    since_seq: Option<i64>,
    /// Page size (default 100, clamped 1..1000).
    limit: Option<i64>,
}

/// Drain the outbox in `seq` order from a cursor (admin-only, audited).
///
/// A camera-scoped credential is REFUSED, not filtered: `seq` is a contiguous fleet-wide cursor, and
/// omitting rows would leave a consumer believing it had drained batches it never saw. Pass the
/// returned `next_seq` as the next `since_seq`; a null `next_seq` means the caller is caught up.
#[utoipa::path(
    get, path = "/api/v1/outbox", tag = "system",
    operation_id = "listOutbox",
    params(
        ("since_seq" = Option<i64>, Query, description = "Return rows with `seq` strictly greater than this cursor (default 0)"),
        ("limit" = Option<i64>, Query, description = "Page size, default 100, clamped to 1..1000"),
    ),
    responses(
        (status = 200, description = "A page of outbox rows in ascending `seq` order, plus the cursor to continue from"),
        (status = 403, description = "Not an admin, or a camera-scoped credential", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_outbox(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<OutboxQuery>,
) -> AppResult<Json<OutboxPage>> {
    principal.require(principal.can_admin(), "read the fleet outbox")?;
    // Refused rather than filtered. This is a FLEET drain with a monotonic `seq` cursor: filtering it
    // by camera would punch holes in the sequence, and a consumer that treats `seq` as contiguous
    // would silently believe it had drained batches it never saw. The surface is fleet-wide by
    // construction, so a camera-scoped credential has no coherent view of it.
    crate::routes::cameras::require_fleet_scope(&principal, "read the fleet outbox")?;
    let since = q.since_seq.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let entries = sqlx::query_as::<_, OutboxEntry>(
        "SELECT seq, topic, camera_id, site_id, frame_id, task_type, detection_count, created_at
           FROM outbox
          WHERE seq > ?
          ORDER BY seq ASC
          LIMIT ?",
    )
    .bind(since)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;

    let next_seq = entries.last().map(|e| e.seq);
    let count = entries.len();
    auth::audit(
        &st.pool,
        &principal,
        "read_outbox",
        "outbox",
        &format!("since:{since}"),
        json!({ "since_seq": since, "limit": limit, "returned": count }),
    )
    .await;
    Ok(Json(OutboxPage {
        entries,
        next_seq,
        count,
    }))
}

#[derive(Debug, Serialize)]
pub struct SiteInfo {
    site_id: Option<String>,
    name: &'static str,
    version: &'static str,
    started_at: DateTime<Utc>,
}

/// This node's fleet identity: `site_id`, build version and boot time.
///
/// Carries no secrets, but is not anonymous: when auth is disabled (the LAN default) the extractor
/// yields the synthetic admin so this stays open, while an auth-enabled box will not disclose the
/// site identity and version to an unauthenticated caller.
#[utoipa::path(
    get, path = "/api/v1/site", tag = "system",
    operation_id = "getSiteInfo",
    responses(
        (status = 200, description = "This node's site id, name, version and start time"),
        (status = 403, description = "Missing `system:read`", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn site_info(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<SiteInfo>> {
    principal.require_cap(Cap::SystemRead, "read site info")?;
    Ok(Json(SiteInfo {
        site_id: st.cfg.site_id.clone(),
        name: "Heldar Core",
        version: env!("CARGO_PKG_VERSION"),
        started_at: st.started_at,
    }))
}
