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

/// Readiness: the database is reachable (returns 503 otherwise). When
/// `HELDAR_READYZ_MIN_RECORDING_PERCENT > 0` this also acts as an HA recorder-quorum probe (see
/// docs/HA.md): a node whose recording coverage drops below the threshold reports 503 so a
/// keepalived `health_script` can fail it over to a hot spare. Default 0 keeps DB-only behaviour.
async fn readyz(State(st): State<AppState>) -> Response {
    if let Err(e) = sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&st.pool)
        .await
    {
        tracing::error!(error = %e, "readyz: database not reachable");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ready": false, "reason": "database" })),
        )
            .into_response();
    }

    let required = st.cfg.readyz_min_recording_percent;
    if required > 0.0 {
        let counts = async {
            let enabled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE enabled = 1")
                .fetch_one(&st.pool)
                .await?;
            let recording: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM camera_status WHERE state = 'recording'")
                    .fetch_one(&st.pool)
                    .await?;
            Ok::<_, sqlx::Error>((enabled, recording))
        }
        .await;
        let (enabled, recording) = match counts {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "readyz: recorder-quorum query failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "ready": false, "reason": "database" })),
                )
                    .into_response();
            }
        };
        // No enabled cameras => nothing to record => the node is ready by definition.
        let pct = if enabled > 0 {
            (recording as f64) * 100.0 / (enabled as f64)
        } else {
            100.0
        };
        let pct = (pct * 10.0).round() / 10.0;
        if pct < required {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "ready": false,
                    "reason": "insufficient_recorders",
                    "recording_pct": pct,
                    "required_pct": required,
                })),
            )
                .into_response();
        }
    }

    (StatusCode::OK, Json(json!({ "ready": true }))).into_response()
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
    /// No recent disk_smart_warning/raid_degraded events (see services::health disk-health pass).
    disk_health_ok: bool,
    /// Timestamp of the most recent disk-health alert (any time), or null if none ever fired.
    last_disk_alert_at: Option<DateTime<Utc>>,
    /// Active live-preview transcode engine (software | vaapi | nvenc).
    live_transcode_engine: String,
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

    // Disk health: the latest disk-health alert (any time) and whether one fired recently (within a
    // few SMART-check cycles). With checks disabled no such events exist, so health reads as OK.
    let last_disk_alert_raw: Option<String> = sqlx::query_scalar(
        "SELECT MAX(timestamp) FROM events WHERE event_type IN ('disk_smart_warning', 'raid_degraded')",
    )
    .fetch_one(&st.pool)
    .await?;
    let last_disk_alert_at = last_disk_alert_raw
        .as_deref()
        .and_then(crate::util::parse_rfc3339);
    let recent_window_s = (st.cfg.smart_check_interval_s.saturating_mul(3)).max(900) as i64;
    let cutoff = Utc::now() - chrono::Duration::seconds(recent_window_s);
    let recent_disk_alerts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events
          WHERE event_type IN ('disk_smart_warning', 'raid_degraded') AND timestamp >= ?",
    )
    .bind(cutoff)
    .fetch_one(&st.pool)
    .await?;

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
        disk_health_ok: recent_disk_alerts == 0,
        last_disk_alert_at,
        live_transcode_engine: st.cfg.live_transcode_engine.clone(),
    }))
}
