//! Red-zone breach rule engine. Polls recent zone-enter events on operator-designated red/restricted
//! zones and records each as a tracked incident (`breach_alerts`, open→acknowledged→resolved),
//! correlating a subject by track id → a vehicle plate when one is available. The kernel zone engine
//! already mirrors restricted-zone events to the notifier; this adds the worked incident + subject
//! correlation, so it does not re-notify.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::config::MovementConfig;

pub async fn run(pool: SqlitePool, cfg: Arc<MovementConfig>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.engine_interval_s));
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&pool, &cfg).await {
            tracing::error!(error = %e, "movement breach: sweep failed");
        }
    }
}

/// Run the breach sweep once on demand (ops / tests).
pub async fn run_once(pool: &SqlitePool, cfg: &MovementConfig) -> anyhow::Result<()> {
    sweep(pool, cfg).await
}

#[derive(FromRow)]
struct ZoneEv {
    id: String,
    camera_id: String,
    zone_name: String,
    track_id: Option<String>,
    timestamp: DateTime<Utc>,
    evidence_path: Option<String>,
}

async fn sweep(pool: &SqlitePool, cfg: &MovementConfig) -> anyhow::Result<()> {
    // Resolve the set of red/breach zones (by configured kind).
    let mut red: Vec<(String, String)> = Vec::new(); // (zone_id, severity)
    for kind in &cfg.red_zone_kinds {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, severity FROM zones WHERE kind = ? AND enabled = 1")
                .bind(kind)
                .fetch_all(pool)
                .await?;
        red.extend(rows);
    }
    if red.is_empty() {
        return Ok(());
    }
    let scan_start = Utc::now() - TimeDelta::try_seconds(cfg.scan_window_s).unwrap();
    let mut created = 0u64;
    for (zone_id, severity) in &red {
        let events: Vec<ZoneEv> = sqlx::query_as(
            "SELECT id, camera_id, zone_name, track_id, timestamp, evidence_path
               FROM zone_events
              WHERE zone_id = ? AND event_type = 'enter' AND created_at >= ?",
        )
        .bind(zone_id)
        .bind(scan_start)
        .fetch_all(pool)
        .await?;
        for ev in events {
            // Correlate a subject: same per-camera track id in a vehicle entry event near this time.
            // Errors propagate (retry next sweep) rather than freezing the breach as 'unknown'.
            let (subject_type, subject) = correlate(pool, &ev).await?;
            // Backfill correlation on a re-seen breach whose plate arrived late from ANPR (the entry
            // event commits asynchronously, after the zone event); never clobber an already-correlated
            // breach or operator-set fields.
            let res = sqlx::query(
                "INSERT INTO breach_alerts
                   (id, camera_id, zone_id, zone_name, zone_event_id, rule, subject_type, subject,
                    track_id, severity, status, detail, evidence_path, created_at)
                 VALUES (?, ?, ?, ?, ?, 'red_zone_entry', ?, ?, ?, ?, 'open', ?, ?, ?)
                 ON CONFLICT(zone_event_id) DO UPDATE SET
                     subject_type = excluded.subject_type,
                     subject = excluded.subject,
                     detail = excluded.detail
                   WHERE breach_alerts.subject IS NULL AND excluded.subject IS NOT NULL",
            )
            .bind(format!("brc_{}", Uuid::new_v4().simple()))
            .bind(&ev.camera_id)
            .bind(zone_id)
            .bind(&ev.zone_name)
            .bind(&ev.id)
            .bind(&subject_type)
            .bind(&subject)
            .bind(&ev.track_id)
            .bind(severity)
            .bind(sqlx::types::Json(serde_json::json!({
                "zone_event_at": ev.timestamp, "correlation": if subject.is_some() {"track_to_plate"} else {"none"}
            })))
            .bind(&ev.evidence_path)
            .bind(Utc::now())
            .execute(pool)
            .await?;
            created += res.rows_affected();
        }
    }
    if created > 0 {
        tracing::warn!(created, "movement breach: new red-zone breach incidents");
    }
    Ok(())
}

/// Best-effort subject correlation: a vehicle entry event sharing the breach's (camera, track id)
/// within ±5 min. Returns (subject_type, plate). Person breaches stay unknown (no plate, no embedding).
async fn correlate(
    pool: &SqlitePool,
    ev: &ZoneEv,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let Some(track) = ev.track_id.as_deref() else {
        return Ok((Some("unknown".into()), None));
    };
    let lo = ev.timestamp - TimeDelta::try_minutes(5).unwrap();
    let hi = ev.timestamp + TimeDelta::try_minutes(5).unwrap();
    let plate: Option<String> = sqlx::query_scalar(
        "SELECT plate FROM entry_events
          WHERE camera_id = ? AND track_id = ? AND plate IS NOT NULL
            AND timestamp >= ? AND timestamp <= ?
          ORDER BY timestamp ASC LIMIT 1",
    )
    .bind(&ev.camera_id)
    .bind(track)
    .bind(lo)
    .bind(hi)
    .fetch_optional(pool)
    .await?;
    Ok(match plate {
        Some(p) => (Some("vehicle".into()), Some(p)),
        None => (Some("unknown".into()), None),
    })
}
