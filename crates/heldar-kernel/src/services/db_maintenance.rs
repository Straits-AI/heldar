//! Metadata-DB size maintenance: measure, checkpoint the WAL, incrementally reclaim freed pages,
//! and enforce a hard file-size cap by shedding the oldest `detections` (events/audit are protected).
//! Called from the retention sweep after the row-retention deletes have freed pages.

use anyhow::Context;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::services::settings;

/// Current DB file size in bytes = `page_count * page_size` (main DB; WAL is folded in by
/// `checkpoint_wal` before measuring).
pub async fn db_size_bytes(pool: &SqlitePool) -> anyhow::Result<u64> {
    let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(pool)
        .await
        .context("PRAGMA page_count")?;
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(pool)
        .await
        .context("PRAGMA page_size")?;
    Ok((page_count.max(0) as u64) * (page_size.max(0) as u64))
}

/// Fold the WAL back into the main DB and truncate the `-wal` file, bounding its on-disk size.
pub async fn checkpoint_wal(pool: &SqlitePool) -> anyhow::Result<()> {
    // Returns a row (busy, log, checkpointed); we don't need it, just run it.
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(pool)
        .await
        .context("PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(())
}

/// Return up to `max_pages` freed pages to the OS. No-op unless the DB is in
/// `auto_vacuum=INCREMENTAL` mode (see `ensure_incremental_autovacuum`).
pub async fn incremental_vacuum(pool: &SqlitePool, max_pages: u32) -> anyhow::Result<()> {
    sqlx::query(&format!("PRAGMA incremental_vacuum({max_pages})"))
        .execute(pool)
        .await
        .context("PRAGMA incremental_vacuum")?;
    Ok(())
}

/// Delete the oldest `batch` detection rows (by `created_at`). Returns rows deleted.
pub async fn prune_oldest_detections(pool: &SqlitePool, batch: u32) -> anyhow::Result<u64> {
    let n = sqlx::query(
        "DELETE FROM detections WHERE id IN \
         (SELECT id FROM detections ORDER BY created_at ASC LIMIT ?)",
    )
    .bind(batch)
    .execute(pool)
    .await
    .context("prune oldest detections")?
    .rows_affected();
    Ok(n)
}

/// Outcome of one `enforce_db_cap` pass.
#[derive(Debug, Default, Clone, Copy)]
pub struct DbCapReport {
    pub detections_deleted: u64,
    pub final_bytes: u64,
    /// True if the DB is still over cap after we stopped (only protected data left, or no reclaim).
    pub over_cap: bool,
}

/// How many pages to try to reclaim per incremental_vacuum call — bounded so the write-lock hold stays short.
const VACUUM_PAGES: u32 = 20_000;
/// How many detection rows to delete per prune batch.
const PRUNE_BATCH: u32 = 5_000;

/// Enforce the DB size cap: checkpoint the WAL, reclaim freed pages, then while the DB file is over
/// the cap delete the oldest detections and reclaim — stopping if no deletable detections remain OR
/// if a prune+vacuum makes no progress on file size (the no-runaway guard). Cap disabled (<=0) ⇒ no-op.
pub async fn enforce_db_cap(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<DbCapReport> {
    let max = settings::get_i64(pool, settings::DB_MAX_BYTES)
        .await
        .filter(|&v| v > 0)
        .unwrap_or(cfg.max_db_bytes as i64);
    if max <= 0 {
        return Ok(DbCapReport::default()); // cap disabled
    }
    let max = max as u64;

    checkpoint_wal(pool).await?;
    incremental_vacuum(pool, VACUUM_PAGES).await?;
    let mut size = db_size_bytes(pool).await?;
    let mut deleted: u64 = 0;

    while size > max {
        let n = prune_oldest_detections(pool, PRUNE_BATCH).await?;
        if n == 0 {
            tracing::warn!(
                bytes = size,
                cap = max,
                "db retention: over cap but no deletable detections remain (protected data only)"
            );
            return Ok(DbCapReport {
                detections_deleted: deleted,
                final_bytes: size,
                over_cap: true,
            });
        }
        incremental_vacuum(pool, VACUUM_PAGES).await?;
        let new_size = db_size_bytes(pool).await?;
        deleted += n;
        if new_size >= size {
            // No progress — reclaim can't shrink the file yet (e.g. auto_vacuum not converted).
            tracing::warn!(
                bytes = new_size,
                cap = max,
                "db retention: prune made no progress on file size; stopping (reclaim inactive?)"
            );
            return Ok(DbCapReport {
                detections_deleted: deleted,
                final_bytes: new_size,
                over_cap: true,
            });
        }
        size = new_size;
    }
    Ok(DbCapReport {
        detections_deleted: deleted,
        final_bytes: size,
        over_cap: false,
    })
}

/// Ensure the DB uses `auto_vacuum=INCREMENTAL` so `incremental_vacuum` can reclaim pages.
/// New DBs are born INCREMENTAL (via the connect pragma in `db::init_pool`); an existing DB in
/// mode NONE(0)/FULL(1) is converted with a one-time `VACUUM`. The VACUUM needs temp space ≈ the DB
/// size, so it is SKIPPED (and retried next boot) when free disk < db_size × 1.1. Returns whether a
/// conversion VACUUM ran.
pub async fn ensure_incremental_autovacuum(
    pool: &SqlitePool,
    cfg: &Config,
) -> anyhow::Result<bool> {
    // Current mode is a DB-header property; any connection reports the same value.
    let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
        .fetch_one(pool)
        .await
        .context("PRAGMA auto_vacuum")?;
    if mode == 2 {
        return Ok(false); // already INCREMENTAL
    }
    // A full VACUUM rewrites the DB and needs ~size of scratch space. Measure + disk-gate via the
    // POOL *before* pinning a connection — holding the pinned connection while calling a pool-based
    // helper would self-deadlock a max_connections(1) pool.
    let size = db_size_bytes(pool).await?;
    match crate::services::storage::disk_stats_async(cfg.data_dir.clone()).await {
        Some(stats) => {
            let need = (size as f64 * 1.1) as u64;
            if stats.free_bytes < need {
                tracing::warn!(
                    free = stats.free_bytes, need,
                    "db retention: skipping auto_vacuum conversion (insufficient free disk); will retry next boot"
                );
                return Ok(false);
            }
        }
        None => {
            tracing::warn!(
                "db retention: could not stat disk before auto_vacuum conversion; skipping"
            );
            return Ok(false);
        }
    }
    // Pin ONE connection: the requesting PRAGMA and the applying VACUUM are connection-scoped — the
    // VACUUM only converts the DB if it runs on the same connection that set the pragma. On the live
    // multi-connection pool the two statements could otherwise land on different connections, so the
    // multi-minute VACUUM would complete without flipping the mode.
    let mut conn = pool
        .acquire()
        .await
        .context("acquire connection for auto_vacuum conversion")?;
    sqlx::query("PRAGMA auto_vacuum=INCREMENTAL")
        .execute(&mut *conn)
        .await
        .context("set auto_vacuum")?;
    sqlx::query("VACUUM")
        .execute(&mut *conn)
        .await
        .context("VACUUM (auto_vacuum conversion)")?;
    // Convergence check: a multi-minute VACUUM that did not actually flip the mode must NOT report
    // success (that would silently disable the size cap and re-run every boot).
    let after: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
        .fetch_one(&mut *conn)
        .await
        .context("verify auto_vacuum after VACUUM")?;
    if after != 2 {
        anyhow::bail!(
            "auto_vacuum conversion did not take effect (mode still {after} after VACUUM)"
        );
    }
    tracing::info!(
        prior_mode = mode,
        "db retention: converted DB to auto_vacuum=INCREMENTAL (one-time)"
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    // Build an in-memory-ish temp DB in INCREMENTAL auto_vacuum mode with the columns we touch.
    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA auto_vacuum=INCREMENTAL")
            .execute(&pool)
            .await
            .unwrap();
        // auto_vacuum only takes effect after a VACUUM on a fresh DB before tables:
        sqlx::query("VACUUM").execute(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE detections (id TEXT PRIMARY KEY, created_at TEXT NOT NULL, blob TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Real settings table schema (from migration 0002_settings.sql):
        // value is TEXT (not INTEGER), and updated_at is required.
        sqlx::query(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn seed_detections(pool: &SqlitePool, n: usize) {
        for i in 0..n {
            // ~2KB blob per row so a few thousand rows exceed a tiny cap.
            let blob = "x".repeat(2000);
            let ts = format!("2026-07-{:02}T00:00:{:02}Z", 1 + (i % 27), i % 60);
            sqlx::query("INSERT INTO detections (id, created_at, blob) VALUES (?, ?, ?)")
                .bind(format!("det-{i:08}"))
                .bind(ts)
                .bind(blob)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn db_size_bytes_is_page_count_times_size() {
        let pool = test_pool().await;
        let pc: i64 = sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&pool)
            .await
            .unwrap();
        let ps: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            db_size_bytes(&pool).await.unwrap(),
            (pc as u64) * (ps as u64)
        );
    }

    #[tokio::test]
    async fn cap_disabled_is_noop() {
        let pool = test_pool().await;
        seed_detections(&pool, 500).await;
        // Config::default() does not exist in this crate; use Config::from_env() per test_cfg idiom.
        let mut cfg = Config::from_env();
        cfg.max_db_bytes = 0; // disabled
        let before = count(&pool, "detections").await;
        let rep = enforce_db_cap(&pool, &cfg).await.unwrap();
        assert_eq!(rep.detections_deleted, 0);
        assert_eq!(count(&pool, "detections").await, before);
    }

    #[tokio::test]
    async fn enforce_sheds_oldest_detections_until_under_cap() {
        let pool = test_pool().await;
        seed_detections(&pool, 4000).await; // ~ several MB of blobs
                                            // Config::default() does not exist in this crate; use Config::from_env() per test_cfg idiom.
        let mut cfg = Config::from_env();
        cfg.max_db_bytes = 2 * 1024 * 1024; // 2 MB tiny cap
        let before = count(&pool, "detections").await;
        let rep = enforce_db_cap(&pool, &cfg).await.unwrap();
        assert!(rep.detections_deleted > 0, "should have deleted rows");
        assert!(
            count(&pool, "detections").await < before,
            "row count dropped"
        );
        assert!(
            db_size_bytes(&pool).await.unwrap() <= cfg.max_db_bytes || rep.over_cap,
            "under cap, or explicitly flagged over_cap"
        );
        // Oldest were removed first: the earliest id must be gone, a later id must remain.
        let earliest_gone: i64 =
            sqlx::query_scalar("SELECT count(*) FROM detections WHERE id = 'det-00000000'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(earliest_gone, 0, "oldest detection deleted");
    }

    #[tokio::test]
    async fn settings_override_beats_env_default() {
        let pool = test_pool().await;
        seed_detections(&pool, 2000).await;
        // env default huge (no prune), but the settings override is tiny (prune).
        // value is TEXT in the real schema, and updated_at is required.
        sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)")
            .bind(settings::DB_MAX_BYTES)
            .bind("1048576") // 1 MB as TEXT (matching real schema)
            .bind("2026-07-11T00:00:00Z")
            .execute(&pool)
            .await
            .unwrap();
        // Config::default() does not exist in this crate; use Config::from_env() per test_cfg idiom.
        let mut cfg = Config::from_env();
        cfg.max_db_bytes = 100 * 1024 * 1024 * 1024; // 100 GB env default
        let rep = enforce_db_cap(&pool, &cfg).await.unwrap();
        assert!(
            rep.detections_deleted > 0,
            "settings override should force pruning"
        );
    }

    #[tokio::test]
    async fn conversion_sets_incremental_on_a_plain_db() {
        // A file-based temp DB created WITHOUT the incremental pragma starts in mode 0 (NONE).
        // We use a file DB (not memory) because VACUUM behaves correctly with file-backed DBs.
        let dir =
            std::env::temp_dir().join(format!("heldar-autovacuum-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        // Create a minimal table so the DB is non-trivial.
        sqlx::query("CREATE TABLE detections (id TEXT PRIMARY KEY, created_at TEXT, blob TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let mode0: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(mode0, 2, "starts non-incremental");
        // Point data_dir at a temp dir with plenty of free space so the disk guard passes.
        let mut cfg = Config::from_env();
        cfg.data_dir = std::env::temp_dir();
        let converted = ensure_incremental_autovacuum(&pool, &cfg).await.unwrap();
        let mode1: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(converted, "should have run a conversion VACUUM");
        assert_eq!(mode1, 2, "now INCREMENTAL");
        // Calling again must be a no-op (already mode 2).
        let converted2 = ensure_incremental_autovacuum(&pool, &cfg).await.unwrap();
        assert!(!converted2, "already INCREMENTAL — should skip");
        drop(pool);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn conversion_converges_on_multi_connection_pool() {
        // A pooled, file-backed mode-0 DB with >1 connection: the pinned PRAGMA+VACUUM must still flip
        // the mode to 2 (guards the connection-scoping trap). Must be a FILE DB — :memory: gives each
        // pooled connection its own separate database.
        let dir =
            std::env::temp_dir().join(format!("heldar-autovacuum-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE detections (id TEXT PRIMARY KEY, created_at TEXT, blob TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let mode0: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(mode0, 2, "starts non-incremental");
        let mut cfg = Config::from_env();
        cfg.data_dir = std::env::temp_dir(); // plenty of free space so the disk guard passes
        let converted = ensure_incremental_autovacuum(&pool, &cfg).await.unwrap();
        assert!(converted, "should have converted");
        // Verification note: SQLite caches `auto_vacuum` per-connection at file-open time (it's read
        // from the file header into that connection's in-memory Btree state and is NOT refreshed by a
        // VACUUM run on a different connection). So an already-open connection elsewhere in THIS pool
        // (e.g. the one used for the `mode0` check above) can keep reporting the pre-conversion value
        // even though the on-disk file is genuinely converted — that staleness is an orthogonal SQLite
        // quirk, not the connection-scoping bug this test guards against. The only way to observe the
        // actual on-disk state is a connection that opens the file fresh, so drop this pool and reopen
        // one against the same file (equivalent to what the next server boot would see).
        drop(pool);
        let pool2 = SqlitePoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        let mode1: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&pool2)
            .await
            .unwrap();
        assert_eq!(
            mode1, 2,
            "converged to INCREMENTAL on a multi-connection pool"
        );
        drop(pool2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
