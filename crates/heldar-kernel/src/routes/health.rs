use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::Principal;
use crate::error::{AppError, AppResult};
use crate::models::{CameraStatus, Event};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/health/cameras", get(list_status))
        .route("/api/v1/cameras/{id}/health", get(camera_status))
        .route("/api/v1/events", get(list_events))
}

async fn list_status(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<CameraStatus>>> {
    principal.require(principal.can_view(), "view camera health")?;
    let mut rows =
        sqlx::query_as::<_, CameraStatus>("SELECT * FROM camera_status ORDER BY camera_id ASC")
            .fetch_all(&st.pool)
            .await?;
    // A disabled camera's recorder state is irrelevant and can be left stale by the async recorder
    // teardown (e.g. "recording"/"error" right after a disable); report it as "disabled" so the
    // health table is truthful.
    let disabled: std::collections::HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT id FROM cameras WHERE enabled = 0")
            .fetch_all(&st.pool)
            .await?
            .into_iter()
            .collect();
    for r in &mut rows {
        if disabled.contains(&r.camera_id) {
            r.state = "disabled".into();
        }
    }
    Ok(Json(rows))
}

async fn camera_status(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<CameraStatus>> {
    principal.require(principal.can_view(), "view camera health")?;
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
struct EventQuery {
    camera_id: Option<String>,
    event_type: Option<String>,
    severity: Option<String>,
    limit: Option<i64>,
}

async fn list_events(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<EventQuery>,
) -> AppResult<Json<Vec<Event>>> {
    principal.require(principal.can_view(), "view events")?;
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
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
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
