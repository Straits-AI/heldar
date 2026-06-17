//! Webhook delivery engine — the SINGLE deliverer of generic events to external systems, superseding
//! the old single-URL alert notifier.
//!
//! Each enabled [`WebhookSubscription`] is an independent at-least-once deliverer: it keeps its own
//! persisted `cursor_at` (an `events.created_at`, mirroring the old notifier cursor), an event-type +
//! severity filter, and an optional HMAC-SHA256 secret. Every tick we load the enabled subscriptions
//! and, for each, deliver the events newer than its cursor that pass the filter — POSTing the JSON
//! envelope with `X-Heldar-Event` / `X-Heldar-Delivery` / `X-Heldar-Timestamp` headers and, when a
//! secret is set, `X-Heldar-Signature: sha256=<hex HMAC-SHA256(secret, raw_body)>`. Each attempt is
//! recorded in `webhook_deliveries`; a retryable failure keeps the cursor (retried next cycle) until
//! the per-event attempts in that ledger reach [`MAX_ATTEMPTS`], after which the event is given up on
//! and the cursor advances so one bad endpoint cannot wedge the queue forever.
//!
//! `run()` NEVER returns: with no enabled subscriptions it idles the cycle. The supervisor in `main`
//! therefore spawns it unconditionally and never tight-loops respawning it.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::config::Config;
use crate::models::{Event, WebhookSubscription};

type HmacSha256 = Hmac<Sha256>;

/// Per-cycle event fetch size (mirrors the old notifier batch).
const BATCH: i64 = 100;
/// Give up on an event after this many recorded failed attempts (so a dead endpoint can't wedge the
/// per-subscription cursor forever — the cursor then advances past the poison event).
const MAX_ATTEMPTS: i64 = 5;

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    // Built once, outside the loop, and reused across cycles. On the (practically impossible) build
    // failure, park forever rather than return — returning would have the supervisor respawn us.
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "webhooks: failed to build http client; idling");
            std::future::pending::<reqwest::Client>().await
        }
    };

    let mut tick = tokio::time::interval(Duration::from_secs(cfg.notifier_interval_s.max(5)));
    loop {
        tick.tick().await;
        let subs = match load_enabled(&pool).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "webhooks: failed to load subscriptions");
                continue;
            }
        };
        if subs.is_empty() {
            // No enabled subscriptions: idle quietly this cycle (never return — see the module note).
            continue;
        }
        for sub in subs {
            if let Err(e) = deliver_subscription(&pool, &client, &sub).await {
                tracing::error!(error = %e, subscription = %sub.id, "webhooks: delivery cycle failed");
            }
        }
    }
}

