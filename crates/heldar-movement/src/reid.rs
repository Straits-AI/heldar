//! Multi-signal ReID candidate engine.
//!
//! VEHICLE ReID is anchored on the PLATE (resolved by the access-control app into `entry_events`): the proposer
//! finds the same normalized plate appearing on two topology-linked cameras within a plausible transit
//! window, and scores the link by fusing plate-exactness + transit-time plausibility + vehicle
//! attribute agreement (colour/type). The plate is the dominant signal — this is the "multi-signal,
//! never pure visual embedding" stance. Each link is a *candidate* for human review, never an identity.
//!
//! PERSON ReID has no plate and no appearance embedding here, so it is exposed ONLY as a low-confidence,
//! on-demand search over topology + time (see `search_person`) — never auto-proposed.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Value};
use sqlx::types::Json;
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::config::MovementConfig;

pub async fn run(pool: SqlitePool, cfg: Arc<MovementConfig>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.engine_interval_s));
    loop {
        tick.tick().await;
        if let Err(e) = propose_vehicle_candidates(&pool, &cfg).await {
            tracing::error!(error = %e, "movement reid: candidate proposal failed");
        }
        if let Err(e) = prune(&pool, &cfg).await {
            tracing::error!(error = %e, "movement reid: prune failed");
        }
    }
}

/// Run the proposer once on demand (ops / tests).
pub async fn run_once(pool: &SqlitePool, cfg: &MovementConfig) -> anyhow::Result<()> {
    propose_vehicle_candidates(pool, cfg).await
}

#[derive(FromRow)]
struct PairRow {
    a_id: String,
    a_cam: String,
    a_ts: DateTime<Utc>,
    plate: String,
    a_subj: Json<Value>,
    b_id: String,
    b_cam: String,
    b_ts: DateTime<Utc>,
    b_subj: Json<Value>,
    transit: i64,
}

fn attr<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty())
}

/// Fuse signals into a 0..1 score; returns (score, signals-json). Plate match is the anchor.
fn score_pair(gap: f64, transit: f64, a: &Value, b: &Value) -> (f64, Value) {
    let mut score: f64 = 0.8; // plate-exact anchor (OCR can still err, so not 1.0)
                              // Transit-time plausibility: closer to the expected transit, higher.
    let transit_signal = if transit > 0.0 && gap <= transit {
        0.1
    } else if transit > 0.0 && gap <= transit * 2.0 {
        0.05
    } else {
        0.0
    };
    score += transit_signal;
    let color_match = match (attr(a, "color"), attr(b, "color")) {
        (Some(x), Some(y)) => Some(x.eq_ignore_ascii_case(y)),
        _ => None,
    };
    let type_match = match (attr(a, "vehicle_type"), attr(b, "vehicle_type")) {
        (Some(x), Some(y)) => Some(x.eq_ignore_ascii_case(y)),
        _ => None,
    };
    if color_match == Some(true) {
        score += 0.05;
    } else if color_match == Some(false) {
        score -= 0.1; // attribute CONFLICT lowers confidence (maybe plate misread/cloned)
    }
    if type_match == Some(true) {
        score += 0.05;
    } else if type_match == Some(false) {
        score -= 0.1;
    }
    let score = score.clamp(0.0, 1.0);
    (
        score,
        json!({ "plate_exact": true, "transit_seconds": gap, "expected_transit": transit,
                "color_match": color_match, "type_match": type_match }),
    )
}

