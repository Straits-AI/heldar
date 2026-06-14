use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::AppResult;
use crate::services::remote_access::{self, OverlayStatus};
use crate::services::storage::{self, StorageReport};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/v1/system", get(system_info))
}

/// Liveness: the process is up.
async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Readiness: the database is reachable (returns 503 otherwise).
async fn readyz(State(st): State<AppState>) -> Response {
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&st.pool)
        .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({ "ready": true }))).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "readyz: database not reachable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "ready": false, "reason": "database" })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Serialize)]
struct SystemInfo {
    name: &'static str,
    version: &'static str,
    started_at: DateTime<Utc>,
    uptime_seconds: i64,
    recorder_enabled: bool,
    cameras_total: i64,
    cameras_recording: i64,
    active_recorders: usize,
    segments_total: i64,
    recordings_bytes: i64,
    recordings_gb: f64,
    max_recordings_gb: f64,
    storage: StorageReport,
    remote_access: OverlayStatus,
}

async fn system_info(State(st): State<AppState>) -> AppResult<Json<SystemInfo>> {
    let cameras_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras")
        .fetch_one(&st.pool)
        .await?;
    let cameras_recording: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM camera_status WHERE state = 'recording'")
            .fetch_one(&st.pool)
            .await?;
    let segments_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM segments")
        .fetch_one(&st.pool)
        .await?;
    let recordings_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM segments")
            .fetch_one(&st.pool)
            .await?;
    let active_recorders = st.recorder.active_ids().await.len();
    let storage = storage::storage_report(&st.pool, &st.cfg).await?;

    Ok(Json(SystemInfo {
        name: "Heldar Core",
        version: env!("CARGO_PKG_VERSION"),
        started_at: st.started_at,
        uptime_seconds: (Utc::now() - st.started_at).num_seconds(),
        recorder_enabled: st.cfg.recorder_enabled,
        cameras_total,
        cameras_recording,
        active_recorders,
        segments_total,
        recordings_bytes,
        recordings_gb: recordings_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
        max_recordings_gb: st.cfg.max_recordings_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
        storage,
        remote_access: remote_access::status(&st.cfg),
    }))
}
