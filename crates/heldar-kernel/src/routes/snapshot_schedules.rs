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

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{
    PersistedSnapshot, SnapshotSchedule, SnapshotScheduleCreate, SnapshotScheduleUpdate,
};
use crate::state::{AppState, CameraOwned};
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

/// The snapshot schedules on a camera, oldest first.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/snapshot-schedules", tag = "cameras",
    operation_id = "listSnapshotSchedules",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "Schedules for this camera, oldest first", body = Vec<SnapshotSchedule>),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_schedules(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<SnapshotSchedule>>> {
    principal.require_cap(Cap::CameraRead, "list snapshot schedules")?;
    let _ = st.camera_for(&principal, &id).await?;
    let rows = sqlx::query_as::<_, SnapshotSchedule>(
        "SELECT * FROM snapshot_schedules WHERE camera_id = ? ORDER BY created_at ASC",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

/// Create a snapshot schedule on a camera.
///
/// `interval_seconds` is clamped to 5..=86400 rather than rejected, so the stored value can differ
/// from the one sent — read it back from the response.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/snapshot-schedules", tag = "cameras",
    operation_id = "createSnapshotSchedule",
    params(("id" = String, Path, description = "Camera id")),
    request_body = SnapshotScheduleCreate,
    responses(
        (status = 201, description = "The created schedule", body = SnapshotSchedule),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn create_schedule(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<SnapshotScheduleCreate>,
) -> AppResult<(StatusCode, Json<SnapshotSchedule>)> {
    principal.require(principal.can_manage_registry(), "create snapshot schedules")?;
    let _ = st.camera_for(&principal, &id).await?;

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

/// Update a snapshot schedule's interval or enabled flag.
///
/// Scope is resolved from the schedule's owning camera BEFORE the row is disclosed: to a
/// camera-scoped credential, someone else's schedule and a nonexistent one both answer 403 with the
/// same message. `interval_seconds` is clamped to 5..=86400.
#[utoipa::path(
    patch, path = "/api/v1/snapshot-schedules/{schedule_id}", tag = "cameras",
    operation_id = "updateSnapshotSchedule",
    params(("schedule_id" = String, Path, description = "Snapshot schedule id")),
    request_body = SnapshotScheduleUpdate,
    responses(
        (status = 200, description = "The updated schedule", body = SnapshotSchedule),
        (status = 403, description = "Missing `registry:manage`, or a schedule this credential does not hold — indistinguishable from an unknown one", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown schedule (fleet-scoped credentials only)", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn update_schedule(
    State(st): State<AppState>,
    Path(schedule_id): Path<String>,
    principal: Principal,
    Json(body): Json<SnapshotScheduleUpdate>,
) -> AppResult<Json<SnapshotSchedule>> {
    principal.require(principal.can_manage_registry(), "update snapshot schedules")?;
    // Owning camera before the row is disclosed — disabling this schedule silences another camera's
    // scheduled captures.
    let _ = st
        .resource_camera(
            &principal,
            CameraOwned::SnapshotSchedule,
            &schedule_id,
            "update snapshot schedules",
        )
        .await?;
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

/// Delete a snapshot schedule.
///
/// Scope is checked before the DELETE, so the 204-vs-404 shape cannot be used as an id-space oracle:
/// a camera-scoped credential gets the same 403 for someone else's schedule and for one that does
/// not exist.
#[utoipa::path(
    delete, path = "/api/v1/snapshot-schedules/{schedule_id}", tag = "cameras",
    operation_id = "deleteSnapshotSchedule",
    params(("schedule_id" = String, Path, description = "Snapshot schedule id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Missing `registry:manage`, or a schedule this credential does not hold — indistinguishable from an unknown one", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown schedule (fleet-scoped credentials only)", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn delete_schedule(
    State(st): State<AppState>,
    Path(schedule_id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete snapshot schedules")?;
    // Before the DELETE, so the 204-vs-404 shape below stops being an id-space oracle.
    let _ = st
        .resource_camera(
            &principal,
            CameraOwned::SnapshotSchedule,
            &schedule_id,
            "delete snapshot schedules",
        )
        .await?;
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
pub struct SnapshotRangeQuery {
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

/// Snapshots captured for a camera, newest first.
///
/// `limit` is clamped to 1..=5000 (default 500), so a larger value silently returns fewer rows.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/snapshots", tag = "recordings",
    operation_id = "listCameraSnapshots",
    params(
        ("id" = String, Path, description = "Camera id"),
        ("from" = Option<String>, Query, description = "RFC3339 lower bound on `taken_at`"),
        ("to" = Option<String>, Query, description = "RFC3339 upper bound on `taken_at`"),
        ("limit" = Option<i64>, Query, description = "Max rows, clamped to 1..=5000 (default 500)"),
    ),
    responses(
        (status = 200, description = "Captured snapshots, newest first, each with its `/media/snapshots/...` URL"),
        (status = 400, description = "Unparseable `from`/`to`, or `from` after `to`", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `video:playback`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_snapshots(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<SnapshotRangeQuery>,
) -> AppResult<Json<Vec<SnapshotView>>> {
    principal.require_cap(Cap::VideoPlayback, "list snapshots")?;
    let _ = st.camera_for(&principal, &id).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Scope;
    use std::collections::HashSet;
    use std::sync::Arc;

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = Arc::new(crate::config::Config::from_env());
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            media_jobs: crate::services::media_jobs::MediaJobGovernor::new(2),
            started_at: Utc::now(),
            pool,
            cfg,
        }
    }

    fn scoped(cameras: &[&str]) -> Principal {
        let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: Scope::Cameras(Arc::new(set)),
            ..Principal::system_admin()
        }
    }

    async fn seed(pool: &sqlx::SqlitePool, camera_id: &str, schedule_id: &str) {
        let now = Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
            .bind(camera_id)
            .bind(camera_id)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO snapshot_schedules
               (id, camera_id, interval_seconds, enabled, last_fired_at, created_at, updated_at)
             VALUES (?,?,300,1,NULL,?,?)",
        )
        .bind(schedule_id)
        .bind(camera_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_scoped_key_cannot_touch_another_cameras_snapshot_schedule() {
        let st = test_state().await;
        seed(&st.pool, "cam_a", "snsch_a").await;
        seed(&st.pool, "cam_sentinel_b", "snsch_b").await;
        let p = scoped(&["cam_a"]);

        let out_of_scope = delete_schedule(State(st.clone()), Path("snsch_b".into()), p.clone())
            .await
            .unwrap_err();
        let nonexistent = delete_schedule(State(st.clone()), Path("snsch_zzz".into()), p.clone())
            .await
            .unwrap_err();
        assert!(matches!(out_of_scope, AppError::Forbidden(_)));
        assert_eq!(out_of_scope.to_string(), nonexistent.to_string());
        assert!(!out_of_scope.to_string().contains("cam_sentinel_b"));

        let update: SnapshotScheduleUpdate =
            serde_json::from_value(json!({ "enabled": false })).unwrap();
        assert!(matches!(
            update_schedule(
                State(st.clone()),
                Path("snsch_b".into()),
                p.clone(),
                Json(update),
            )
            .await
            .unwrap_err(),
            AppError::Forbidden(_)
        ));

        // Still there, still firing.
        let enabled: i64 =
            sqlx::query_scalar("SELECT enabled FROM snapshot_schedules WHERE id = 'snsch_b'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(enabled, 1);

        // No over-blocking on its own camera.
        assert_eq!(
            delete_schedule(State(st.clone()), Path("snsch_a".into()), p)
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn an_unscoped_principal_is_unaffected() {
        let st = test_state().await;
        seed(&st.pool, "cam_sentinel_b", "snsch_b").await;
        let admin = Principal::system_admin();
        match delete_schedule(State(st.clone()), Path("snsch_zzz".into()), admin.clone())
            .await
            .unwrap_err()
        {
            AppError::NotFound(m) => assert_eq!(m, "snapshot schedule snsch_zzz not found"),
            other => panic!("expected the pre-existing 404, got {other:?}"),
        }
        assert_eq!(
            delete_schedule(State(st.clone()), Path("snsch_b".into()), admin)
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
    }
}
