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
        .foreign_keys(true)
        .pragma("auto_vacuum", "incremental");

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

    /// init_pool must NOT convert a pre-existing DB's auto_vacuum mode (the conversion moved to a
    /// background task). A mode-0 file DB stays mode 0 after init_pool — guards against re-adding the
    /// boot-blocking VACUUM.
    #[tokio::test]
    async fn init_pool_does_not_convert_autovacuum() {
        let dir =
            std::env::temp_dir().join(format!("heldar-initpool-noconv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("t.db");
        // Create a mode-0 DB WITHOUT the incremental connect pragma, with a table so it is non-trivial.
        {
            let url = format!("sqlite://{}?mode=rwc", db_path.display());
            let seed = SqlitePoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .unwrap();
            sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY)")
                .execute(&seed)
                .await
                .unwrap();
            let m: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
                .fetch_one(&seed)
                .await
                .unwrap();
            assert_ne!(m, 2, "seed DB is non-incremental");
            seed.close().await;
        }
        let mut cfg = Config::from_env();
        cfg.database_url = format!("sqlite://{}", db_path.display());
        // Point data_dir at a directory that actually exists so the disk-space gate inside
        // ensure_incremental_autovacuum (statvfs on cfg.data_dir) can resolve instead of silently
        // skipping the conversion for an unrelated reason (default `./data` doesn't exist under the
        // test cwd) — otherwise this test would pass "by accident" without exercising init_pool.
        cfg.data_dir = dir.clone();
        // Pin the pool to a single connection so the read-back below deterministically lands on the
        // same physical connection init_pool used — otherwise which connection serves the final
        // PRAGMA is pool-implementation-dependent and the assertion would be flaky either way.
        cfg.db_max_connections = 1;
        let pool = init_pool(&cfg).await.unwrap();
        let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode, 0, "init_pool performed no conversion VACUUM");
        drop(pool);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
