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
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct QueryPlan {
    pub from: Option<String>,
    pub to: Option<String>,
    /// Time-of-day filter, e.g. "after 6pm" ⇒ hour_min=18.
    ///
    /// Read in [`QueryPlan::tz`], NOT in UTC — see that field. "after 6pm" meaning 18:00 UTC is how
    /// a search at a Malaysian site quietly answers about 2am.
    pub hour_min: Option<u32>,
    pub hour_max: Option<u32>,
    /// The IANA zone `hour_min`/`hour_max` and relative dates are read in (#125).
    ///
    /// ALWAYS SERIALIZED, even when absent, because this plan is written to `search_log` as the
    /// accountability record for identity-bearing searches. A logged plan with no `tz` field is one
    /// written before this existed and means "UTC, unlabelled"; a plan with `tz: null` means the
    /// zone was resolved at execution time and is echoed in the response. Without the field, rows
    /// from before and after this change look identical and mean different things.
    pub tz: Option<String>,
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
    /// Zone-id scope — SEMANTIC route only (issue #77): recorded here so the search_log snapshot
    /// captures it. The structured executor ignores it and `planner::sanitize` clears it, so a
    /// structured/NL caller can never set it expecting filtering that doesn't happen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
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

/// SQL predicate confining a source fetch to `plan.cameras`, or `None` when the plan names none.
///
/// The camera filter has to run in SQL rather than only in the Rust field-filter below, because every
/// source is fetched `ORDER BY <ts> DESC LIMIT fetch_cap`. A post-fetch filter bounds ROWS EXAMINED,
/// not rows returned: newer rows from cameras the caller did not name consume the page, and the
/// caller's own older in-window matches never reach the filter at all. These routes carry no offset
/// or cursor, so past the cap those rows are unreachable by any query the API accepts — and for a
/// camera-scoped credential, whose `cameras` list is its SCOPE (`routes::confine_requested_cameras`),
/// that is the scope layer denying the caller its own data.
///
/// It also makes `truncated` honest. Raised from the UNFILTERED row count, `truncated: true` beside
/// `count: 0` reported the FLEET's in-window volume — a bit about cameras the caller does not hold,
/// differencable over a swept window into the fleet's event-rate profile. Computed from the confined
/// fetch it says what it claims to say: the CALLER'S OWN matches may be incomplete.
///
/// `column` is a compile-time constant at every call site (never caller input), and the ids are
/// bound. `IN (…)` never matches NULL, which is exactly what the Rust filter did
/// (`camera_id … .unwrap_or(false)`), so a camera-less row stays excluded rather than newly appearing.
/// An empty list still means "every camera" for every caller, so an unconfined query is unchanged.
///
/// Returns the predicate together with the ids to bind, and callers MUST bind from the RETURNED
/// vector rather than from `plan.cameras`: the two differ (deduped, capped), and binding from the
/// input would desync the parameter count from the placeholders.
///
/// `plan.cameras` is caller-supplied and `planner::sanitize` does not bound it, so a 100k-element
/// list would otherwise become 100k SQL variables and fail the statement outright — turning a slow
/// request into a 500. Over [`CAMERA_PRED_MAX`] DISTINCT ids the pushdown is skipped and the Rust
/// filter below carries the whole load, exactly as it did before: the ANSWER is identical either way
/// (the filter is applied twice by design), only the page-eviction and `truncated` honesty are lost —
/// and no real deployment, let alone a camera scope, names a thousand cameras.
fn camera_pred(plan: &QueryPlan, column: &str) -> Option<(String, Vec<String>)> {
    if plan.cameras.is_empty() {
        return None;
    }
    let mut ids: Vec<String> = plan.cameras.clone();
    // `IN (…)` is a set test and so is the Rust filter's `any(==)`, so deduping cannot change the
    // result set — it only stops a repeated id from inflating the bind count.
    ids.sort();
    ids.dedup();
    if ids.len() > CAMERA_PRED_MAX {
        return None;
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    Some((format!(" AND {column} IN ({placeholders})"), ids))
}

/// Distinct camera ids above which the pushdown is skipped — see [`camera_pred`]. Far above any real
/// fleet, far below SQLite's variable ceiling.
const CAMERA_PRED_MAX: usize = 1000;

/// The effective [from, to) the executor will actually scan, after applying the default window. Shown
/// in the proof so the caller sees the real window even when the plan left it unset.
pub fn window(plan: &QueryPlan) -> (DateTime<Utc>, DateTime<Utc>) {
    let now = Utc::now();
    // Default window: last 7 days, so an unbounded query never scans the whole history.
    let from = parse_ts(&plan.from).unwrap_or(now - TimeDelta::try_days(7).unwrap());
    let to = parse_ts(&plan.to).unwrap_or(now + TimeDelta::try_minutes(1).unwrap());
    (from, to)
}

/// Result of executing a plan: the matching hits (newest first, capped) plus whether the per-source
/// fetch hit its cap. `truncated == true` means a source returned as many rows as `fetch_cap`, so
/// older in-window rows were cut BEFORE the Rust field-filters ran — the field-filtered result may
/// therefore be an undercount, and the proof layer must not claim completeness.
pub struct ExecOutcome {
    pub hits: Vec<SearchHit>,
    pub truncated: bool,
}

/// Execute the plan deterministically and return the matching hits (newest first, capped).
/// Execute a plan, reading its hour filter in `tz`.
///
/// `tz` is resolved by the caller (explicit plan field, then the camera's site, then the box
/// default, then UTC) so that this stays a pure function of the plan it is handed.
pub async fn execute_in(
    pool: &SqlitePool,
    plan: &QueryPlan,
    max: i64,
    tz: chrono_tz::Tz,
) -> sqlx::Result<ExecOutcome> {
    execute_inner(pool, plan, max, tz).await
}

/// Backwards-compatible entry point: UTC, which is what every caller meant before #125.
pub async fn execute(pool: &SqlitePool, plan: &QueryPlan, max: i64) -> sqlx::Result<ExecOutcome> {
    execute_inner(pool, plan, max, chrono_tz::Tz::UTC).await
}

async fn execute_inner(
    pool: &SqlitePool,
    plan: &QueryPlan,
    max: i64,
    tz: chrono_tz::Tz,
) -> sqlx::Result<ExecOutcome> {
    let (from, to) = window(plan);
    let fetch_cap = (max * 5).clamp(100, 20_000);

    let mut hits: Vec<SearchHit> = Vec::new();
    // Set when any source returns a full page: the time-window fetch cut older rows before field
    // filtering, so the answer below may omit in-window matches. Surfaced through the proof layer.
    let mut truncated = false;

    if want(plan, "entry") {
        let cams = camera_pred(plan, "camera_id");
        let sql = format!(
            "SELECT id, timestamp, camera_id, event_type, plate, subject, auth_status, evidence
               FROM entry_events_read WHERE timestamp >= ? AND timestamp <= ?{}
              ORDER BY timestamp DESC LIMIT ?",
            cams.as_ref().map(|(p, _)| p.as_str()).unwrap_or("")
        );
        let mut q = sqlx::query_as::<_, EntryRow>(&sql).bind(from).bind(to);
        for c in cams.iter().flat_map(|(_, ids)| ids) {
            q = q.bind(c);
        }
        let rows: Vec<EntryRow> = q.bind(fetch_cap).fetch_all(pool).await?;
        truncated |= rows.len() as i64 >= fetch_cap;
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
        let cams = camera_pred(plan, "ze.camera_id");
        let sql = format!(
            "SELECT ze.id, ze.timestamp, ze.camera_id, ze.event_type, ze.label, ze.zone_name,
                    z.kind AS kind, ze.evidence_path
               FROM zone_events ze LEFT JOIN zones z ON z.id = ze.zone_id
              WHERE ze.timestamp >= ? AND ze.timestamp <= ?{}
              ORDER BY ze.timestamp DESC LIMIT ?",
            cams.as_ref().map(|(p, _)| p.as_str()).unwrap_or("")
        );
        let mut q = sqlx::query_as::<_, ZoneRow>(&sql).bind(from).bind(to);
        for c in cams.iter().flat_map(|(_, ids)| ids) {
            q = q.bind(c);
        }
        let rows: Vec<ZoneRow> = q.bind(fetch_cap).fetch_all(pool).await?;
        truncated |= rows.len() as i64 >= fetch_cap;
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
        let cams = camera_pred(plan, "camera_id");
        let sql = format!(
            "SELECT id, created_at, camera_id, rule, subject_type, subject, zone_name, severity, evidence_path
               FROM breach_alerts_read WHERE created_at >= ? AND created_at <= ?{}
              ORDER BY created_at DESC LIMIT ?",
            cams.as_ref().map(|(p, _)| p.as_str()).unwrap_or("")
        );
        let mut q = sqlx::query_as::<_, BreachRow>(&sql).bind(from).bind(to);
        for c in cams.iter().flat_map(|(_, ids)| ids) {
            q = q.bind(c);
        }
        let rows: Vec<BreachRow> = q.bind(fetch_cap).fetch_all(pool).await?;
        truncated |= rows.len() as i64 >= fetch_cap;
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
        // THE HOUR IS READ IN THE RESOLVED ZONE, not in UTC (#125). "after 6pm" at a site eight
        // hours ahead used to select events from 2am local — a syntactically valid search returning
        // convincing footage of the wrong part of the night.
        let hr = h.timestamp.with_timezone(&tz).hour();
        match (plan.hour_min, plan.hour_max) {
            // Overnight window (e.g. 22:00–06:00): min > max means a wraparound union, not an empty set.
            (Some(lo), Some(hi)) if lo > hi => {
                if !(hr >= lo || hr <= hi) {
                    return false;
                }
            }
            _ => {
                if let Some(lo) = plan.hour_min {
                    if hr < lo {
                        return false;
                    }
                }
                if let Some(hi) = plan.hour_max {
                    if hr > hi {
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
    if truncated {
        tracing::warn!(
            fetch_cap,
            "search: a source hit the fetch cap; older in-window matches may be omitted (result flagged non-exhaustive)"
        );
    }
    Ok(ExecOutcome { hits, truncated })
}

fn parse_ts(s: &Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref().and_then(heldar_kernel::util::parse_rfc3339)
}

/// Build a quick aggregate breakdown (counts by source + by camera) over the hits — for the proof.
pub fn breakdown(hits: &[SearchHit]) -> Value {
    breakdown_in(hits, chrono_tz::Tz::UTC)
}

/// [`breakdown`] with the zone the day buckets are keyed in (#125).
///
/// A calendar day is a wall-clock notion, so a proof histogram keyed in UTC contradicts the query it
/// is proving: a one-local-day search at a +08:00 site produced a `by_day` spanning two dates, which
/// is exactly the kind of quiet disagreement the proof layer exists to rule out.
pub fn breakdown_in(hits: &[SearchHit], tz: chrono_tz::Tz) -> Value {
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
        let local = h.timestamp.with_timezone(&tz);
        let day = format!(
            "{:04}-{:02}-{:02}",
            local.year(),
            local.month(),
            local.day()
        );
        *by_day.entry(day.clone()).or_insert(json!(0)) =
            json!(by_day.get(&day).and_then(|v| v.as_i64()).unwrap_or(0) + 1);
    }
    json!({ "by_source": by_source, "by_day": by_day })
}

#[cfg(test)]
mod tz_filter_tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Tz;

    fn hit(at: &str) -> SearchHit {
        SearchHit {
            source: "entry".into(),
            id: "e1".into(),
            timestamp: chrono::DateTime::parse_from_rfc3339(at)
                .unwrap()
                .with_timezone(&Utc),
            camera_id: Some("cam_a".into()),
            kind: "entry".into(),
            plate: None,
            subject: serde_json::json!({}),
            auth_status: None,
            zone: None,
            zone_kind: None,
            evidence_path: None,
            claim_level: "event".into(),
        }
    }

    /// Apply only the hour filter, the way `execute_inner` does.
    fn passes(h: &SearchHit, lo: Option<u32>, hi: Option<u32>, tz: Tz) -> bool {
        let hr = h.timestamp.with_timezone(&tz).hour();
        match (lo, hi) {
            (Some(lo), Some(hi)) if lo > hi => hr >= lo || hr <= hi,
            _ => lo.map(|l| hr >= l).unwrap_or(true) && hi.map(|x| hr <= x).unwrap_or(true),
        }
    }

    /// THE EIGHT HOURS. An operator in Kuala Lumpur asking for "after 6pm" means their evening.
    /// Read in UTC, the same filter selects the small hours of the following morning — a valid
    /// search returning convincing footage of the wrong part of the night.
    #[test]
    fn after_6pm_means_the_sites_evening_not_utcs() {
        // 20:00 in Kuala Lumpur is 12:00 UTC.
        let evening_in_kl = hit("2026-06-01T12:00:00Z");
        assert!(
            passes(&evening_in_kl, Some(18), None, Tz::Asia__Kuala_Lumpur),
            "20:00 local is after 6pm"
        );
        assert!(
            !passes(&evening_in_kl, Some(18), None, Tz::UTC),
            "the same instant is 12:00 UTC and must NOT match — if it does, the zone is not \
             reaching the comparison"
        );

        // And the converse: an event that IS after 18:00 UTC is 02:00 the next day in KL.
        let late_utc = hit("2026-06-01T19:00:00Z");
        assert!(passes(&late_utc, Some(18), None, Tz::UTC));
        assert!(
            !passes(&late_utc, Some(18), None, Tz::Asia__Kuala_Lumpur),
            "03:00 the next morning in KL is not the evening the operator asked about"
        );
    }

    /// An overnight filter (22:00–06:00) must wrap the SITE's midnight.
    #[test]
    fn an_overnight_hour_filter_wraps_the_sites_midnight() {
        // 23:00 KL = 15:00Z.
        let h = hit("2026-06-01T15:00:00Z");
        assert!(passes(&h, Some(22), Some(6), Tz::Asia__Kuala_Lumpur));
        assert!(!passes(&h, Some(22), Some(6), Tz::UTC));
    }

    /// A DST zone: the same wall-clock hour is a different UTC instant in summer and winter, which
    /// is the whole reason a fixed offset would not do.
    #[test]
    fn a_dst_zone_shifts_the_same_local_hour_across_the_year() {
        // 19:00 London is 18:00Z in summer (BST) and 19:00Z in winter (GMT).
        let summer = hit("2026-06-01T18:00:00Z");
        let winter = hit("2026-12-01T19:00:00Z");
        for h in [&summer, &winter] {
            assert_eq!(
                h.timestamp.with_timezone(&Tz::Europe__London).hour(),
                19,
                "both are 19:00 local, half a year apart"
            );
            assert!(passes(h, Some(18), None, Tz::Europe__London));
        }
        // Read in UTC they are different hours, so a fixed-offset shortcut would answer differently
        // in June and December for the same question.
        assert_ne!(summer.timestamp.hour(), winter.timestamp.hour());
    }

    /// END-TO-END, and the reason it exists: every test above reimplements the filter in `passes`,
    /// so they prove the LOGIC is zone-aware and prove nothing about whether `execute_in` uses the
    /// zone it is handed. Changing `execute_inner` back to `h.timestamp.hour()` leaves them green.
    /// This one goes through the real executor, so it fails.
    #[tokio::test]
    async fn execute_in_actually_reads_the_hour_in_the_zone_it_is_given() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        heldar_kernel::db::run_migrations(&pool).await.unwrap();

        // 12:00Z on 2026-06-01 — 20:00 in Kuala Lumpur, i.e. after 6pm THERE and not here.
        let at = chrono::DateTime::parse_from_rfc3339("2026-06-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        sqlx::query(
            "INSERT INTO zone_events
               (id, camera_id, zone_id, zone_name, event_type, label, timestamp, created_at)
             VALUES ('ze_1','cam_a','z1','Gate','enter','person',?,?)",
        )
        .bind(at)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();

        let plan = QueryPlan {
            from: Some("2026-05-01T00:00:00Z".into()),
            to: Some("2026-07-01T00:00:00Z".into()),
            hour_min: Some(18),
            sources: vec!["zone".into()],
            ..Default::default()
        };

        let kl = execute_in(&pool, &plan, 100, Tz::Asia__Kuala_Lumpur)
            .await
            .unwrap();
        assert_eq!(
            kl.hits.len(),
            1,
            "20:00 in Kuala Lumpur is after 6pm and the event must be returned"
        );

        let utc = execute_in(&pool, &plan, 100, Tz::UTC).await.unwrap();
        assert_eq!(
            utc.hits.len(),
            0,
            "the SAME event read in UTC is 12:00 and must NOT match. If it does, execute_in is \
             ignoring the zone it was handed and every other test in this module is proving \
             nothing about the code path that runs."
        );
    }

    /// The branch's central safety promise for search, and it was tested through the LOCAL `passes`
    /// helper — so it stayed green when the executor was reverted, i.e. it proved nothing about
    /// production. It goes through `execute_in` now.
    #[tokio::test]
    async fn utc_stays_utc_when_nothing_is_configured() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        heldar_kernel::db::run_migrations(&pool).await.unwrap();
        // 19:00Z — after 6pm in UTC, and 03:00 the next day in Kuala Lumpur.
        let at = chrono::DateTime::parse_from_rfc3339("2026-06-01T19:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        sqlx::query(
            "INSERT INTO zone_events
               (id, camera_id, zone_id, zone_name, event_type, label, timestamp, created_at)
             VALUES ('ze_u','cam_a','z1','Gate','enter','person',?,?)",
        )
        .bind(at)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();

        let plan = QueryPlan {
            from: Some("2026-05-01T00:00:00Z".into()),
            to: Some("2026-07-01T00:00:00Z".into()),
            hour_min: Some(18),
            sources: vec!["zone".into()],
            ..Default::default()
        };
        let out = execute_in(&pool, &plan, 100, Tz::UTC).await.unwrap();
        assert_eq!(
            out.hits.len(),
            1,
            "an unconfigured box must answer exactly as it always has: 19:00Z is after 6pm UTC"
        );
        let _ = Utc.timestamp_opt(0, 0);
    }
}

#[cfg(test)]
mod breakdown_tz_tests {
    use super::*;
    use chrono_tz::Tz;

    /// A one-local-day search must not produce a proof histogram spanning two dates. The proof layer
    /// exists to rule out exactly this sort of quiet disagreement with the query it describes.
    #[test]
    fn day_buckets_are_keyed_in_the_querys_own_zone() {
        let hits: Vec<SearchHit> = [
            "2026-06-01T16:30:00Z", // 2026-06-02 00:30 +08
            "2026-06-02T03:00:00Z", // 2026-06-02 11:00 +08
            "2026-06-02T15:30:00Z", // 2026-06-02 23:30 +08
        ]
        .iter()
        .map(|t| SearchHit {
            source: "zone".into(),
            id: t.to_string(),
            timestamp: chrono::DateTime::parse_from_rfc3339(t)
                .unwrap()
                .with_timezone(&Utc),
            camera_id: Some("cam_a".into()),
            kind: "enter".into(),
            plate: None,
            subject: json!({}),
            auth_status: None,
            zone: None,
            zone_kind: None,
            evidence_path: None,
            claim_level: "event".into(),
        })
        .collect();

        let kl = breakdown_in(&hits, Tz::Asia__Kuala_Lumpur);
        assert_eq!(
            kl["by_day"],
            json!({"2026-06-02": 3}),
            "all three are the same calendar day in Kuala Lumpur"
        );

        let utc = breakdown_in(&hits, Tz::UTC);
        assert_eq!(
            utc["by_day"],
            json!({"2026-06-01": 1, "2026-06-02": 2}),
            "and genuinely two days in UTC — if these agree, the zone is not reaching the buckets"
        );
    }
}
