//! The structured query PLAN and its deterministic executor. The plan is the only thing the NL layer
//! produces; the executor runs it against the kernel's stored facts and returns the rows — the answer
//! is always the data, never model output.

use chrono::{DateTime, Datelike, TimeDelta, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::types::Json;
use sqlx::{FromRow, SqlitePool};

/// A structured, executable query plan. Produced by the planner (rules or LLM), shown back to the
/// caller, and executed deterministically. All fields optional ⇒ "everything in the default window".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryPlan {
    pub from: Option<String>,
    pub to: Option<String>,
    /// Time-of-day filter, e.g. "after 6pm" ⇒ hour_min=18 (UTC hour of the event timestamp).
    pub hour_min: Option<u32>,
    pub hour_max: Option<u32>,
    #[serde(default)]
    pub cameras: Vec<String>,
    /// Which fact sources to search: any of entry | zone | breach (empty ⇒ all).
    #[serde(default)]
    pub sources: Vec<String>,
    pub plate: Option<String>,
    pub color: Option<String>,
    pub vehicle_type: Option<String>,
    /// vehicle | person
    pub subject_type: Option<String>,
    #[serde(default)]
    pub auth_status: Vec<String>,
    pub event_type: Option<String>,
    pub zone_kind: Option<String>,
    /// Free-text substring matched across plate / zone / kind.
    pub text: Option<String>,
    pub limit: Option<i64>,
}

/// A unified search result, normalized across the fact tables. Carries its claim level + evidence.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub source: String,
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub camera_id: Option<String>,
    pub kind: String,
    pub plate: Option<String>,
    pub subject: Value,
    pub auth_status: Option<String>,
    pub zone: Option<String>,
    pub zone_kind: Option<String>,
    pub evidence_path: Option<String>,
    pub claim_level: String,
}

#[derive(FromRow)]
struct EntryRow {
    id: String,
    timestamp: DateTime<Utc>,
    camera_id: Option<String>,
    event_type: String,
    plate: Option<String>,
    subject: Json<Value>,
    auth_status: String,
    evidence: Json<Value>,
}
#[derive(FromRow)]
struct ZoneRow {
    id: String,
    timestamp: DateTime<Utc>,
    camera_id: String,
    event_type: String,
    label: Option<String>,
    zone_name: String,
    kind: Option<String>,
    evidence_path: Option<String>,
}
#[derive(FromRow)]
struct BreachRow {
    id: String,
    created_at: DateTime<Utc>,
    camera_id: Option<String>,
    rule: String,
    subject_type: Option<String>,
    subject: Option<String>,
    zone_name: Option<String>,
    severity: String,
    evidence_path: Option<String>,
}

fn want(plan: &QueryPlan, src: &str) -> bool {
    plan.sources.is_empty() || plan.sources.iter().any(|s| s == src)
}

/// The effective [from, to) the executor will actually scan, after applying the default window. Shown
/// in the proof so the caller sees the real window even when the plan left it unset.
pub fn window(plan: &QueryPlan) -> (DateTime<Utc>, DateTime<Utc>) {
    let now = Utc::now();
    // Default window: last 7 days, so an unbounded query never scans the whole history.
    let from = parse_ts(&plan.from).unwrap_or(now - TimeDelta::try_days(7).unwrap());
    let to = parse_ts(&plan.to).unwrap_or(now + TimeDelta::try_minutes(1).unwrap());
    (from, to)
}

