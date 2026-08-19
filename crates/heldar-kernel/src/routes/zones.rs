//! Zone CRUD + zone-events query (Stage 3).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{Zone, ZoneCreate, ZoneEvent, ZoneUpdate};
use crate::state::{AppState, CameraOwned};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/cameras/{id}/zones",
            get(list_zones).post(create_zone),
        )
        .route(
            "/api/v1/zones/{zone_id}",
            axum::routing::patch(update_zone).delete(delete_zone),
        )
        .route("/api/v1/cameras/{id}/zone-events", get(list_zone_events))
        .route(
            "/api/v1/cameras/{id}/zone-events/aggregates",
            get(zone_event_aggregates),
        )
        .route("/api/v1/cameras/{id}/zones/occupancy", get(zone_occupancy))
}

const MAX_POLYGON_VERTICES: usize = 512;

fn validate_polygon(v: &serde_json::Value) -> AppResult<()> {
    validate_points(v, 3)
}

fn validate_kind(kind: &str) -> AppResult<()> {
    if matches!(kind, "region" | "presence" | "line") {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "`kind` must be one of region|presence|line".into(),
        ))
    }
}

/// A `kind = "line"` zone's geometry is a 2-point polyline (A→B), not a polygon.
fn validate_line(v: &serde_json::Value) -> AppResult<()> {
    let arr = v
        .as_array()
        .ok_or_else(|| AppError::BadRequest("`polygon` must be an array of [x,y] points".into()))?;
    if arr.len() != 2 {
        return Err(AppError::BadRequest(
            "a line zone's `polygon` must have exactly 2 points (A and B)".into(),
        ));
    }
    validate_points(v, 2)
}

fn validate_points(v: &serde_json::Value, min: usize) -> AppResult<()> {
    let arr = v
        .as_array()
        .ok_or_else(|| AppError::BadRequest("`polygon` must be an array of [x,y] points".into()))?;
    if arr.len() < min {
        return Err(AppError::BadRequest(format!(
            "`polygon` must have at least {min} points"
        )));
    }
    if arr.len() > MAX_POLYGON_VERTICES {
        return Err(AppError::BadRequest(format!(
            "`polygon` has too many vertices (max {MAX_POLYGON_VERTICES})"
        )));
    }
    for (i, pt) in arr.iter().enumerate() {
        let p = pt
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or_else(|| AppError::BadRequest(format!("polygon point {i} must be [x, y]")))?;
        for c in p {
            let n = c
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| AppError::BadRequest(format!("polygon point {i} is not numeric")))?;
            if !(0.0..=1.0).contains(&n) {
                return Err(AppError::BadRequest(format!(
                    "polygon coordinates must be normalized 0..1 (point {i})"
                )));
            }
        }
    }
    Ok(())
}

fn validate_labels(v: &serde_json::Value) -> AppResult<()> {
    let arr = v
        .as_array()
        .ok_or_else(|| AppError::BadRequest("`labels` must be an array of strings".into()))?;
    for l in arr {
        match l.as_str() {
            Some(s) if !s.trim().is_empty() => {}
            _ => {
                return Err(AppError::BadRequest(
                    "`labels` must be non-empty strings".into(),
                ))
            }
        }
    }
    Ok(())
}

fn validate_severity(s: &str) -> AppResult<()> {
    if matches!(s, "info" | "warning" | "critical") {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "`severity` must be info|warning|critical".into(),
        ))
    }
}