async fn propose_vehicle_candidates(pool: &SqlitePool, cfg: &MovementConfig) -> anyhow::Result<()> {
    let scan_start = Utc::now() - TimeDelta::try_seconds(cfg.scan_window_s).unwrap();
    // Same plate on two topology-linked cameras, b later than a; only recent `b` (this scan window).
    let pairs: Vec<PairRow> = sqlx::query_as(
        "SELECT a.id AS a_id, a.camera_id AS a_cam, a.timestamp AS a_ts, a.plate AS plate,
                a.subject AS a_subj,
                b.id AS b_id, b.camera_id AS b_cam, b.timestamp AS b_ts, b.subject AS b_subj,
                l.transit_seconds AS transit
           FROM entry_events a
           JOIN entry_events b
             ON b.plate = a.plate AND b.camera_id != a.camera_id AND b.timestamp > a.timestamp
           JOIN camera_links l
             ON (l.from_camera = a.camera_id AND l.to_camera = b.camera_id)
             OR (l.bidirectional = 1 AND l.from_camera = b.camera_id AND l.to_camera = a.camera_id)
          WHERE a.plate IS NOT NULL AND a.plate != '' AND b.timestamp >= ?
          ORDER BY b.timestamp DESC LIMIT 1000",
    )
    .bind(scan_start)
    .fetch_all(pool)
    .await?;

    let mut proposed = 0u64;
    for p in pairs {
        let gap = (p.b_ts - p.a_ts).num_seconds() as f64;
        // Reject implausible gaps (too fast to physically transit, or far beyond the link window).
        if gap < 1.0 || gap > (p.transit as f64) * 4.0 {
            continue;
        }
        let (score, signals) = score_pair(gap, p.transit as f64, &p.a_subj.0, &p.b_subj.0);
        if score < cfg.min_candidate_score {
            continue;
        }
        // Don't clobber a human-reviewed candidate (UNIQUE on subject_type+from_ref+to_ref).
        let res = sqlx::query(
            "INSERT INTO movement_candidates
               (id, subject_type, anchor, from_camera, from_ref, from_time, to_camera, to_ref, to_time,
                transit_seconds, score, signals, status, created_at)
             VALUES (?, 'vehicle', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)
             ON CONFLICT(subject_type, from_ref, to_ref) DO NOTHING",
        )
        .bind(format!("cand_{}", Uuid::new_v4().simple()))
        .bind(&p.plate)
        .bind(&p.a_cam)
        .bind(&p.a_id)
        .bind(p.a_ts)
        .bind(&p.b_cam)
        .bind(&p.b_id)
        .bind(p.b_ts)
        .bind(gap)
        .bind(score)
        .bind(Json(&signals))
        .bind(Utc::now())
        .execute(pool)
        .await?;
        proposed += res.rows_affected();
    }
    if proposed > 0 {
        tracing::info!(proposed, "movement reid: proposed vehicle candidates");
    }
    Ok(())
}

async fn prune(pool: &SqlitePool, cfg: &MovementConfig) -> anyhow::Result<()> {
    let cutoff = Utc::now() - TimeDelta::try_days(cfg.retention_days.max(1)).unwrap();
    sqlx::query("DELETE FROM movement_candidates WHERE created_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM breach_alerts WHERE created_at < ? AND status = 'resolved'")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(())
}

/// A single appearance of a subject (for a trail).
#[derive(FromRow, serde::Serialize)]
pub struct Appearance {
    pub event_id: String,
    pub camera_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    pub auth_status: String,
    pub direction: String,
}

/// All appearances of a plate across cameras, time-ordered = its movement trail. Caller MUST audit.
/// Bounded to the most recent [`TRAIL_MAX`] appearances: a heavily-travelled plate could otherwise
/// match tens of thousands of rows and load them all into memory (OOM/latency). We take the newest
/// rows (`DESC LIMIT`) then return them oldest-first for the trail.
pub async fn trail_for_plate(pool: &SqlitePool, plate_norm: &str) -> sqlx::Result<Vec<Appearance>> {
    const TRAIL_MAX: i64 = 10_000;
    let mut rows = sqlx::query_as::<_, Appearance>(
        "SELECT id AS event_id, camera_id, timestamp, event_type, auth_status, direction
           FROM entry_events WHERE plate = ? ORDER BY timestamp DESC LIMIT ?",
    )
    .bind(plate_norm)
    .bind(TRAIL_MAX)
    .fetch_all(pool)
    .await?;
    rows.reverse(); // newest-first capped → oldest-first trail
    Ok(rows)
}
