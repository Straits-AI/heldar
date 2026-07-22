//! Forensic-search owns its query-log lifecycle: `search_log` (one row per NL/structured/plan query)
//! is otherwise append-only, so this prunes rows past `query_log_retention_days`. The kernel's
//! retention sweeper handles only kernel-owned tables; each app prunes its own (mirrors heldar-entry).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use heldar_kernel::config::Config;
use sqlx::SqlitePool;

use crate::config::SearchConfig;

pub async fn run(pool: SqlitePool, cfg: Arc<Config>, scfg: Arc<SearchConfig>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.retention_interval_s.max(30)));
    loop {
        tick.tick().await;
        let cutoff = Utc::now() - chrono::Duration::days(scfg.query_log_retention_days.max(1));
        match sqlx::query("DELETE FROM search_log WHERE created_at < ?")
            .bind(cutoff)
            .execute(&pool)
            .await
        {
            Ok(r) if r.rows_affected() > 0 => {
                tracing::info!(
                    deleted = r.rows_affected(),
                    "search retention: pruned old query-log rows"
                )
            }
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "search retention: prune failed"),
        }
    }
}
