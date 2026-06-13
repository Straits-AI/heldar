use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::models::{CameraStatus, Event};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/health/cameras", get(list_status))
        .route("/api/v1/cameras/{id}/health", get(camera_status))
        .route("/api/v1/events", get(list_events))
}

async fn list_status(State(st): State<AppState>) -> AppResult<Json<Vec<CameraStatus>>> {
    let rows =
        sqlx::query_as::<_, CameraStatus>("SELECT * FROM camera_status ORDER BY camera_id ASC")
            .fetch_all(&st.pool)
            .await?;
    Ok(Json(rows))
}

async fn camera_status(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<CameraStatus>> {
    let row = sqlx::query_as::<_, CameraStatus>("SELECT * FROM camera_status WHERE camera_id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("no status for camera {id}")))?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    camera_id: Option<String>,
    event_type: Option<String>,
    severity: Option<String>,
    limit: Option<i64>,
}

async fn list_events(
    State(st): State<AppState>,
    Query(q): Query<EventQuery>,
) -> AppResult<Json<Vec<Event>>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    let rows = sqlx::query_as::<_, Event>(
        "SELECT * FROM events
         WHERE (? IS NULL OR camera_id = ?)
           AND (? IS NULL OR event_type = ?)
           AND (? IS NULL OR severity = ?)
         ORDER BY timestamp DESC LIMIT ?",
    )
    .bind(&q.camera_id)
    .bind(&q.camera_id)
    .bind(&q.event_type)
    .bind(&q.event_type)
    .bind(&q.severity)
    .bind(&q.severity)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}