/// Execute the plan deterministically and return the matching hits (newest first, capped).
pub async fn execute(
    pool: &SqlitePool,
    plan: &QueryPlan,
    max: i64,
) -> sqlx::Result<Vec<SearchHit>> {
    let (from, to) = window(plan);
    let fetch_cap = (max * 5).clamp(100, 20_000);

    let mut hits: Vec<SearchHit> = Vec::new();

    if want(plan, "entry") {
        let rows: Vec<EntryRow> = sqlx::query_as(
            "SELECT id, timestamp, camera_id, event_type, plate, subject, auth_status, evidence
               FROM entry_events WHERE timestamp >= ? AND timestamp <= ?
              ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(from)
        .bind(to)
        .bind(fetch_cap)
        .fetch_all(pool)
        .await?;
        for r in rows {
            let ev_path = r
                .evidence
                .0
                .get("snapshot_path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            hits.push(SearchHit {
                source: "entry".into(),
                id: r.id,
                timestamp: r.timestamp,
                camera_id: r.camera_id,
                kind: r.event_type,
                plate: r.plate,
                subject: r.subject.0,
                auth_status: Some(r.auth_status),
                zone: None,
                zone_kind: None,
                evidence_path: ev_path,
                claim_level: "event".into(),
            });
        }
    }
    if want(plan, "zone") {
        let rows: Vec<ZoneRow> = sqlx::query_as(
            "SELECT ze.id, ze.timestamp, ze.camera_id, ze.event_type, ze.label, ze.zone_name,
                    z.kind AS kind, ze.evidence_path
               FROM zone_events ze LEFT JOIN zones z ON z.id = ze.zone_id
              WHERE ze.timestamp >= ? AND ze.timestamp <= ?
              ORDER BY ze.timestamp DESC LIMIT ?",
        )
        .bind(from)
        .bind(to)
        .bind(fetch_cap)
        .fetch_all(pool)
        .await?;
        for r in rows {
            hits.push(SearchHit {
                source: "zone".into(),
                id: r.id,
                timestamp: r.timestamp,
                camera_id: Some(r.camera_id),
                kind: r.event_type,
                plate: None,
                subject: json!({ "label": r.label }),
                auth_status: None,
                zone: Some(r.zone_name),
                zone_kind: r.kind,
                evidence_path: r.evidence_path,
                claim_level: "event".into(),
            });
        }
    }
    if want(plan, "breach") {
        let rows: Vec<BreachRow> = sqlx::query_as(
            "SELECT id, created_at, camera_id, rule, subject_type, subject, zone_name, severity, evidence_path
               FROM breach_alerts WHERE created_at >= ? AND created_at <= ?
              ORDER BY created_at DESC LIMIT ?",
        )
        .bind(from)
        .bind(to)
        .bind(fetch_cap)
        .fetch_all(pool)
        .await?;
        for r in rows {
            hits.push(SearchHit {
                source: "breach".into(),
                id: r.id,
                timestamp: r.created_at,
                camera_id: r.camera_id,
                kind: r.rule,
                plate: r.subject.clone(),
                subject: json!({ "subject_type": r.subject_type, "severity": r.severity }),
                auth_status: None,
                zone: r.zone_name,
                zone_kind: None,
                evidence_path: r.evidence_path,
                claim_level: "event".into(),
            });
        }
    }

    // Apply the remaining (field) filters deterministically in Rust.
    let camset = &plan.cameras;
    hits.retain(|h| {
        if !camset.is_empty()
            && !h
                .camera_id
                .as_deref()
                .map(|c| camset.iter().any(|x| x == c))
                .unwrap_or(false)
        {
            return false;
        }
        match (plan.hour_min, plan.hour_max) {
            // Overnight window (e.g. 22:00–06:00): min > max means a wraparound union, not an empty set.
            (Some(lo), Some(hi)) if lo > hi => {
                let hr = h.timestamp.hour();
                if !(hr >= lo || hr <= hi) {
                    return false;
                }
            }
            _ => {
                if let Some(lo) = plan.hour_min {
                    if h.timestamp.hour() < lo {
                        return false;
                    }
                }
                if let Some(hi) = plan.hour_max {
                    if h.timestamp.hour() > hi {
                        return false;
                    }
                }
            }
        }
        if let Some(p) = &plan.plate {
            if h.plate.as_deref() != Some(p.as_str()) {
                return false;
            }
        }
        if let Some(c) = &plan.color {
            if h.subject
                .get("color")
                .and_then(|v| v.as_str())
                .map(|x| !x.eq_ignore_ascii_case(c))
                .unwrap_or(true)
            {
                return false;
            }
        }
        if let Some(vt) = &plan.vehicle_type {
            if h.subject
                .get("vehicle_type")
                .and_then(|v| v.as_str())
                .map(|x| !x.eq_ignore_ascii_case(vt))
                .unwrap_or(true)
            {
                return false;
            }
        }
        if let Some(stp) = &plan.subject_type {
            let hit_type = h
                .subject
                .get("type")
                .or_else(|| h.subject.get("subject_type"))
                .and_then(|v| v.as_str());
            // entry vehicle events have subject.type == "vehicle"; person filter mainly hits zone/label.
            match stp.as_str() {
                "vehicle" => {
                    if !(hit_type == Some("vehicle") || h.plate.is_some()) {
                        return false;
                    }
                }
                "person" => {
                    let is_person = hit_type == Some("person")
                        || h.subject.get("label").and_then(|v| v.as_str()) == Some("person");
                    if !is_person {
                        return false;
                    }
                }
                _ => {}
            }
        }
        if !plan.auth_status.is_empty() {
            match &h.auth_status {
                Some(a) if plan.auth_status.iter().any(|x| x == a) => {}
                _ => return false,
            }
        }
        if let Some(et) = &plan.event_type {
            if !h.kind.eq_ignore_ascii_case(et) {
                return false;
            }
        }
        if let Some(zk) = &plan.zone_kind {
            if h.zone_kind
                .as_deref()
                .map(|k| !k.eq_ignore_ascii_case(zk))
                .unwrap_or(true)
            {
                return false;
            }
        }
        if let Some(t) = &plan.text {
            let tl = t.to_lowercase();
            let hay = format!(
                "{} {} {} {}",
                h.plate.clone().unwrap_or_default(),
                h.zone.clone().unwrap_or_default(),
                h.kind,
                h.subject
            )
            .to_lowercase();
            if !hay.contains(&tl) {
                return false;
            }
        }
        true
    });

    hits.sort_by_key(|h| std::cmp::Reverse(h.timestamp));
    let limit = plan.limit.unwrap_or(max).clamp(1, max) as usize;
    hits.truncate(limit);
    Ok(hits)
}

fn parse_ts(s: &Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref().and_then(heldar_kernel::util::parse_rfc3339)
}

/// Build a quick aggregate breakdown (counts by source + by camera) over the hits — for the proof.
pub fn breakdown(hits: &[SearchHit]) -> Value {
    let mut by_source = serde_json::Map::new();
    let mut by_day = serde_json::Map::new();
    for h in hits {
        *by_source.entry(h.source.clone()).or_insert(json!(0)) = json!(
            by_source
                .get(&h.source)
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + 1
        );
        let day = format!(
            "{:04}-{:02}-{:02}",
            h.timestamp.year(),
            h.timestamp.month(),
            h.timestamp.day()
        );
        *by_day.entry(day.clone()).or_insert(json!(0)) =
            json!(by_day.get(&day).and_then(|v| v.as_i64()).unwrap_or(0) + 1);
    }
    json!({ "by_source": by_source, "by_day": by_day })
}