/// Load enabled subscriptions, oldest first (stable delivery order across cycles).
async fn load_enabled(pool: &SqlitePool) -> sqlx::Result<Vec<WebhookSubscription>> {
    sqlx::query_as::<_, WebhookSubscription>(
        "SELECT * FROM webhook_subscriptions WHERE enabled = 1 ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await
}

/// Persist the advanced delivery cursor for one subscription.
async fn save_cursor(pool: &SqlitePool, sub_id: &str, cursor: DateTime<Utc>) -> sqlx::Result<()> {
    sqlx::query("UPDATE webhook_subscriptions SET cursor_at = ? WHERE id = ?")
        .bind(cursor)
        .bind(sub_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deliver every pending, matching event for one subscription, draining in batches until caught up or
/// stopped at a retryable failure.
async fn deliver_subscription(
    pool: &SqlitePool,
    client: &reqwest::Client,
    sub: &WebhookSubscription,
) -> anyhow::Result<()> {
    // First time we see this subscription (cursor NULL): start at "now", no backlog replay — mirrors
    // the old notifier so a subscription added LATER does not replay the full event history.
    let Some(mut cursor) = sub.cursor_at else {
        save_cursor(pool, &sub.id, Utc::now()).await?;
        return Ok(());
    };

    loop {
        let events = fetch_events(pool, cursor, &sub.min_severity).await?;
        if events.is_empty() {
            break;
        }
        let n = events.len();
        let mut advanced = false;
        for ev in events {
            // Event-type filter: ["*"] matches all, otherwise exact membership. A non-match is not a
            // delivery (nothing recorded) — just step the cursor past it.
            if !matches_event_type(&sub.event_types.0, &ev.event_type) {
                cursor = ev.created_at;
                advanced = true;
                continue;
            }
            match try_deliver(pool, client, sub, &ev).await {
                DeliverOutcome::Advance => {
                    cursor = ev.created_at;
                    advanced = true;
                }
                DeliverOutcome::Retry => {
                    // Retryable failure under the attempt bound: keep the cursor on this event (retry
                    // next cycle) but persist progress made on earlier events in this batch.
                    if advanced {
                        save_cursor(pool, &sub.id, cursor).await?;
                    }
                    return Ok(());
                }
            }
        }
        if advanced {
            save_cursor(pool, &sub.id, cursor).await?;
        }
        if n < BATCH as usize {
            break;
        }
    }
    Ok(())
}

/// Fetch the next batch of events newer than `cursor` that pass the severity floor, oldest first.
async fn fetch_events(
    pool: &SqlitePool,
    cursor: DateTime<Utc>,
    min_severity: &str,
) -> sqlx::Result<Vec<Event>> {
    let sql = format!(
        "SELECT * FROM events
         WHERE {} AND created_at > ?
         ORDER BY created_at ASC LIMIT ?",
        min_severity_sql(min_severity),
    );
    sqlx::query_as::<_, Event>(&sql)
        .bind(cursor)
        .bind(BATCH)
        .fetch_all(pool)
        .await
}

enum DeliverOutcome {
    /// Move the cursor past this event (delivered, or given up on after the attempt bound).
    Advance,
    /// Retryable failure under the attempt bound: stop this subscription's cycle, keep the cursor.
    Retry,
}

/// Attempt to deliver one event, recording the attempt in the `webhook_deliveries` ledger.
async fn try_deliver(
    pool: &SqlitePool,
    client: &reqwest::Client,
    sub: &WebhookSubscription,
    ev: &Event,
) -> DeliverOutcome {
    let prior_failures: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM webhook_deliveries
          WHERE subscription_id = ? AND event_id = ? AND status = 'failed'",
    )
    .bind(&sub.id)
    .bind(&ev.id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let attempt = prior_failures + 1;

    let delivery_id = format!("whd_{}", Uuid::new_v4().simple());
    let body = event_body(ev);
    let res = send_event(
        client,
        &sub.url,
        &delivery_id,
        &ev.event_type,
        sub.secret.as_deref(),
        &body,
    )
    .await;

    record_delivery(
        pool,
        &delivery_id,
        &sub.id,
        Some(&ev.id),
        Some(&ev.event_type),
        res.ok,
        attempt,
        res.status.map(i64::from),
        res.error.as_deref(),
    )
    .await;

    if res.ok {
        DeliverOutcome::Advance
    } else if attempt >= MAX_ATTEMPTS {
        tracing::warn!(
            subscription = %sub.id,
            event = %ev.id,
            attempts = attempt,
            "webhooks: giving up on event after max attempts; advancing cursor past it"
        );
        DeliverOutcome::Advance
    } else {
        tracing::warn!(
            subscription = %sub.id,
            event = %ev.id,
            attempt,
            error = res.error.as_deref().unwrap_or("non-2xx"),
            "webhooks: delivery failed; will retry next cycle"
        );
        DeliverOutcome::Retry
    }
}

/// The JSON envelope POSTed for an event (the body that is HMAC-signed verbatim).
pub fn event_body(ev: &Event) -> Value {
    json!({
        "id": ev.id,
        "camera_id": ev.camera_id,
        "site_id": ev.site_id,
        "event_type": ev.event_type,
        "severity": ev.severity,
        "timestamp": ev.timestamp,
        "payload": ev.payload.0,
    })
}

/// Outcome of a single signed POST: success flag, HTTP status (if a response came back), and an error
/// string for the delivery ledger.
pub struct SendResult {
    pub ok: bool,
    pub status: Option<u16>,
    pub error: Option<String>,
}

/// POST a signed webhook body. The body is serialized ONCE and both signed and sent verbatim so the
/// `X-Heldar-Signature` always covers the exact bytes the receiver gets. Used by the delivery loop and
/// by the synthetic `/test` route.
pub async fn send_event(
    client: &reqwest::Client,
    url: &str,
    delivery_id: &str,
    event_type: &str,
    secret: Option<&str>,
    body: &Value,
) -> SendResult {
    let raw = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
    let mut req = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("X-Heldar-Event", event_type)
        .header("X-Heldar-Delivery", delivery_id)
        .header("X-Heldar-Timestamp", Utc::now().timestamp().to_string())
        .body(raw.clone());
    if let Some(secret) = secret.filter(|s| !s.is_empty()) {
        req = req.header("X-Heldar-Signature", sign(secret, raw.as_bytes()));
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            SendResult {
                ok: status.is_success(),
                status: Some(status.as_u16()),
                error: if status.is_success() {
                    None
                } else {
                    Some(format!("webhook returned HTTP {}", status.as_u16()))
                },
            }
        }
        Err(e) => SendResult {
            ok: false,
            status: None,
            error: Some(e.to_string()),
        },
    }
}

/// Insert one row into the `webhook_deliveries` ledger. Best-effort: a failure is logged, not fatal.
#[allow(clippy::too_many_arguments)]
pub async fn record_delivery(
    pool: &SqlitePool,
    id: &str,
    subscription_id: &str,
    event_id: Option<&str>,
    event_type: Option<&str>,
    delivered: bool,
    attempts: i64,
    response_code: Option<i64>,
    error: Option<&str>,
) {
    let now = Utc::now();
    let delivered_at = if delivered { Some(now) } else { None };
    let status = if delivered { "delivered" } else { "failed" };
    let res = sqlx::query(
        "INSERT INTO webhook_deliveries
           (id, subscription_id, event_id, event_type, status, attempts, response_code, error, created_at, delivered_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(subscription_id)
    .bind(event_id)
    .bind(event_type)
    .bind(status)
    .bind(attempts)
    .bind(response_code)
    .bind(error)
    .bind(now)
    .bind(delivered_at)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::error!(error = %e, subscription = %subscription_id, "webhooks: failed to record delivery");
    }
}

/// `sha256=<hex>` where `<hex>` is HMAC-SHA256(secret, body). HMAC accepts any key length.
fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(body);
    format!(
        "sha256={}",
        crate::auth::hex_encode(&mac.finalize().into_bytes())
    )
}

/// Whether `event_type` is selected by a subscription's filter. `["*"]` matches everything; otherwise
/// it is exact membership.
pub fn matches_event_type(filter: &[String], event_type: &str) -> bool {
    filter.iter().any(|t| t == "*") || filter.iter().any(|t| t == event_type)
}

/// SQL predicate selecting the severities at or above `min_severity`. Values are static literals (never
/// user input) so this is safe to splice into the query. `info` (or unknown) admits all severities.
fn min_severity_sql(min_severity: &str) -> &'static str {
    match min_severity {
        "critical" => "severity = 'critical'",
        "warning" => "severity IN ('warning', 'critical')",
        _ => "1 = 1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches_everything() {
        let star = vec!["*".to_string()];
        assert!(matches_event_type(&star, "zone_enter"));
        assert!(matches_event_type(&star, "anything_at_all"));
    }

    #[test]
    fn explicit_set_is_exact_membership() {
        let set = vec!["zone_enter".to_string(), "disk_pressure".to_string()];
        assert!(matches_event_type(&set, "zone_enter"));
        assert!(matches_event_type(&set, "disk_pressure"));
        assert!(!matches_event_type(&set, "zone_exit"));
        assert!(!matches_event_type(&[], "zone_enter"));
    }

    #[test]
    fn severity_floor_thresholds() {
        assert_eq!(min_severity_sql("critical"), "severity = 'critical'");
        assert_eq!(
            min_severity_sql("warning"),
            "severity IN ('warning', 'critical')"
        );
        // info (and any unknown value) admits all severities.
        assert_eq!(min_severity_sql("info"), "1 = 1");
        assert_eq!(min_severity_sql("whatever"), "1 = 1");
    }

    #[test]
    fn signature_is_stable_prefixed_hmac_sha256() {
        // Known-answer: HMAC-SHA256(key="key", msg="The quick brown fox jumps over the lazy dog").
        let sig = sign("key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            sig,
            "sha256=f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
        // Stable + key-sensitive.
        assert_eq!(sign("s", b"body"), sign("s", b"body"));
        assert_ne!(sign("s1", b"body"), sign("s2", b"body"));
    }
}
