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

use crate::auth::{self, Principal};
use crate::error::AppResult;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/outbox", get(list_outbox))
        .route("/api/v1/site", get(site_info))
}

/// One durable outbox row (a committed detection batch). Mirrors the `outbox` table (migration 0006).
#[derive(Debug, Serialize, sqlx::FromRow)]
struct OutboxEntry {
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
struct OutboxPage {
    entries: Vec<OutboxEntry>,
    /// Highest `seq` in this page; null when the page is empty (caller is caught up).
    next_seq: Option<i64>,
    count: usize,
}

#[derive(Debug, Deserialize)]
struct OutboxQuery {
    /// Return rows with `seq` strictly greater than this cursor (default 0 = from the start).
    since_seq: Option<i64>,
    /// Page size (default 100, clamped 1..1000).
    limit: Option<i64>,
}

/// Drain the outbox in `seq` order from a cursor (admin-only, audited).
async fn list_outbox(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<OutboxQuery>,
) -> AppResult<Json<OutboxPage>> {
    principal.require(principal.can_admin(), "read the fleet outbox")?;
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

/// This node's fleet identity. No auth: it exposes only public build/site metadata (no secrets).
#[derive(Debug, Serialize)]
struct SiteInfo {
    site_id: Option<String>,
    name: &'static str,
    version: &'static str,
    started_at: DateTime<Utc>,
}

async fn site_info(State(st): State<AppState>) -> Json<SiteInfo> {
    Json(SiteInfo {
        site_id: st.cfg.site_id.clone(),
        name: "Heldar Core",
        version: env!("CARGO_PKG_VERSION"),
        started_at: st.started_at,
    })
}
