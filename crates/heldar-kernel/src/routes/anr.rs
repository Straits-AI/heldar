//! ANR (Automatic Network Replenishment): persisted recording-gap listing + manual retry.
//!
//! Listing a camera's persisted recording gaps (the [`recording_gaps`](crate::models::RecordingGap)
//! rows the indexer detects + the ANR loop fills) is open to any authenticated principal. Resetting a
//! gap so the ANR loop retries it is a manager+ mutation and is written to the audit log.
//!
//! NOTE the path is `/recording-gaps` (not `/gaps`): `/api/v1/cameras/{id}/gaps`
//! ([`crate::routes::recordings`]) already serves COMPUTED coverage holes over a time window. This
//! surface exposes the PERSISTED gap table (with fill state) that ANR acts on — a distinct resource.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::models::RecordingGap;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/cameras/{id}/recording-gaps", get(list_gaps))
        .route(
            "/api/v1/cameras/{id}/recording-gaps/{gap_id}/retry",
            post(retry_gap),
        )
}

#[derive(Debug, Deserialize)]
pub struct GapQuery {
    /// Optional filter on fill state (`pending` | `filled` | `failed`).
    state: Option<String>,
    limit: Option<i64>,
}

/// List a camera's persisted recording gaps, newest first (viewer+).
///
/// These are the PERSISTED `recording_gaps` rows ANR acts on, with their fill state — not the
/// computed coverage holes `/api/v1/cameras/{id}/gaps` derives from the segment table.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/recording-gaps", tag = "recordings",
    operation_id = "listRecordingGaps",
    params(
        ("id" = String, Path, description = "Camera id"),
        ("state" = Option<String>, Query, description = "Filter on fill state: `pending` | `filled` | `failed`"),
        ("limit" = Option<i64>, Query, description = "Row cap, clamped to 1..=5000 (default 500)"),
    ),
    responses(
        (status = 200, description = "Persisted recording gaps, newest gap start first"),
        (status = 403, description = "Missing `video:playback`", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera, or one this credential does not hold", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_gaps(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Query(q): Query<GapQuery>,
) -> AppResult<Json<Vec<RecordingGap>>> {
    principal.require_cap(Cap::VideoPlayback, "view recording gaps")?;
    let _ = st.camera_for(&principal, &id).await?;
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);
    let rows =
        match q.state.as_deref() {
            Some(state) => {
                sqlx::query_as::<_, RecordingGap>(
                    "SELECT * FROM recording_gaps WHERE camera_id = ? AND fill_state = ?
             ORDER BY gap_start DESC LIMIT ?",
                )
                .bind(&id)
                .bind(state)
                .bind(limit)
                .fetch_all(&st.pool)
                .await?
            }
            None => sqlx::query_as::<_, RecordingGap>(
                "SELECT * FROM recording_gaps WHERE camera_id = ? ORDER BY gap_start DESC LIMIT ?",
            )
            .bind(&id)
            .bind(limit)
            .fetch_all(&st.pool)
            .await?,
        };
    Ok(Json(rows))
}

/// Reset a gap to `pending` (clearing attempts/result) so the ANR loop retries it (manager+).
///
/// This QUEUES the fill, it does not perform it: the ANR loop picks the row up on a later pass, so
/// the returned row always reads `pending`.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/recording-gaps/{gap_id}/retry", tag = "recordings",
    operation_id = "retryRecordingGap",
    params(
        ("id" = String, Path, description = "Camera id"),
        ("gap_id" = String, Path, description = "Recording-gap id"),
    ),
    responses(
        (status = 200, description = "The gap, reset to `pending`"),
        (status = 403, description = "Missing `registry:manage`", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera (or one this credential does not hold), or no such gap on it", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn retry_gap(
    State(st): State<AppState>,
    Path((id, gap_id)): Path<(String, String)>,
    principal: Principal,
) -> AppResult<Json<RecordingGap>> {
    principal.require(principal.can_manage_registry(), "retry recording-gap fill")?;
    let _ = st.camera_for(&principal, &id).await?;
    let res = sqlx::query(
        "UPDATE recording_gaps
            SET fill_state = 'pending', fill_attempts = 0, last_attempt_at = NULL, filled_at = NULL
          WHERE id = ? AND camera_id = ?",
    )
    .bind(&gap_id)
    .bind(&id)
    .execute(&st.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "recording gap {gap_id} not found"
        )));
    }
    auth::audit(
        &st.pool,
        &principal,
        "anr_retry_gap",
        "recording_gap",
        &gap_id,
        json!({ "camera_id": id }),
    )
    .await;
    let gap = sqlx::query_as::<_, RecordingGap>("SELECT * FROM recording_gaps WHERE id = ?")
        .bind(&gap_id)
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(gap))
}
