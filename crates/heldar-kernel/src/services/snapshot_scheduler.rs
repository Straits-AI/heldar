//! Scheduled interval snapshots. On each tick the scheduler fires any due `snapshot_schedule`
//! (enabled, and either never fired or its interval has elapsed since `last_fired_at`): it grabs a
//! live JPEG via the snapshot service, writes it to `snapshots_dir/{camera_id}/{taken_at}.jpg`,
//! records a `snapshots` row, and advances `last_fired_at`. Captured frames are pruned by the
//! retention sweeper past HELDAR_SNAPSHOT_RETENTION_HOURS. Spawned from `main` (supervised) only
//! when HELDAR_SNAPSHOT_SCHEDULER_ENABLED.

use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::services::snapshot;
use crate::state::AppState;

pub async fn run(state: AppState) {
    let interval_s = state.cfg.snapshot_scheduler_interval_s.max(5);
    let mut tick = tokio::time::interval(Duration::from_secs(interval_s));
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&state).await {
            tracing::error!(error = %e, "snapshot_scheduler: tick failed");
        }
    }
}

/// Fire every due schedule once. Due-ness is computed in Rust (robust against SQLite text-time
/// quirks): a schedule is due when it has never fired, or `last_fired_at + interval_seconds <= now`.
async fn sweep(state: &AppState) -> anyhow::Result<()> {
    let now = Utc::now();
    let schedules: Vec<crate::models::SnapshotSchedule> =
        sqlx::query_as::<_, crate::models::SnapshotSchedule>(
            "SELECT * FROM snapshot_schedules WHERE enabled = 1",
        )
        .fetch_all(&state.pool)
        .await?;

    for sched in schedules {
        let interval = sched.interval_seconds.max(1);
        let due = match sched.last_fired_at {
            None => true,
            Some(last) => last + chrono::Duration::seconds(interval) <= now,
        };
        if !due {
            continue;
        }
        // One camera failing to capture must not stop the others; log and move on.
        if let Err(e) = fire(state, &sched).await {
            tracing::warn!(
                schedule = %sched.id,
                camera = %sched.camera_id,
                error = %e,
                "snapshot_scheduler: capture failed"
            );
        }
    }
    Ok(())
}

/// Capture one frame for a schedule, persist the file + row, and stamp `last_fired_at`.
async fn fire(state: &AppState, sched: &crate::models::SnapshotSchedule) -> anyhow::Result<()> {
    let taken_at: DateTime<Utc> = Utc::now();
    let bytes = snapshot::snapshot_live_raw(state, &sched.camera_id).await?;
    let size_bytes = bytes.len() as i64;

    let dir = state.cfg.snapshots_dir.join(&sched.camera_id);
    tokio::fs::create_dir_all(&dir).await?;
    // Compact, sortable, URL-safe filename derived from the capture time (no colons/offset chars).
    let fname = format!("{}.jpg", taken_at.format("%Y%m%dT%H%M%S%3fZ"));
    let path = dir.join(&fname);
    tokio::fs::write(&path, &bytes).await?;
    let path_str = path.to_string_lossy().to_string();

    let id = format!("snap_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO snapshots (id, camera_id, schedule_id, path, taken_at, size_bytes, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&sched.camera_id)
    .bind(&sched.id)
    .bind(&path_str)
    .bind(taken_at)
    .bind(size_bytes)
    .bind(now)
    .execute(&state.pool)
    .await?;

    sqlx::query("UPDATE snapshot_schedules SET last_fired_at = ?, updated_at = ? WHERE id = ?")
        .bind(taken_at)
        .bind(now)
        .bind(&sched.id)
        .execute(&state.pool)
        .await?;

    tracing::debug!(camera = %sched.camera_id, path = %path_str, "snapshot_scheduler: captured snapshot");
    Ok(())
}
