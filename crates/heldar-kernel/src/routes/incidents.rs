//! Evidence-lock + incident tagging on recorded segments.
//!
//! A durable evidence hold (`segments.evidence_locked`) pins footage so the retention sweeper never
//! prunes it (distinct from the transient `locked` read-lock used by clip/snapshot export, which is
//! wiped at startup). Segments can be tagged with a free-form `incident_id` so evidence can be
//! grouped into a case and reviewed together. Locking/tagging is a manager+ mutation and is written
//! to the immutable audit log; reading the incident roll-up is open to any authenticated principal.

use axum::extract::{Path, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::Segment;
use crate::repo;
use crate::routes::recordings::SegmentView;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/segments/{id}/evidence-lock",
            post(lock_evidence).delete(unlock_evidence),
        )
        .route("/api/v1/segments/{id}/incident", patch(tag_incident))
        .route("/api/v1/incidents", get(list_incidents))
        .route(
            "/api/v1/incidents/{incident_id}/segments",
            get(incident_segments),
        )
}

/// Lock body: an optional incident tag to attach when pinning the segment.
#[derive(Debug, Deserialize)]
struct EvidenceLockBody {
    incident_id: Option<String>,
}

/// Tag body: the incident to set, or JSON `null` to clear the tag.
#[derive(Debug, Deserialize)]
struct IncidentTagBody {
    #[serde(default)]
    incident_id: Option<String>,
}

/// Roll-up of one incident: how many segments are tagged to it, their footprint, and span.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct IncidentSummary {
    pub incident_id: String,
    pub segment_count: i64,
    pub total_bytes: i64,
    pub oldest_start: DateTime<Utc>,
    pub newest_end: DateTime<Utc>,
}

/// Trim an optional incident id, treating blank/whitespace as absent (no tag).
fn norm_incident(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Load a segment by id or 404.
async fn load_segment(pool: &sqlx::SqlitePool, id: &str) -> AppResult<Segment> {
    sqlx::query_as::<_, Segment>("SELECT * FROM segments WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("segment {id} not found")))
}

async fn lock_evidence(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<EvidenceLockBody>,
) -> AppResult<Json<SegmentView>> {
    principal.require(principal.can_manage_registry(), "evidence-lock segments")?;
    let _ = load_segment(&st.pool, &id).await?;
    let incident_id = norm_incident(body.incident_id);
    repo::set_evidence_locked(&st.pool, &id, true, incident_id.as_deref()).await?;
    auth::audit(
        &st.pool,
        &principal,
        "evidence_lock_segment",
        "segment",
        &id,
        json!({ "incident_id": incident_id }),
    )
    .await;
    let seg = load_segment(&st.pool, &id).await?;
    Ok(Json(SegmentView::new(seg)))
}

async fn unlock_evidence(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<SegmentView>> {
    principal.require(principal.can_manage_registry(), "evidence-unlock segments")?;
    let _ = load_segment(&st.pool, &id).await?;
    // incident_id is preserved (COALESCE) so the case tag survives unlocking.
    repo::set_evidence_locked(&st.pool, &id, false, None).await?;
    auth::audit(
        &st.pool,
        &principal,
        "evidence_unlock_segment",
        "segment",
        &id,
        json!({}),
    )
    .await;
    let seg = load_segment(&st.pool, &id).await?;
    Ok(Json(SegmentView::new(seg)))
}

async fn tag_incident(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<IncidentTagBody>,
) -> AppResult<Json<SegmentView>> {
    principal.require(principal.can_manage_registry(), "tag segment incidents")?;
    let _ = load_segment(&st.pool, &id).await?;
    let incident_id = norm_incident(body.incident_id);
    // Direct set/clear (not COALESCE): a null/blank tag clears the association.
    sqlx::query("UPDATE segments SET incident_id = ? WHERE id = ?")
        .bind(&incident_id)
        .bind(&id)
        .execute(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "tag_segment_incident",
        "segment",
        &id,
        json!({ "incident_id": incident_id }),
    )
    .await;
    let seg = load_segment(&st.pool, &id).await?;
    Ok(Json(SegmentView::new(seg)))
}

async fn list_incidents(
    State(st): State<AppState>,
    _principal: Principal,
) -> AppResult<Json<Vec<IncidentSummary>>> {
    let rows = sqlx::query_as::<_, IncidentSummary>(
        "SELECT incident_id,
                COUNT(*) AS segment_count,
                COALESCE(SUM(size_bytes), 0) AS total_bytes,
                MIN(start_time) AS oldest_start,
                MAX(end_time) AS newest_end
         FROM segments
         WHERE incident_id IS NOT NULL
         GROUP BY incident_id
         ORDER BY newest_end DESC
         LIMIT 1000",
    )
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

/// Defensive upper bound on segments returned for one incident roll-up. Generous (an incident is a
/// handful of pinned clips in practice); a hit is logged so truncation is never silent.
const INCIDENT_SEGMENTS_CAP: i64 = 5000;

async fn incident_segments(
    State(st): State<AppState>,
    Path(incident_id): Path<String>,
    _principal: Principal,
) -> AppResult<Json<Vec<SegmentView>>> {
    let segments = sqlx::query_as::<_, Segment>(
        "SELECT * FROM segments WHERE incident_id = ? ORDER BY start_time ASC LIMIT ?",
    )
    .bind(&incident_id)
    .bind(INCIDENT_SEGMENTS_CAP)
    .fetch_all(&st.pool)
    .await?;
    if segments.len() as i64 >= INCIDENT_SEGMENTS_CAP {
        tracing::warn!(
            incident_id = %incident_id,
            cap = INCIDENT_SEGMENTS_CAP,
            "incident segment query hit the row cap; results may be truncated"
        );
    }
    let views = segments.into_iter().map(SegmentView::new).collect();
    Ok(Json(views))
}
