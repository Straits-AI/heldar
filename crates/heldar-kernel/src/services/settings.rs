//! Runtime-tunable key/value settings (the `settings` table). Operator-set policy that should take
//! effect without an env change + restart — e.g. the recording disk limits the dashboard can change.
//! Readers fall back to the static env [`Config`](crate::config::Config) when a key is unset, so the
//! env values remain the defaults.

use sqlx::SqlitePool;

/// Global recording size cap, in bytes (overrides `HELDAR_MAX_RECORDINGS_GB`).
pub const RECORDING_MAX_BYTES: &str = "recording_max_bytes";
/// Free-disk floor on the recordings filesystem, in bytes (overrides `HELDAR_MIN_FREE_DISK_GB`).
pub const RECORDING_MIN_FREE_BYTES: &str = "recording_min_free_bytes";

/// Read an integer setting, or `None` if unset / unparseable.
pub async fn get_i64(pool: &SqlitePool, key: &str) -> Option<i64> {
    let raw: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    raw.and_then(|s| s.parse::<i64>().ok())
}

/// Upsert an integer setting.
pub async fn set_i64(pool: &SqlitePool, key: &str, value: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value.to_string())
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a setting (reverting the reader to its env default).
pub async fn clear(pool: &SqlitePool, key: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn set_get_clear_roundtrip() {
        let pool = mem_pool().await;
        // unset → None (reader falls back to env default)
        assert_eq!(get_i64(&pool, RECORDING_MAX_BYTES).await, None);
        // set + read back
        set_i64(&pool, RECORDING_MAX_BYTES, 500_000_000)
            .await
            .unwrap();
        assert_eq!(get_i64(&pool, RECORDING_MAX_BYTES).await, Some(500_000_000));
        // upsert overwrites
        set_i64(&pool, RECORDING_MAX_BYTES, 1_000_000_000)
            .await
            .unwrap();
        assert_eq!(
            get_i64(&pool, RECORDING_MAX_BYTES).await,
            Some(1_000_000_000)
        );
        // clear → back to None
        clear(&pool, RECORDING_MAX_BYTES).await.unwrap();
        assert_eq!(get_i64(&pool, RECORDING_MAX_BYTES).await, None);
    }
}
