use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::{Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{CameraStatus, Event};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/health/cameras", get(list_status))
        .route("/api/v1/cameras/{id}/health", get(camera_status))
        .route("/api/v1/events", get(list_events))
}

/// Live recorder health (state, last segment, reconnects, fps, bitrate, last error) per camera.
///
/// Confined to the cameras this credential holds — a scoped caller gets a short list, not a 403.
/// A disabled camera always reports `disabled`, overriding whatever stale state the recorder
/// teardown left in its row.
#[utoipa::path(
    get, path = "/api/v1/health/cameras", tag = "cameras",
    operation_id = "listCameraHealth",
    responses(
        (status = 200, description = "Health rows for the caller's cameras, by camera id"),
        (status = 403, description = "Missing `camera:read`", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_status(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<CameraStatus>>> {
    principal.require_cap(Cap::CameraRead, "view camera health")?;
    // Filtered like `list_cameras`: the per-camera twin of this route is scope-checked, so an
    // unfiltered list here hands a scoped credential the full roster plus live operational state
    // (recorder state, last_segment_at, reconnect_count, fps, bitrate, last_error) for every camera.
    let mut sql = "SELECT * FROM camera_status WHERE 1=1".to_string();
    let scope = crate::state::camera_scope_filter(&principal, "camera_id");
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY camera_id ASC");
    let mut q = sqlx::query_as::<_, CameraStatus>(&sql);
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        q = q.bind(id);
    }
    let mut rows = q.fetch_all(&st.pool).await?;
    // A disabled camera's recorder state is irrelevant and can be left stale by the async recorder
    // teardown (e.g. "recording"/"error" right after a disable); report it as "disabled" so the
    // health table is truthful.
    // Same filter: this roster query is a camera-id list in its own right.
    let mut dsql = "SELECT id FROM cameras WHERE enabled = 0".to_string();
    let dscope = crate::state::camera_scope_filter(&principal, "id");
    if let Some((pred, _)) = &dscope {
        dsql.push_str(pred);
    }
    let mut dq = sqlx::query_scalar::<_, String>(&dsql);
    for id in dscope.iter().flat_map(|(_, ids)| ids) {
        dq = dq.bind(id);
    }
    let disabled: std::collections::HashSet<String> =
        dq.fetch_all(&st.pool).await?.into_iter().collect();
    for r in &mut rows {
        if disabled.contains(&r.camera_id) {
            r.state = "disabled".into();
        }
    }
    Ok(Json(rows))
}

/// Live recorder health for one camera.
///
/// Scope is checked before the row is read, so an out-of-scope camera is a 403 and never a 404 —
/// the 404 here means only "no status row yet" (the recorder has never run for it). A disabled
/// camera reports `disabled` regardless of stale recorder state.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/health", tag = "cameras",
    operation_id = "getCameraHealth",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The camera's health row"),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "No status recorded for this camera yet", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn camera_status(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<CameraStatus>> {
    principal.require_cap(Cap::CameraRead, "view camera health")?;
    // Scope BEFORE the status row is read. Health leaks live operational state — recorder state,
    // last_segment_at, reconnect_count, fps, bitrate, last_error — which is a per-camera activity
    // feed, and answering 404 for an out-of-scope camera would also disclose whether it exists.
    st.camera_scope_check(&principal, &id)?;
    let mut row =
        sqlx::query_as::<_, CameraStatus>("SELECT * FROM camera_status WHERE camera_id = ?")
            .bind(&id)
            .fetch_optional(&st.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("no status for camera {id}")))?;
    // See list_status: report a disabled camera as "disabled" regardless of stale recorder state.
    let enabled: Option<bool> = sqlx::query_scalar("SELECT enabled FROM cameras WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?;
    if enabled == Some(false) {
        row.state = "disabled".into();
    }
    Ok(Json(row))
}

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    camera_id: Option<String>,
    event_type: Option<String>,
    severity: Option<String>,
    limit: Option<i64>,
}

