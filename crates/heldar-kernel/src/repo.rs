//! Small shared data-access helpers used by background services and routes.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Upsert the camera status row, setting `state` and `last_error` (does not touch counters).
pub async fn set_state(
    pool: &SqlitePool,
    camera_id: &str,
    state: &str,
    last_error: Option<&str>,
) -> sqlx::Result<()> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_status (camera_id, state, last_error, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            state = excluded.state,
            last_error = excluded.last_error,
            updated_at = excluded.updated_at",
    )
    .bind(camera_id)
    .bind(state)
    .bind(last_error)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark the recorder process started: set state, pid, and last_started_at.
pub async fn set_running(
    pool: &SqlitePool,
    camera_id: &str,
    state: &str,
    pid: Option<i64>,
) -> sqlx::Result<()> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_status (camera_id, state, recorder_pid, last_started_at, last_error, updated_at)
         VALUES (?, ?, ?, ?, NULL, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            state = excluded.state,
            recorder_pid = excluded.recorder_pid,
            last_started_at = excluded.last_started_at,
            last_error = NULL,
            updated_at = excluded.updated_at",
    )
    .bind(camera_id)
    .bind(state)
    .bind(pid)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Increment the reconnect counter, clear pid, and record the last error.
pub async fn bump_reconnect(
    pool: &SqlitePool,
    camera_id: &str,
    last_error: &str,
) -> sqlx::Result<()> {
    let now = Utc::now();
    let err = last_error.chars().rev().take(800).collect::<String>();
    let err: String = err.chars().rev().collect();
    sqlx::query(
        "INSERT INTO camera_status (camera_id, state, reconnect_count, last_error, recorder_pid, updated_at)
         VALUES (?, 'offline', 1, ?, NULL, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            state = 'offline',
            reconnect_count = camera_status.reconnect_count + 1,
            last_error = excluded.last_error,
            recorder_pid = NULL,
            updated_at = excluded.updated_at",
    )
    .bind(camera_id)
    .bind(err)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that a new segment was indexed: bump count, set last_segment_at and observed bitrate.
pub async fn record_segment_indexed(
    pool: &SqlitePool,
    camera_id: &str,
    last_segment_at: DateTime<Utc>,
    bitrate_kbps: Option<f64>,
    fps_observed: Option<f64>,
) -> sqlx::Result<()> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_status
           (camera_id, state, last_segment_at, segments_written, bitrate_kbps, fps_observed, updated_at)
         VALUES (?, 'recording', ?, 1, ?, ?, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            state = 'recording',
            last_segment_at = excluded.last_segment_at,
            segments_written = camera_status.segments_written + 1,
            bitrate_kbps = excluded.bitrate_kbps,
            fps_observed = excluded.fps_observed,
            last_error = NULL,
            updated_at = excluded.updated_at",
    )
    .bind(camera_id)
    .bind(last_segment_at)
    .bind(bitrate_kbps)
    .bind(fps_observed)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert an event into the event log.
pub async fn log_event(
    pool: &SqlitePool,
    camera_id: Option<&str>,
    event_type: &str,
    severity: &str,
    payload: Value,
) -> sqlx::Result<()> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO events (id, camera_id, site_id, event_type, severity, timestamp, payload, created_at)
         VALUES (?, ?, NULL, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(camera_id)
    .bind(event_type)
    .bind(severity)
    .bind(now)
    .bind(Json(payload))
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Toggle a transient read-lock on a set of segments so the retention sweeper (which only deletes
/// `locked = 0`) won't remove them while clip/snapshot ffmpeg is reading them — closing the TOCTOU
/// between selecting segments and ffmpeg opening their files. Best-effort: a failure is logged, not
/// fatal (the read still proceeds). Locks are cleared at startup ([`crate::db::clear_segment_read_locks`])
/// so a crash mid-read cannot pin segments forever.
pub async fn set_segments_locked(pool: &SqlitePool, ids: &[String], locked: bool) {
    if ids.is_empty() {
        return;
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!("UPDATE segments SET locked = ? WHERE id IN ({placeholders})");
    let mut q = sqlx::query(&sql).bind(i64::from(locked));
    for id in ids {
        q = q.bind(id);
    }
    if let Err(e) = q.execute(pool).await {
        tracing::warn!(error = %e, locked, "failed to toggle segment read-lock");
    }
}