async fn list_zones(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<Zone>>> {
    principal.require_cap(Cap::EventsRead, "list zones")?;
    let _ = st.camera_for(&principal, &id).await?;
    let zones = sqlx::query_as::<_, Zone>(
        "SELECT * FROM zones WHERE camera_id = ? ORDER BY created_at ASC",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(zones))
}

async fn create_zone(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<ZoneCreate>,
) -> AppResult<(StatusCode, Json<Zone>)> {
    principal.require(principal.can_manage_registry(), "create zones")?;
    let _ = st.camera_for(&principal, &id).await?;
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    let kind = body.kind.unwrap_or_else(|| "region".into());
    validate_kind(&kind)?;
    if kind == "line" {
        validate_line(&body.polygon)?;
    } else {
        validate_polygon(&body.polygon)?;
    }
    if let Some(l) = &body.labels {
        validate_labels(l)?;
    }
    let severity = body.severity.unwrap_or_else(|| "info".into());
    validate_severity(&severity)?;
    let dwell = body.dwell_seconds.unwrap_or(0.0).max(0.0);
    let labels = SqlxJson(body.labels.unwrap_or_else(|| json!([])));
    let config = SqlxJson(body.config.unwrap_or_else(|| json!({})));
    let polygon = SqlxJson(body.polygon);

    // Idempotency: a camera has at most one zone of a given name. If one already exists, return
    // it instead of silently creating a duplicate — stacked-up identical zones (e.g. a
    // provisioning script re-POSTing on every restart) each fire their own copy of every event.
    // Observed live: 4 duplicates of one full-frame zone quadrupling every enter/dwell. Change an
    // existing zone via PATCH, not by re-creating it.
    if let Some(existing) =
        sqlx::query_as::<_, Zone>("SELECT * FROM zones WHERE camera_id = ? AND name = ?")
            .bind(&id)
            .bind(&body.name)
            .fetch_optional(&st.pool)
            .await?
    {
        return Ok((StatusCode::OK, Json(existing)));
    }

    let now = Utc::now();
    let zone_id = format!("zone_{}", Uuid::new_v4().simple());

    sqlx::query(
        "INSERT INTO zones
           (id, camera_id, name, kind, polygon, dwell_seconds, labels, severity, config, enabled, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&zone_id)
    .bind(&id)
    .bind(&body.name)
    .bind(&kind)
    .bind(polygon)
    .bind(dwell)
    .bind(labels)
    .bind(&severity)
    .bind(config)
    .bind(body.enabled.unwrap_or(true))
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;

    let zone = sqlx::query_as::<_, Zone>("SELECT * FROM zones WHERE id = ?")
        .bind(&zone_id)
        .fetch_one(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_zone",
        "zone",
        &zone_id,
        json!({ "camera_id": &id, "name": &zone.name, "kind": &zone.kind }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(zone)))
}

async fn update_zone(
    State(st): State<AppState>,
    Path(zone_id): Path<String>,
    principal: Principal,
    Json(body): Json<ZoneUpdate>,
) -> AppResult<Json<Zone>> {
    principal.require(principal.can_manage_registry(), "update zones")?;
    // The zone is addressed by its own id, so resolve its owning camera before the row is disclosed.
    let _ = st
        .resource_camera(&principal, CameraOwned::Zone, &zone_id, "update zones")
        .await?;
    let cur = sqlx::query_as::<_, Zone>("SELECT * FROM zones WHERE id = ?")
        .bind(&zone_id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("zone {zone_id} not found")))?;

    let name = body.name.unwrap_or(cur.name);
    let kind = body.kind.unwrap_or(cur.kind);
    validate_kind(&kind)?;
    let severity = body.severity.unwrap_or(cur.severity);
    validate_severity(&severity)?;
    let polygon = match body.polygon {
        Some(p) => {
            if kind == "line" {
                validate_line(&p)?;
            } else {
                validate_polygon(&p)?;
            }
            SqlxJson(p)
        }
        None => {
            // Kind flips must keep geometry consistent (a 2-point line can't become a region).
            if kind == "line" {
                validate_line(&cur.polygon.0)?;
            } else {
                validate_polygon(&cur.polygon.0)?;
            }
            cur.polygon
        }
    };
    let dwell = body
        .dwell_seconds
        .map(|v| v.max(0.0))
        .unwrap_or(cur.dwell_seconds);
    if let Some(l) = &body.labels {
        validate_labels(l)?;
    }
    let labels = SqlxJson(body.labels.unwrap_or(cur.labels.0));
    let config = SqlxJson(body.config.unwrap_or(cur.config.0));
    let enabled = body.enabled.unwrap_or(cur.enabled);

    sqlx::query(
        "UPDATE zones SET name=?, kind=?, polygon=?, dwell_seconds=?, labels=?, severity=?, config=?, enabled=?, updated_at=?
         WHERE id=?",
    )
    .bind(&name)
    .bind(&kind)
    .bind(polygon)
    .bind(dwell)
    .bind(labels)
    .bind(&severity)
    .bind(config)
    .bind(enabled)
    .bind(Utc::now())
    .bind(&zone_id)
    .execute(&st.pool)
    .await?;

    let zone = sqlx::query_as::<_, Zone>("SELECT * FROM zones WHERE id = ?")
        .bind(&zone_id)
        .fetch_one(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_zone",
        "zone",
        &zone_id,
        json!({}),
    )
    .await;
    Ok(Json(zone))
}

async fn delete_zone(
    State(st): State<AppState>,
    Path(zone_id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete zones")?;
    // Before the DELETE: the bare `rows_affected() == 0` 404 below otherwise distinguishes "another
    // camera's zone" (204) from "never existed" (404), which enumerates the box's zone id space.
    let _ = st
        .resource_camera(&principal, CameraOwned::Zone, &zone_id, "delete zones")
        .await?;
    let res = sqlx::query("DELETE FROM zones WHERE id = ?")
        .bind(&zone_id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("zone {zone_id} not found")));
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_zone",
        "zone",
        &zone_id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct ZoneEventQuery {
    from: Option<String>,
    to: Option<String>,
    zone_id: Option<String>,
    event_type: Option<String>,
    track_id: Option<String>,
    limit: Option<i64>,
}

/// A zone event enriched with the recorded segment covering its timestamp (when the indexer has
/// caught up) — the UI turns it into a playback link.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct ZoneEventView {
    #[sqlx(flatten)]
    #[serde(flatten)]
    event: ZoneEvent,
    segment_id: Option<String>,
}

async fn list_zone_events(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<ZoneEventQuery>,
) -> AppResult<Json<Vec<ZoneEventView>>> {
    principal.require_cap(Cap::EventsRead, "list zone events")?;
    let _ = st.camera_for(&principal, &id).await?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let parse = |s: &Option<String>, field: &str| -> AppResult<Option<chrono::DateTime<Utc>>> {
        match s {
            Some(v) => crate::util::parse_rfc3339(v)
                .map(Some)
                .ok_or_else(|| AppError::BadRequest(format!("invalid `{field}` timestamp"))),
            None => Ok(None),
        }
    };
    let from = parse(&q.from, "from")?;
    let to = parse(&q.to, "to")?;
    let rows = sqlx::query_as::<_, ZoneEventView>(
        "SELECT ze.*,
                (SELECT s.id FROM segments s
                  WHERE s.camera_id = ze.camera_id
                    AND s.start_time <= ze.timestamp AND s.end_time >= ze.timestamp
                  LIMIT 1) AS segment_id
         FROM zone_events ze
         WHERE ze.camera_id = ?
           AND (? IS NULL OR ze.timestamp >= ?)
           AND (? IS NULL OR ze.timestamp <= ?)
           AND (? IS NULL OR ze.zone_id = ?)
           AND (? IS NULL OR ze.event_type = ?)
           AND (? IS NULL OR ze.track_id = ?)
         ORDER BY ze.timestamp DESC LIMIT ?",
    )
    .bind(&id)
    .bind(from)
    .bind(from)
    .bind(to)
    .bind(to)
    .bind(&q.zone_id)
    .bind(&q.zone_id)
    .bind(&q.event_type)
    .bind(&q.event_type)
    .bind(&q.track_id)
    .bind(&q.track_id)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct AggregateQuery {
    from: Option<String>,
    to: Option<String>,
    /// Bucket width in minutes (default 60, clamped 1..=1440).
    bucket_minutes: Option<i64>,
}

/// Server-side zone-event aggregates: per (zone, event_type, time bucket) counts — the data
/// behind occupancy/throughput charts (line-crossing tallies, enters per hour) without shipping
/// raw events to the client.
async fn zone_event_aggregates(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<AggregateQuery>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "aggregate zone events")?;
    let _ = st.camera_for(&principal, &id).await?;
    let bucket_minutes = q.bucket_minutes.unwrap_or(60).clamp(1, 1440);
    let parse = |s: &Option<String>, field: &str| -> AppResult<Option<chrono::DateTime<Utc>>> {
        match s {
            Some(v) => crate::util::parse_rfc3339(v)
                .map(Some)
                .ok_or_else(|| AppError::BadRequest(format!("invalid `{field}` timestamp"))),
            None => Ok(None),
        }
    };
    let to = parse(&q.to, "to")?.unwrap_or_else(Utc::now);
    let from = parse(&q.from, "from")?.unwrap_or_else(|| to - chrono::Duration::hours(24));
    let bucket_secs = bucket_minutes * 60;
    // Bucket by epoch-seconds divided into fixed windows (SQLite: unixepoch on the RFC3339 text).
    let rows: Vec<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT ze.zone_id, ze.zone_name, ze.event_type,
                (CAST(strftime('%s', ze.timestamp) AS INTEGER) / ?) * ? AS bucket_start,
                COUNT(*) AS n
         FROM zone_events ze
         WHERE ze.camera_id = ? AND ze.timestamp >= ? AND ze.timestamp <= ?
         GROUP BY ze.zone_id, ze.event_type, bucket_start
         ORDER BY bucket_start ASC",
    )
    .bind(bucket_secs)
    .bind(bucket_secs)
    .bind(&id)
    .bind(from)
    .bind(to)
    .fetch_all(&st.pool)
    .await?;
    let buckets: Vec<Value> = rows
        .into_iter()
        .map(|(zone_id, zone_name, event_type, bucket_start, n)| {
            json!({
                "zone_id": zone_id,
                "zone_name": zone_name,
                "event_type": event_type,
                "bucket_start": chrono::DateTime::<Utc>::from_timestamp(bucket_start, 0)
                    .map(|t| t.to_rfc3339()),
                "count": n,
            })
        })
        .collect();
    Ok(Json(json!({
        "from": from.to_rfc3339(),
        "to": to.to_rfc3339(),
        "bucket_minutes": bucket_minutes,
        "buckets": buckets,
    })))
}

