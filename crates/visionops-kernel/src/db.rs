use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::config::Config;

/// Open the SQLite pool with WAL + sane concurrency settings, creating the file if needed.
pub async fn init_pool(cfg: &Config) -> anyhow::Result<SqlitePool> {
    if !cfg.database_url.starts_with("sqlite") {
        anyhow::bail!(
            "Stage 0 supports sqlite only; got `{}`. Postgres is planned via SQLx.",
            cfg.database_url
        );
    }

    let opts = SqliteConnectOptions::from_str(&cfg.database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(15))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(cfg.db_max_connections)
        .acquire_timeout(Duration::from_secs(20))
        .connect_with(opts)
        .await?;

    Ok(pool)
}

/// Apply embedded migrations from `./migrations`.
pub async fn run_migrations(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// Clear any transient segment read-locks left over from a crash. clip/snapshot export set
/// `segments.locked = 1` while ffmpeg reads a segment and release it afterwards; if the process died
/// mid-read those segments would stay locked (and never be pruned by retention). Clearing at startup
/// makes the read-lock crash-safe. NOTE: this means `locked` is reserved for transient read-locks —
/// a future durable evidence-hold must use a separate column, not this one.
pub async fn clear_segment_read_locks(pool: &SqlitePool) -> anyhow::Result<()> {
    let n = sqlx::query("UPDATE segments SET locked = 0 WHERE locked <> 0")
        .execute(pool)
        .await?
        .rows_affected();
    if n > 0 {
        tracing::info!(cleared = n, "startup: cleared stale segment read-locks");
    }
    Ok(())
}
