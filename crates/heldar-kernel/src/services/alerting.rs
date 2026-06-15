//! UI-configurable alerting settings, persisted in the `app_state` key/value table so changes take
//! effect WITHOUT a restart: the notifier service ([`crate::services::notifier`]) re-reads them every
//! cycle, and the `/api/v1/system/alerting` routes read + write them.
//!
//! Three keys are used (all optional — sensible defaults apply):
//!   - `alert_webhook_url`   — the POST target (empty/absent = unset; falls back to the env config)
//!   - `alert_enabled`       — "true"/"false" (default true when a url is set)
//!   - `alert_min_severity`  — "warning" | "critical" (default "warning")
//!
//! The get/set style mirrors the notifier cursor helpers (the same `app_state` upsert).

use chrono::Utc;
use sqlx::SqlitePool;

pub const WEBHOOK_KEY: &str = "alert_webhook_url";
pub const ENABLED_KEY: &str = "alert_enabled";
pub const MIN_SEVERITY_KEY: &str = "alert_min_severity";

/// Read one `app_state` value (mirrors the notifier's cursor read).
pub async fn get_state(pool: &SqlitePool, key: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT value FROM app_state WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// Upsert one `app_state` value (mirrors the notifier's cursor save).
pub async fn set_state(pool: &SqlitePool, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO app_state (key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(Utc::now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete one `app_state` key (used to clear the configured webhook).
pub async fn clear_state(pool: &SqlitePool, key: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM app_state WHERE key = ?")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// Resolved alerting settings for one notifier cycle / one API read.
#[derive(Debug)]
pub struct Resolved {
    /// The webhook url (stored value, else the env fallback), or None when unconfigured.
    pub webhook_url: Option<String>,
    /// Whether delivery is enabled (default true; "false" disables).
    pub enabled: bool,
    /// `warning` (warning+critical) or `critical` (critical only).
    pub min_severity: String,
}

/// Resolve the effective settings from `app_state`, falling back to `fallback_url` (the env
/// `HELDAR_ALERT_WEBHOOK_URL`) when no webhook is stored — backward compat with the old env-only path.
pub async fn resolve(pool: &SqlitePool, fallback_url: Option<&str>) -> Resolved {
    let stored = get_state(pool, WEBHOOK_KEY)
        .await
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let webhook_url = stored.or_else(|| {
        fallback_url
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    let enabled = !matches!(get_state(pool, ENABLED_KEY).await.as_deref(), Some("false"));
    let min_severity = match get_state(pool, MIN_SEVERITY_KEY).await.as_deref() {
        Some("critical") => "critical",
        _ => "warning",
    }
    .to_string();
    Resolved {
        webhook_url,
        enabled,
        min_severity,
    }
}

/// SQL predicate selecting the event severities that pass `min_severity` (`critical` => critical
/// only; anything else => warning+critical). Values are static literals — never user input — so this
/// is safe to splice into the query (and uses the `(severity, created_at)` index either way).
pub fn severity_sql(min_severity: &str) -> &'static str {
    if min_severity == "critical" {
        "severity = 'critical'"
    } else {
        "severity IN ('warning', 'critical')"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_sql_thresholds() {
        assert_eq!(severity_sql("critical"), "severity = 'critical'");
        // warning (and any unknown value) admits both warning and critical.
        assert_eq!(
            severity_sql("warning"),
            "severity IN ('warning', 'critical')"
        );
        assert_eq!(
            severity_sql("anything"),
            "severity IN ('warning', 'critical')"
        );
    }
}