/// Live per-zone occupancy (tracks currently inside), maintained by the zone engine as a
/// write-behind aggregate. `updated_at` tells the reader how fresh each count is (engine state is
/// in-memory and resets on restart).
async fn zone_occupancy(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "view zone occupancy")?;
    let _ = st.camera_for(&principal, &id).await?;
    let rows: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT zo.zone_id, zo.count, zo.updated_at
         FROM zone_occupancy zo JOIN zones z ON z.id = zo.zone_id
         WHERE zo.camera_id = ? AND z.enabled = 1
         ORDER BY zo.zone_id",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(zone_id, count, updated_at)| {
            json!({ "zone_id": zone_id, "count": count, "updated_at": updated_at })
        })
        .collect();
    Ok(Json(json!({ "zones": out })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Scope;
    use std::collections::HashSet;
    use std::sync::Arc;

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = Arc::new(crate::config::Config::from_env());
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            media_jobs: crate::services::media_jobs::MediaJobGovernor::new(2),
            started_at: Utc::now(),
            pool,
            cfg,
        }
    }

    fn scoped(cameras: &[&str]) -> Principal {
        let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: Scope::Cameras(Arc::new(set)),
            ..Principal::system_admin()
        }
    }

    async fn seed(pool: &sqlx::SqlitePool, camera_id: &str, zone_id: &str) {
        let now = Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
            .bind(camera_id)
            .bind(camera_id)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO zones (id, camera_id, name, polygon, created_at, updated_at)
             VALUES (?,?,?,'[[0,0],[0,1],[1,1]]',?,?)",
        )
        .bind(zone_id)
        .bind(camera_id)
        .bind(zone_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    fn empty_update() -> ZoneUpdate {
        serde_json::from_value(json!({})).unwrap()
    }

    /// THE ORACLE TEST. `DELETE /api/v1/zones/{id}` used to answer 204 for another camera's zone and
    /// 404 for a fabricated one, which enumerates the box's zone id space one probe at a time. Both
    /// must now be the SAME refusal, byte for byte, naming neither the owner nor the probed id.
    #[tokio::test]
    async fn a_scoped_key_cannot_distinguish_another_cameras_zone_from_a_missing_one() {
        let st = test_state().await;
        seed(&st.pool, "cam_a", "zone_a").await;
        seed(&st.pool, "cam_sentinel_b", "zone_b").await;
        let p = scoped(&["cam_a"]);

        for (out_of_scope, nonexistent) in [
            (
                delete_zone(State(st.clone()), Path("zone_b".into()), p.clone())
                    .await
                    .unwrap_err(),
                delete_zone(State(st.clone()), Path("zone_zzz".into()), p.clone())
                    .await
                    .unwrap_err(),
            ),
            (
                update_zone(
                    State(st.clone()),
                    Path("zone_b".into()),
                    p.clone(),
                    Json(empty_update()),
                )
                .await
                .unwrap_err(),
                update_zone(
                    State(st.clone()),
                    Path("zone_zzz".into()),
                    p.clone(),
                    Json(empty_update()),
                )
                .await
                .unwrap_err(),
            ),
        ] {
            assert!(matches!(out_of_scope, AppError::Forbidden(_)));
            assert!(matches!(nonexistent, AppError::Forbidden(_)));
            assert_eq!(out_of_scope.to_string(), nonexistent.to_string());
            let msg = out_of_scope.to_string();
            assert!(!msg.contains("cam_sentinel_b"), "{msg}");
            assert!(!msg.contains("zone_b"), "{msg}");
        }

        // And the refusal is real: the row survived.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM zones WHERE id = 'zone_b'")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "another camera's zone must not be deletable");
    }

    /// Constraint 2: an unscoped credential sees NO change — including the 404 body for a missing zone.
    #[tokio::test]
    async fn an_unscoped_principal_is_unaffected() {
        let st = test_state().await;
        seed(&st.pool, "cam_sentinel_b", "zone_b").await;
        let admin = Principal::system_admin();

        match delete_zone(State(st.clone()), Path("zone_zzz".into()), admin.clone())
            .await
            .unwrap_err()
        {
            AppError::NotFound(m) => assert_eq!(m, "zone zone_zzz not found"),
            other => panic!("expected the pre-existing 404, got {other:?}"),
        }
        // ...and it can still delete any camera's zone, exactly as before.
        assert_eq!(
            delete_zone(State(st.clone()), Path("zone_b".into()), admin)
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
    }

    /// Constraint: no over-blocking. A scoped key still owns its own camera's zones.
    #[tokio::test]
    async fn a_scoped_principal_still_edits_its_own_zone() {
        let st = test_state().await;
        seed(&st.pool, "cam_a", "zone_a").await;
        let p = scoped(&["cam_a"]);
        assert!(update_zone(
            State(st.clone()),
            Path("zone_a".into()),
            p.clone(),
            Json(empty_update()),
        )
        .await
        .is_ok());
        assert_eq!(
            delete_zone(State(st.clone()), Path("zone_a".into()), p)
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
    }
}