/// The box event feed (camera offline, recorder errors, recording gaps, zone and ingest events),
/// newest first.
///
/// Confined to the caller's cameras. Box-level events with no camera (disk, RAID, system) are
/// deliberately withheld from a camera-scoped credential, so such a caller sees a strict subset of
/// what an unscoped one sees. `limit` defaults to 200 and is clamped to 1..=2000.
#[utoipa::path(
    get, path = "/api/v1/events", tag = "cameras",
    operation_id = "listEvents",
    params(
        ("camera_id" = Option<String>, Query, description = "Only events for this camera"),
        ("event_type" = Option<String>, Query, description = "Only events of this type"),
        ("severity" = Option<String>, Query, description = "Only events of this severity"),
        ("limit" = Option<i64>, Query, description = "Max rows (default 200, clamped to 1..=2000)"),
    ),
    responses(
        (status = 200, description = "Matching events, newest first"),
        (status = 403, description = "Missing `events:read`", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_events(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<EventQuery>,
) -> AppResult<Json<Vec<Event>>> {
    principal.require_cap(Cap::EventsRead, "view events")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    // Confine to the caller's cameras. The event feed carries camera_offline, recorder_error,
    // recording_gap, zone and ingest events for every camera — a per-camera activity feed for the
    // whole fleet, which is most of what camera scope exists to withhold.
    //
    // Events with a NULL camera_id (box-level: disk, RAID, system) are deliberately NOT shown to a
    // scoped credential: they are not attributable to a camera it holds, and fail-closed is the right
    // default for a surface whose whole purpose here is confinement.
    let mut sql = "SELECT * FROM events
         WHERE (? IS NULL OR camera_id = ?)
           AND (? IS NULL OR event_type = ?)
           AND (? IS NULL OR severity = ?)"
        .to_string();
    let scope = crate::state::camera_scope_filter(&principal, "camera_id");
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, Event>(&sql)
        .bind(&q.camera_id)
        .bind(&q.camera_id)
        .bind(&q.event_type)
        .bind(&q.event_type)
        .bind(&q.severity)
        .bind(&q.severity);
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    let rows = query.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::services::recorder::RecorderManager;
    use crate::services::sampler::SamplerManager;
    use std::sync::Arc;

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = Arc::new(Config::from_env());
        AppState {
            recorder: RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: SamplerManager::new(pool.clone(), cfg.clone()),
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
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    /// A disabled camera must report `disabled` even when its `camera_status` row was left at a stale
    /// `recording` (or `error`) by the async recorder teardown.
    #[tokio::test]
    async fn disabled_camera_reports_disabled_not_stale_recording() {
        let st = test_state().await;
        let now = chrono::Utc::now();
        for (id, enabled) in [("cam_on", 1), ("cam_off", 0)] {
            sqlx::query(
                "INSERT INTO cameras (id, name, enabled, created_at, updated_at) VALUES (?,?,?,?,?)",
            )
            .bind(id)
            .bind(id)
            .bind(enabled)
            .bind(now)
            .bind(now)
            .execute(&st.pool)
            .await
            .unwrap();
            // both left with a stale 'recording' status row
            crate::repo::set_state(&st.pool, id, "recording", None)
                .await
                .unwrap();
        }

        let Json(rows) = list_status(State(st.clone()), Principal::system_admin())
            .await
            .unwrap();
        let by: std::collections::HashMap<String, String> =
            rows.into_iter().map(|r| (r.camera_id, r.state)).collect();
        assert_eq!(
            by["cam_on"], "recording",
            "enabled camera keeps its recorder state"
        );
        assert_eq!(
            by["cam_off"], "disabled",
            "disabled camera overrides stale 'recording'"
        );

        // the single-camera endpoint applies the same rule
        let Json(one) = camera_status(
            State(st.clone()),
            Principal::system_admin(),
            Path("cam_off".into()),
        )
        .await
        .unwrap();
        assert_eq!(one.state, "disabled");
    }
}
