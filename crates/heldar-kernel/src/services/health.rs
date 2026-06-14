//! Health monitor: downgrades cameras that claim to be recording but have stopped producing
//! segments (a stalled-but-connected stream), emitting an event on the transition.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::repo;

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.health_interval_s.max(5)));
    loop {
        tick.tick().await;
        if let Err(e) = check_once(&pool).await {
            tracing::error!(error = %e, "health: check failed");
        }
    }
}

/// (camera_id, last_segment_at, last_started_at, segment_seconds)
type StaleRow = (String, Option<DateTime<Utc>>, Option<DateTime<Utc>>, i64);

async fn check_once(pool: &SqlitePool) -> anyhow::Result<()> {
    let rows: Vec<StaleRow> = sqlx::query_as(
        "SELECT cs.camera_id, cs.last_segment_at, cs.last_started_at, c.segment_seconds
         FROM camera_status cs
         JOIN cameras c ON c.id = cs.camera_id
         WHERE cs.state = 'recording'",
    )
    .fetch_all(pool)
    .await?;

    let now = Utc::now();
    for (camera_id, last_seg, last_start, seg_s) in rows {
        let threshold = (seg_s.max(10) * 3).max(30);
        let seg_age = last_seg.map(|t| (now - t).num_seconds());
        let start_age = last_start.map(|t| (now - t).num_seconds());

        let recent_segment = seg_age.map(|a| a <= threshold).unwrap_or(false);
        let recently_started = start_age.map(|a| a <= threshold).unwrap_or(false);
        if recent_segment || recently_started {
            continue;
        }

        let msg = format!("no segments for >{threshold}s while recording");
        let _ = repo::set_state(pool, &camera_id, "error", Some(&msg)).await;
        let _ = repo::log_event(
            pool,
            Some(&camera_id),
            "recorder_error",
            "warning",
            json!({ "reason": "stale", "threshold_seconds": threshold, "last_segment_age_s": seg_age }),
        )
        .await;
        tracing::warn!(%camera_id, threshold, "health: camera stale, marked error");
    }
    Ok(())
}
