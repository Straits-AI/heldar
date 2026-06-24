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

#[cfg(test)]
mod tests {
    use super::*;

    /// First-principles concurrency invariant: under heavy concurrent writers on the real production
    /// pool config (WAL + busy_timeout), a normal write must WAIT (serialize) rather than surface
    /// SQLITE_BUSY as an error. If this ever fails, the busy_timeout is too low (and the 503 mapping
    /// in error.rs is the user-facing safety net).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writers_serialize_without_busy_errors() {
        let dir = std::env::temp_dir().join(format!("heldar-walstress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::from_env();
        cfg.database_url = format!("sqlite://{}", dir.join("t.db").display());
        cfg.db_max_connections = 8;
        let pool = init_pool(&cfg).await.unwrap();
        run_migrations(&pool).await.unwrap();

        // 64 concurrent writers contend for the single WAL writer slot.
        let mut handles = Vec::new();
        for i in 0..64 {
            let p = pool.clone();
            handles.push(tokio::spawn(async move {
                let now = chrono::Utc::now();
                sqlx::query(
                    "INSERT INTO cameras (id, name, retention_hours, storage_quota_bytes, created_at, updated_at)
                     VALUES (?, ?, 168, NULL, ?, ?)",
                )
                .bind(format!("cam{i}"))
                .bind(format!("cam{i}"))
                .bind(now)
                .bind(now)
                .execute(&p)
                .await
            }));
        }
        let mut errors = 0usize;
        for h in handles {
            if h.await.unwrap().is_err() {
                errors += 1;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            errors, 0,
            "concurrent writers must not surface SQLITE_BUSY under WAL + busy_timeout ({errors} failed)"
        );
    }
}
