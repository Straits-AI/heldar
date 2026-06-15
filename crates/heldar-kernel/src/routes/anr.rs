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

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::RecordingGap;
use crate::routes::cameras::load_camera;
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
struct GapQuery {
    /// Optional filter on fill state (`pending` | `filled` | `failed`).
    state: Option<String>,
    limit: Option<i64>,
}

/// List a camera's persisted recording gaps, newest first (viewer+).
async fn list_gaps(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Query(q): Query<GapQuery>,
) -> AppResult<Json<Vec<RecordingGap>>> {
    principal.require(principal.can_view(), "view recording gaps")?;
    let _ = load_camera(&st.pool, &id).await?;
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
async fn retry_gap(
    State(st): State<AppState>,
    Path((id, gap_id)): Path<(String, String)>,
    principal: Principal,
) -> AppResult<Json<RecordingGap>> {
    principal.require(principal.can_manage_registry(), "retry recording-gap fill")?;
    let _ = load_camera(&st.pool, &id).await?;
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
