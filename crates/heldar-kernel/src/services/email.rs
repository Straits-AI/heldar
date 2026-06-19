//! Email/SMTP notifier — the off-by-default `smtp` feature.
//!
//! Delivers matching events to configured recipients over SMTP. Env-configured (HELDAR_SMTP_*); parks
//! forever when SMTP isn't configured (host / from / recipients), so the supervisor never respawns it.
//! Mirrors the webhook delivery loop: a cursor over `events.created_at` plus a severity floor. The
//! cursor starts at boot — no backlog replay, like a fresh webhook subscription — and delivery is
//! at-most-once best-effort: a send failure is logged and the cursor still advances, so a dead relay
//! can never wedge the loop. (Webhooks remain the durable, retrying, UI-managed channel; email is a
//! lightweight always-on relay for operators who want inbox alerts.)

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::models::Event;

const BATCH: i64 = 50;

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    let park = || async {
        std::future::pending::<()>().await;
    };
    let (Some(host), Some(from)) = (cfg.smtp_host.as_deref(), cfg.smtp_from.as_deref()) else {
        tracing::info!("email notifier: HELDAR_SMTP_HOST / _FROM unset; email disabled");
        return park().await;
    };
    if cfg.smtp_recipients.is_empty() {
        tracing::warn!("email notifier: no HELDAR_SMTP_RECIPIENTS; email disabled");
        return park().await;
    }
    let Ok(from_mbox) = from.parse::<Mailbox>() else {
        tracing::error!(
            from,
            "email notifier: invalid HELDAR_SMTP_FROM; email disabled"
        );
        return park().await;
    };
    let recipients: Vec<Mailbox> = cfg
        .smtp_recipients
        .iter()
        .filter_map(|r| r.parse::<Mailbox>().ok())
        .collect();
    if recipients.is_empty() {
        tracing::error!("email notifier: no valid recipient addresses; email disabled");
        return park().await;
    }
    let transport = match build_transport(&cfg, host) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "email notifier: bad SMTP config; email disabled");
            return park().await;
        }
    };

    tracing::info!(
        host,
        port = cfg.smtp_port,
        recipients = recipients.len(),
        "email notifier: started"
    );
    let mut cursor = Utc::now();
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.smtp_interval_s.max(5)));
    loop {
        tick.tick().await;
        let events = match fetch_events(&pool, cursor, &cfg.smtp_min_severity).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "email notifier: event query failed");
                continue;
            }
        };
        for ev in events {
            match build_message(&from_mbox, &recipients, &ev) {
                Ok(msg) => match transport.send(msg).await {
                    Ok(_) => tracing::debug!(event = %ev.id, "emailed event"),
                    Err(e) => {
                        tracing::warn!(error = %e, event = %ev.id, "email send failed; advancing past it")
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, event = %ev.id, "email build failed; skipping")
                }
            }
            cursor = ev.created_at; // at-most-once: advance regardless so a dead relay can't wedge us
        }
    }
}

fn build_transport(
    cfg: &Config,
    host: &str,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, lettre::transport::smtp::Error> {
    let mut builder = match cfg.smtp_tls.as_str() {
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
        "implicit" => AsyncSmtpTransport::<Tokio1Executor>::relay(host)?, // implicit TLS (465)
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?, // STARTTLS (587), default
    };
    builder = builder.port(cfg.smtp_port);
    if let (Some(u), Some(p)) = (cfg.smtp_username.as_deref(), cfg.smtp_password.as_deref()) {
        builder = builder.credentials(Credentials::new(u.to_string(), p.to_string()));
    }
    Ok(builder.build())
}

fn build_message(
    from: &Mailbox,
    to: &[Mailbox],
    ev: &Event,
) -> Result<Message, lettre::error::Error> {
    let cam = ev.camera_id.as_deref().unwrap_or("-");
    let subject = format!(
        "[Heldar] {}: {} ({})",
        ev.severity.to_uppercase(),
        ev.event_type,
        cam
    );
    let body = format!(
        "Event:    {}\nSeverity: {}\nCamera:   {}\nSite:     {}\nTime:     {}\n\nDetails:\n{}\n",
        ev.event_type,
        ev.severity,
        cam,
        ev.site_id.as_deref().unwrap_or("-"),
        ev.timestamp.to_rfc3339(),
        serde_json::to_string_pretty(&ev.payload.0).unwrap_or_default(),
    );
    let mut builder = Message::builder().from(from.clone()).subject(subject);
    for r in to {
        builder = builder.to(r.clone());
    }
    builder.header(ContentType::TEXT_PLAIN).body(body)
}

async fn fetch_events(
    pool: &SqlitePool,
    cursor: DateTime<Utc>,
    min_severity: &str,
) -> sqlx::Result<Vec<Event>> {
    let sql = format!(
        "SELECT * FROM events WHERE {} AND created_at > ? ORDER BY created_at ASC LIMIT ?",
        min_severity_sql(min_severity),
    );
    sqlx::query_as::<_, Event>(&sql)
        .bind(cursor)
        .bind(BATCH)
        .fetch_all(pool)
        .await
}

/// Static-literal severity floor (never user input), spliced into the query like the webhook engine's.
fn min_severity_sql(min: &str) -> &'static str {
    match min {
        "critical" => "severity = 'critical'",
        "warning" => "severity IN ('warning', 'critical')",
        _ => "1 = 1",
    }
}
