//! Campus Entry owns the lifecycle of its own data: it prunes old entry events (and deletes their
//! evidence frames) on the entry-retention TTL. The kernel's retention sweeper handles only
//! kernel-owned data (segments, detections, outbox, zone events, sessions, audit, events).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use heldar_kernel::config::Config;
use sqlx::SqlitePool;

use crate::config::EntryConfig;

pub async fn run(pool: SqlitePool, cfg: Arc<Config>, ecfg: Arc<EntryConfig>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.retention_interval_s.max(30)));
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&pool, &cfg, &ecfg).await {
            tracing::error!(error = %e, "entry retention: sweep failed");
        }
    }
}

async fn sweep(pool: &SqlitePool, cfg: &Config, ecfg: &EntryConfig) -> anyhow::Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(ecfg.entry_retention_days.max(1));
    let old: Vec<(String, sqlx::types::Json<serde_json::Value>)> =
        sqlx::query_as("SELECT id, evidence FROM entry_events WHERE created_at < ?")
            .bind(cutoff)
            .fetch_all(pool)
            .await?;
    if old.is_empty() {
        return Ok(());
    }
    for (_id, evidence) in &old {
        if let Some(name) = evidence
            .0
            .get("snapshot_path")
            .and_then(|v| v.as_str())
            .and_then(|u| u.rsplit('/').next())
        {
            let _ = tokio::fs::remove_file(cfg.snapshots_dir.join(name)).await;
        }
    }
    let n = sqlx::query("DELETE FROM entry_events WHERE created_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    tracing::info!(
        deleted = n,
        "entry retention: pruned old entry events + evidence"
    );
    Ok(())
}
