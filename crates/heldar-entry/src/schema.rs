//! The access-control app owns its own schema, applied idempotently against the shared kernel pool on startup
//! (single-tenant-per-deployment). The open kernel does not define these domain tables.

use sqlx::SqlitePool;

/// Create the access-control tables if they do not exist. Called by the composing server after the
/// kernel migrations have run.
pub async fn init(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::raw_sql(include_str!("schema.sql"))
        .execute(pool)
        .await?;
    Ok(())
}
