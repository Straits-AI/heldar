//! Snapshot-schedule CRUD + a query over captured snapshots.
//!
//! A schedule fires a live-frame capture for its camera every `interval_seconds`; the background
//! scheduler writes the frame and records a row in `snapshots`. Schedules are managed by manager+;
//! any authenticated principal can list schedules and captured snapshots.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{
    PersistedSnapshot, SnapshotSchedule, SnapshotScheduleCreate, SnapshotScheduleUpdate,
};
use crate::routes::cameras::load_camera;
use crate::state::AppState;
use crate::util;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/cameras/{id}/snapshot-schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/snapshot-schedules/{schedule_id}",
            axum::routing::patch(update_schedule).delete(delete_schedule),
        )
        .route("/api/v1/cameras/{id}/snapshots", get(list_snapshots))
}

/// Clamp an interval into a sane range (>= 5s avoids hammering the camera; cap at ~24h).
fn clamp_interval(seconds: i64) -> i64 {
    seconds.clamp(5, 86_400)
}

async fn list_schedules(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<SnapshotSchedule>>> {
    let _ = load_camera(&st.pool, &id).await?;
    let rows = sqlx::query_as::<_, SnapshotSchedule>(
        "SELECT * FROM snapshot_schedules WHERE camera_id = ? ORDER BY created_at ASC",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

async fn create_schedule(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<SnapshotScheduleCreate>,
) -> AppResult<(StatusCode, Json<SnapshotSchedule>)> {
    principal.require(principal.can_manage_registry(), "create snapshot schedules")?;
    let _ = load_camera(&st.pool, &id).await?;

    let interval = clamp_interval(body.interval_seconds.unwrap_or(300));
    let enabled = body.enabled.unwrap_or(true);
    let now = Utc::now();
    let schedule_id = format!("snsch_{}", Uuid::new_v4().simple());

    sqlx::query(
        "INSERT INTO snapshot_schedules
           (id, camera_id, interval_seconds, enabled, last_fired_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, NULL, ?, ?)",
    )
    .bind(&schedule_id)
    .bind(&id)
    .bind(interval)
    .bind(enabled)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;

    let schedule =
        sqlx::query_as::<_, SnapshotSchedule>("SELECT * FROM snapshot_schedules WHERE id = ?")
            .bind(&schedule_id)
            .fetch_one(&st.pool)
            .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_snapshot_schedule",
        "snapshot_schedule",
        &schedule_id,
        json!({ "camera_id": &id, "interval_seconds": interval, "enabled": enabled }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(schedule)))
}

async fn update_schedule(
    State(st): State<AppState>,
    Path(schedule_id): Path<String>,
    principal: Principal,
    Json(body): Json<SnapshotScheduleUpdate>,
) -> AppResult<Json<SnapshotSchedule>> {
    principal.require(principal.can_manage_registry(), "update snapshot schedules")?;
    let cur =
        sqlx::query_as::<_, SnapshotSchedule>("SELECT * FROM snapshot_schedules WHERE id = ?")
            .bind(&schedule_id)
            .fetch_optional(&st.pool)
            .await?
            .ok_or_else(|| {
                AppError::NotFound(format!("snapshot schedule {schedule_id} not found"))
            })?;

    let interval = clamp_interval(body.interval_seconds.unwrap_or(cur.interval_seconds));
    let enabled = body.enabled.unwrap_or(cur.enabled);

    sqlx::query(
        "UPDATE snapshot_schedules SET interval_seconds = ?, enabled = ?, updated_at = ? WHERE id = ?",
    )
    .bind(interval)
    .bind(enabled)
    .bind(Utc::now())
    .bind(&schedule_id)
    .execute(&st.pool)
    .await?;

    let schedule =
        sqlx::query_as::<_, SnapshotSchedule>("SELECT * FROM snapshot_schedules WHERE id = ?")
            .bind(&schedule_id)
            .fetch_one(&st.pool)
            .await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_snapshot_schedule",
        "snapshot_schedule",
        &schedule_id,
        json!({ "interval_seconds": interval, "enabled": enabled }),
    )
    .await;
    Ok(Json(schedule))
}

async fn delete_schedule(
    State(st): State<AppState>,
    Path(schedule_id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete snapshot schedules")?;
    let res = sqlx::query("DELETE FROM snapshot_schedules WHERE id = ?")
        .bind(&schedule_id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "snapshot schedule {schedule_id} not found"
        )));
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_snapshot_schedule",
        "snapshot_schedule",
        &schedule_id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct SnapshotRangeQuery {
    from: Option<String>,
    to: Option<String>,
    limit: Option<i64>,
}

/// A captured snapshot row plus its browser-fetchable media URL. Flattens [`PersistedSnapshot`]
/// (new model fields flow through), mirroring how [`crate::routes::recordings::SegmentView`] wraps a
/// segment with its served URL.
#[derive(Debug, Serialize)]
pub struct SnapshotView {
    #[serde(flatten)]
    snap: PersistedSnapshot,
    /// Browser-fetchable URL for the snapshot file (under /media/snapshots/...).
    url: String,
}

impl SnapshotView {
    fn new(snap: PersistedSnapshot) -> Self {
        let file = std::path::Path::new(&snap.path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let url = format!("/media/snapshots/{}/{}", snap.camera_id, file);
        SnapshotView { snap, url }
    }
}

async fn list_snapshots(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SnapshotRangeQuery>,
) -> AppResult<Json<Vec<SnapshotView>>> {
    let _ = load_camera(&st.pool, &id).await?;
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);
    let parse = |s: &Option<String>, field: &str| -> AppResult<Option<DateTime<Utc>>> {
        match s {
            Some(v) => util::parse_rfc3339(v)
                .map(Some)
                .ok_or_else(|| AppError::BadRequest(format!("invalid `{field}` timestamp"))),
            None => Ok(None),
        }
    };
    let from = parse(&q.from, "from")?;
    let to = parse(&q.to, "to")?;
    if let (Some(f), Some(t)) = (from, to) {
        if f > t {
            return Err(AppError::BadRequest("`from` must be <= `to`".into()));
        }
    }

    let rows = sqlx::query_as::<_, PersistedSnapshot>(
        "SELECT * FROM snapshots
         WHERE camera_id = ?
           AND (? IS NULL OR taken_at >= ?)
           AND (? IS NULL OR taken_at <= ?)
         ORDER BY taken_at DESC LIMIT ?",
    )
    .bind(&id)
    .bind(from)
    .bind(from)
    .bind(to)
    .bind(to)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;

    let views = rows.into_iter().map(SnapshotView::new).collect();
    Ok(Json(views))
}
