//! Prometheus exposition rendering for the `/metrics` endpoint (system + per-camera gauges).

use std::fmt::Write;

use chrono::Utc;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::models::CameraStatus;
use crate::services::storage;

/// Escape a Prometheus label value.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

fn metric(out: &mut String, name: &str, help: &str, kind: &str, value: f64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    let _ = writeln!(out, "{name} {value}");
}

/// Render the full metrics page in Prometheus text exposition format.
pub async fn render(pool: &SqlitePool, cfg: &Config) -> sqlx::Result<String> {
    let mut out = String::new();

    let _ = writeln!(out, "# HELP heldar_build_info Build information");
    let _ = writeln!(out, "# TYPE heldar_build_info gauge");
    let _ = writeln!(
        out,
        "heldar_build_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    );

    let cameras_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras")
        .fetch_one(pool)
        .await?;
    let recording: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM camera_status WHERE state = 'recording'")
            .fetch_one(pool)
            .await?;
    let segments_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM segments")
        .fetch_one(pool)
        .await?;
    let recordings_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM segments")
            .fetch_one(pool)
            .await?;

    metric(
        &mut out,
        "heldar_cameras_total",
        "Registered cameras",
        "gauge",
        cameras_total as f64,
    );
    metric(
        &mut out,
        "heldar_cameras_recording",
        "Cameras currently recording",
        "gauge",
        recording as f64,
    );
    metric(
        &mut out,
        "heldar_segments_total",
        "Indexed recording segments",
        "gauge",
        segments_total as f64,
    );
    metric(
        &mut out,
        "heldar_recordings_bytes",
        "Total bytes of recorded segments",
        "gauge",
        recordings_bytes as f64,
    );

    let ai_tasks_enabled: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ai_tasks WHERE enabled = 1")
            .fetch_one(pool)
            .await?;
    let detections_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM detections")
        .fetch_one(pool)
        .await?;
    metric(
        &mut out,
        "heldar_ai_tasks_enabled",
        "Enabled AI tasks",
        "gauge",
        ai_tasks_enabled as f64,
    );
    // gauge (not counter): the stored count can decrease as the retention sweeper prunes old rows.
    metric(
        &mut out,
        "heldar_detections_stored",
        "AI detections currently stored",
        "gauge",
        detections_total as f64,
    );

    if let Some(d) = storage::disk_stats_async(cfg.recordings_dir.clone()).await {
        metric(
            &mut out,
            "heldar_disk_total_bytes",
            "Total bytes on recordings filesystem",
            "gauge",
            d.total_bytes as f64,
        );
        metric(
            &mut out,
            "heldar_disk_free_bytes",
            "Free bytes on recordings filesystem",
            "gauge",
            d.free_bytes as f64,
        );
        metric(
            &mut out,
            "heldar_disk_used_percent",
            "Used percent of recordings filesystem",
            "gauge",
            d.used_percent,
        );
    }

    let rows = sqlx::query_as::<_, CameraStatus>("SELECT * FROM camera_status")
        .fetch_all(pool)
        .await?;
    let now = Utc::now();

    let _ = writeln!(
        out,
        "# HELP heldar_camera_up Camera recording state (1 = recording)"
    );
    let _ = writeln!(out, "# TYPE heldar_camera_up gauge");
    for r in &rows {
        let up = i32::from(r.state == "recording");
        let _ = writeln!(
            out,
            "heldar_camera_up{{camera=\"{}\",state=\"{}\"}} {up}",
            esc(&r.camera_id),
            esc(&r.state)
        );
    }

    let _ = writeln!(
        out,
        "# HELP heldar_camera_reconnects_total Recorder reconnect count"
    );
    let _ = writeln!(out, "# TYPE heldar_camera_reconnects_total counter");
    for r in &rows {
        let _ = writeln!(
            out,
            "heldar_camera_reconnects_total{{camera=\"{}\"}} {}",
            esc(&r.camera_id),
            r.reconnect_count
        );
    }

    let _ = writeln!(
        out,
        "# HELP heldar_camera_segments_written_total Segments written by the recorder"
    );
    let _ = writeln!(out, "# TYPE heldar_camera_segments_written_total counter");
    for r in &rows {
        let _ = writeln!(
            out,
            "heldar_camera_segments_written_total{{camera=\"{}\"}} {}",
            esc(&r.camera_id),
            r.segments_written
        );
    }

    let _ = writeln!(
        out,
        "# HELP heldar_camera_bitrate_kbps Observed stream bitrate (kbps)"
    );
    let _ = writeln!(out, "# TYPE heldar_camera_bitrate_kbps gauge");
    for r in &rows {
        if let Some(b) = r.bitrate_kbps {
            let _ = writeln!(
                out,
                "heldar_camera_bitrate_kbps{{camera=\"{}\"}} {b}",
                esc(&r.camera_id)
            );
        }
    }

    let _ = writeln!(
        out,
        "# HELP heldar_camera_last_segment_age_seconds Seconds since the last indexed segment"
    );
    let _ = writeln!(out, "# TYPE heldar_camera_last_segment_age_seconds gauge");
    for r in &rows {
        if let Some(t) = r.last_segment_at {
            let age = (now - t).num_seconds().max(0);
            let _ = writeln!(
                out,
                "heldar_camera_last_segment_age_seconds{{camera=\"{}\"}} {age}",
                esc(&r.camera_id)
            );
        }
    }

    Ok(out)
}
