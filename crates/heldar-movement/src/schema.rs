//! Movement intelligence owns its schema, applied idempotently against the shared kernel pool.

use sqlx::SqlitePool;

pub async fn init(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::raw_sql(include_str!("schema.sql"))
        .execute(pool)
        .await?;
    Ok(())
}
