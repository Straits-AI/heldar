use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::AppResult;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/system", get(system_info))
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
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

    Ok(Json(SystemInfo {
        name: "VisionOps Core",
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
    }))
}
