//! Semantic search owns its (small) query-log schema, applied idempotently against the shared pool.

use sqlx::SqlitePool;

pub async fn init(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::raw_sql(include_str!("schema.sql"))
        .execute(pool)
        .await?;
    Ok(())
}
