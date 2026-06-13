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
        .max_connections(8)
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
