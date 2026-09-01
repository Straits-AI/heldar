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

/// Stop the kernel migrations at `version` — the hook that makes an UPGRADE testable.
///
/// A migration that backfills existing rows can only be exercised from a database that predates it,
/// and [`run_migrations`] always lands on head, so a backfill would otherwise be asserted against a
/// table it just created empty. Tests migrate to `version`, seed the legacy rows a shipped box would
/// already hold, then call [`run_migrations`] and check what the backfill made of them.
///
/// Test-only on purpose: nothing on a real box may ever choose to stop half-way, and a running kernel
/// against a partially-migrated schema is a class of failure worth making unrepresentable.
#[cfg(test)]
pub(crate) async fn run_migrations_up_to(pool: &SqlitePool, version: i64) -> anyhow::Result<()> {
    let mut m = sqlx::migrate!("./migrations");
    m.migrations = m
        .migrations
        .iter()
        .filter(|mig| mig.version <= version)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    m.run(pool).await?;
    Ok(())
}

/// One embedded, versioned migration for a composed app crate. `version` is the numeric filename prefix
/// (`0001_init.sql` → 1); keep them dense and ascending. Ship a schema change as a NEW migration —
/// never edit an applied one (the checksum guard in [`run_app_migrations`] rejects that).
pub struct AppMigration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Apply a composed app crate's versioned, append-only migrations against the shared SQLite pool.
///
/// The kernel evolves its own schema through `sqlx::migrate!` (which owns `_sqlx_migrations`). Apps
/// cannot share that table — several apps + the kernel on ONE database would clobber each other's
/// version history — and they previously self-installed a single `CREATE TABLE IF NOT EXISTS` blob with
/// no versioning, so a shipped schema change silently no-opped on an already-booted box. This runner
/// gives each app the same numbered, append-only discipline: applied versions are tracked in a single
/// shared `_heldar_app_migrations` table keyed by `component`, and each new migration is applied +
/// recorded atomically.
///
/// Upgrade safety: on an existing box whose tables were created by the old blob, migration `0001`
/// (that same idempotent blob) applies as a no-op and is recorded, so later migrations (`0002+`) run
/// cleanly — no data loss. `component` is a compile-time constant, bound as a parameter (never
/// interpolated), so there is no injection surface; it is asserted `[a-z_]+` for sanity.
pub async fn run_app_migrations(
    pool: &SqlitePool,
    component: &str,
    migrations: &[AppMigration],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !component.is_empty()
            && component
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_'),
        "app migration component `{component}` must be [a-z_]+"
    );
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _heldar_app_migrations (
            component  TEXT    NOT NULL,
            version    INTEGER NOT NULL,
            name       TEXT    NOT NULL,
            checksum   TEXT    NOT NULL,
            applied_at TEXT    NOT NULL,
            PRIMARY KEY (component, version)
        )",
    )
    .execute(pool)
    .await?;

    let applied: std::collections::HashMap<i64, String> =
        sqlx::query_as("SELECT version, checksum FROM _heldar_app_migrations WHERE component = ?")
            .bind(component)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    let mut ordered: Vec<&AppMigration> = migrations.iter().collect();
    ordered.sort_by_key(|m| m.version);

    for m in ordered {
        let checksum = sha256_hex(m.sql.as_bytes());
        if let Some(prev) = applied.get(&m.version) {
            anyhow::ensure!(
                prev == &checksum,
                "app `{component}` migration {} ({}) was already applied but its SQL changed \
                 (checksum mismatch) — never edit a shipped migration; add a new one",
                m.version,
                m.name
            );
            continue;
        }
        // Apply the DDL and record the version in ONE transaction, so a crash can't leave a
        // half-applied, unrecorded migration that re-runs next boot (non-idempotent DDL like
        // `ALTER TABLE ... ADD COLUMN` would then fail).
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(m.sql).execute(&mut *tx).await.map_err(|e| {
            anyhow::anyhow!(
                "app `{component}` migration {} ({}) failed: {e}",
                m.version,
                m.name
            )
        })?;
        sqlx::query(
            "INSERT INTO _heldar_app_migrations (component, version, name, checksum, applied_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(component)
        .bind(m.version)
        .bind(m.name)
        .bind(&checksum)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!(
            component,
            version = m.version,
            name = m.name,
            "applied app migration"
        );
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
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

    // ---- app migration runner -------------------------------------------------------------------

    /// A single-connection in-memory pool so every query hits the same physical `:memory:` database.
    async fn mem_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    async fn count(pool: &SqlitePool, component: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM _heldar_app_migrations WHERE component = ?")
            .bind(component)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn app_migrations_apply_once_and_are_idempotent() {
        let pool = mem_pool().await;
        let migs = &[AppMigration {
            version: 1,
            name: "init",
            sql: "CREATE TABLE IF NOT EXISTS t_demo (id INTEGER PRIMARY KEY, a TEXT);",
        }];
        run_app_migrations(&pool, "demo", migs).await.unwrap();
        sqlx::query("INSERT INTO t_demo (a) VALUES ('x')")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(count(&pool, "demo").await, 1);
        // Re-run on the next boot: no-op, one record, data intact.
        run_app_migrations(&pool, "demo", migs).await.unwrap();
        assert_eq!(count(&pool, "demo").await, 1);
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM t_demo")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn app_migrations_upgrade_applies_only_new_versions() {
        let pool = mem_pool().await;
        run_app_migrations(
            &pool,
            "up",
            &[AppMigration {
                version: 1,
                name: "init",
                sql: "CREATE TABLE t_up (id INTEGER PRIMARY KEY);",
            }],
        )
        .await
        .unwrap();
        // A later boot ships an extra migration; only version 2 runs.
        run_app_migrations(
            &pool,
            "up",
            &[
                AppMigration {
                    version: 1,
                    name: "init",
                    sql: "CREATE TABLE t_up (id INTEGER PRIMARY KEY);",
                },
                AppMigration {
                    version: 2,
                    name: "add_col",
                    sql: "ALTER TABLE t_up ADD COLUMN label TEXT;",
                },
            ],
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO t_up (id, label) VALUES (1, 'hi')")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(count(&pool, "up").await, 2);
    }

    #[tokio::test]
    async fn existing_blob_box_upgrades_without_data_loss() {
        // Simulate a box already running the old `CREATE TABLE IF NOT EXISTS` blob (tables + data, no
        // migration records), then getting the runner with 0001 = that blob + a 0002 column add.
        let pool = mem_pool().await;
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS t_leg (id INTEGER PRIMARY KEY, a TEXT);")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t_leg (a) VALUES ('keep')")
            .execute(&pool)
            .await
            .unwrap();
        run_app_migrations(
            &pool,
            "leg",
            &[
                AppMigration {
                    version: 1,
                    name: "init",
                    sql: "CREATE TABLE IF NOT EXISTS t_leg (id INTEGER PRIMARY KEY, a TEXT);",
                },
                AppMigration {
                    version: 2,
                    name: "add",
                    sql: "ALTER TABLE t_leg ADD COLUMN b TEXT;",
                },
            ],
        )
        .await
        .unwrap();
        let a: String = sqlx::query_scalar("SELECT a FROM t_leg WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(a, "keep", "pre-existing data survives the upgrade");
        assert_eq!(count(&pool, "leg").await, 2);
        // The new column is usable.
        sqlx::query("UPDATE t_leg SET b = 'new' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn app_migrations_reject_edited_shipped_migration() {
        let pool = mem_pool().await;
        run_app_migrations(
            &pool,
            "ck",
            &[AppMigration {
                version: 1,
                name: "init",
                sql: "CREATE TABLE t_ck (id INTEGER);",
            }],
        )
        .await
        .unwrap();
        // Editing an ALREADY-APPLIED migration's SQL must be rejected (checksum guard).
        let err = run_app_migrations(
            &pool,
            "ck",
            &[AppMigration {
                version: 1,
                name: "init",
                sql: "CREATE TABLE t_ck (id INTEGER, x TEXT);",
            }],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("checksum"), "got: {err}");
    }

    #[tokio::test]
    async fn app_migrations_isolate_components() {
        let pool = mem_pool().await;
        run_app_migrations(
            &pool,
            "aaa",
            &[AppMigration {
                version: 1,
                name: "i",
                sql: "CREATE TABLE t_a (id INTEGER);",
            }],
        )
        .await
        .unwrap();
        // A different component can reuse version 1 without collision.
        run_app_migrations(
            &pool,
            "bbb",
            &[AppMigration {
                version: 1,
                name: "i",
                sql: "CREATE TABLE t_b (id INTEGER);",
            }],
        )
        .await
        .unwrap();
        assert_eq!(count(&pool, "aaa").await, 1);
        assert_eq!(count(&pool, "bbb").await, 1);
    }

    #[tokio::test]
    async fn app_migrations_reject_bad_component_name() {
        let pool = mem_pool().await;
        assert!(run_app_migrations(&pool, "Bad-Name", &[]).await.is_err());
    }
}
