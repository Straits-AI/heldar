//! Retention sweeper: deletes recordings past each camera's age policy, and enforces a global
//! size cap by pruning the oldest unlocked segments. Locked (evidence) segments are never deleted.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::repo;

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.retention_interval_s.max(30)));
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&pool, &cfg).await {
            tracing::error!(error = %e, "retention: sweep failed");
        }
    }
}

async fn sweep(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<()> {
    // 1) Age-based retention, per-camera.
    let mut age_deleted: u64 = 0;
    let cams: Vec<(String, i64)> = sqlx::query_as("SELECT id, retention_hours FROM cameras")
        .fetch_all(pool)
        .await?;
    for (id, hours) in cams {
        let cutoff = Utc::now() - chrono::Duration::hours(hours.max(1));
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, path FROM segments WHERE camera_id = ? AND locked = 0 AND end_time < ?",
        )
        .bind(&id)
        .bind(cutoff)
        .fetch_all(pool)
        .await?;
        for (seg_id, path) in rows {
            let _ = tokio::fs::remove_file(&path).await;
            sqlx::query("DELETE FROM segments WHERE id = ?")
                .bind(&seg_id)
                .execute(pool)
                .await?;
            age_deleted += 1;
        }
    }
    if age_deleted > 0 {
        let _ = repo::log_event(
            pool,
            None,
            "retention_delete",
            "info",
            json!({ "deleted": age_deleted, "reason": "age" }),
        )
        .await;
        tracing::info!(deleted = age_deleted, "retention: age-based cleanup");
    }

    // 2) Global size cap: prune the oldest UNLOCKED segments until the deletable footprint fits the
    //    budget. The budget is the cap minus the locked (evidence) bytes we cannot delete — counting
    //    locked bytes in the comparison would otherwise make us delete every unlocked segment.
    let max = cfg.max_recordings_bytes as i64;
    let locked_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE locked = 1")
            .fetch_one(pool)
            .await?;
    let budget = max - locked_bytes;
    let mut size_deleted: u64 = 0;

    if budget <= 0 {
        // Locked/evidence footage alone meets or exceeds the cap; deleting unlocked footage cannot
        // help. Warn instead of wiping everything.
        let unlocked: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE locked = 0",
        )
        .fetch_one(pool)
        .await?;
        if locked_bytes > max {
            tracing::warn!(
                locked_bytes,
                max,
                "retention: locked (evidence) footage exceeds the size cap; not deleting unlocked footage"
            );
            let _ = repo::log_event(
                pool,
                None,
                "disk_pressure",
                "warning",
                json!({ "reason": "locked_exceeds_cap", "locked_bytes": locked_bytes, "unlocked_bytes": unlocked, "max_bytes": max }),
            )
            .await;
        }
    } else {
        loop {
            let unlocked_total: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE locked = 0",
            )
            .fetch_one(pool)
            .await?;
            if unlocked_total <= budget {
                break;
            }
            let batch: Vec<(String, String)> = sqlx::query_as(
                "SELECT id, path FROM segments WHERE locked = 0 ORDER BY end_time ASC LIMIT 20",
            )
            .fetch_all(pool)
            .await?;
            if batch.is_empty() {
                break;
            }
            for (seg_id, path) in batch {
                let _ = tokio::fs::remove_file(&path).await;
                sqlx::query("DELETE FROM segments WHERE id = ?")
                    .bind(&seg_id)
                    .execute(pool)
                    .await?;
                size_deleted += 1;
            }
        }
    }

    if size_deleted > 0 {
        let _ = repo::log_event(
            pool,
            None,
            "disk_pressure",
            "warning",
            json!({ "deleted": size_deleted, "reason": "size_cap", "max_bytes": max }),
        )
        .await;
        tracing::warn!(deleted = size_deleted, "retention: size-cap cleanup");
    }
    Ok(())
}
