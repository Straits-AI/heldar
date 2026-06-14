//! Alert notifier: delivers new warning/critical events to a configured webhook (POST JSON).
//! The delivery cursor is persisted (survives restarts, so events generated during downtime are
//! still delivered); retryable failures (5xx / 429 / network) do not advance the cursor.
//! `main` only supervises this task when a webhook is configured, so the no-webhook path returns
//! once without a respawn storm.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::models::Event;

const CURSOR_KEY: &str = "notifier_cursor";
const BATCH: i64 = 100;

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    let Some(url) = cfg.alert_webhook_url.clone() else {
        tracing::info!("notifier: no HELDAR_ALERT_WEBHOOK_URL set; alerting disabled");
        return;
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "notifier: failed to build http client");
            return;
        }
    };
    tracing::info!(%url, "notifier: alerting enabled (warning/critical events)");

    // Resume from the persisted cursor; the first ever run starts at "now" (no history replay).
    let mut cursor = match load_cursor(&pool).await {
        Some(c) => c,
        None => {
            let now = Utc::now();
            let _ = save_cursor(&pool, now).await;
            now
        }
    };

    let mut tick = tokio::time::interval(Duration::from_secs(cfg.notifier_interval_s.max(5)));
    loop {
        tick.tick().await;
        // Drain until a batch comes back not-full (backlog cleared) or a failure stops progress.
        loop {
            match deliver_batch(&pool, &client, &url, cursor).await {
                Ok(Some((latest, n))) => {
                    cursor = latest;
                    let _ = save_cursor(&pool, cursor).await;
                    if n < BATCH as usize {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(error = %e, "notifier: delivery cycle failed");
                    break;
                }
            }
        }
    }
}

async fn load_cursor(pool: &SqlitePool) -> Option<DateTime<Utc>> {
    let v: Option<String> = sqlx::query_scalar("SELECT value FROM app_state WHERE key = ?")
        .bind(CURSOR_KEY)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    v.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc))
}

async fn save_cursor(pool: &SqlitePool, cursor: DateTime<Utc>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO app_state (key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(CURSOR_KEY)
    .bind(cursor.to_rfc3339())
    .bind(Utc::now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Deliver one batch of events newer than `cursor`. Returns `Some((new_cursor, delivered))` after
/// any progress, or `None` if there is nothing to deliver or delivery stopped at a retryable
/// failure before any event was delivered (cursor must not advance past the failing event).
async fn deliver_batch(
    pool: &SqlitePool,
    client: &reqwest::Client,
    url: &str,
    cursor: DateTime<Utc>,
) -> anyhow::Result<Option<(DateTime<Utc>, usize)>> {
    let events = sqlx::query_as::<_, Event>(
        "SELECT * FROM events
         WHERE severity IN ('warning', 'critical') AND created_at > ?
         ORDER BY created_at ASC LIMIT ?",
    )
    .bind(cursor)
    .bind(BATCH)
    .fetch_all(pool)
    .await?;
    if events.is_empty() {
        return Ok(None);
    }

    let mut latest: Option<DateTime<Utc>> = None;
    let mut delivered = 0usize;
    for ev in events {
        let body = json!({
            "source": "heldar-core",
            "event_id": ev.id,
            "event_type": ev.event_type,
            "severity": ev.severity,
            "camera_id": ev.camera_id,
            "timestamp": ev.timestamp,
            "payload": ev.payload.0,
        });
        match client.post(url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                let code = resp.status();
                if code.is_server_error() || code.as_u16() == 429 {
                    // Retryable: stop without advancing past this event.
                    tracing::warn!(status = %code, event = %ev.event_type, "notifier: retryable webhook failure; will retry next cycle");
                    return Ok(latest.map(|l| (l, delivered)));
                }
                // Other 4xx: the event won't ever be accepted; log and skip past it.
                tracing::warn!(status = %code, event = %ev.event_type, "notifier: webhook rejected event; skipping");
            }
            Err(e) => {
                tracing::warn!(error = %e, "notifier: webhook post failed; will retry next cycle");
                return Ok(latest.map(|l| (l, delivered)));
            }
        }
        latest = Some(ev.created_at);
        delivered += 1;
    }
    Ok(latest.map(|l| (l, delivered)))
}
