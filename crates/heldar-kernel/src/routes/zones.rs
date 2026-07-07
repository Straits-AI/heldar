//! Zone CRUD + zone-events query (Stage 3).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{Zone, ZoneCreate, ZoneEvent, ZoneUpdate};
use crate::routes::cameras::load_camera;
use crate::state::AppState;

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
}

const MAX_POLYGON_VERTICES: usize = 512;

fn validate_polygon(v: &serde_json::Value) -> AppResult<()> {
    let arr = v
        .as_array()
        .ok_or_else(|| AppError::BadRequest("`polygon` must be an array of [x,y] points".into()))?;
    if arr.len() < 3 {
        return Err(AppError::BadRequest(
            "`polygon` must have at least 3 points".into(),
        ));
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
    principal.require(principal.can_view(), "list zones")?;
    let _ = load_camera(&st.pool, &id).await?;
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
    let _ = load_camera(&st.pool, &id).await?;
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    validate_polygon(&body.polygon)?;
    if let Some(l) = &body.labels {
        validate_labels(l)?;
    }
    let severity = body.severity.unwrap_or_else(|| "info".into());
    validate_severity(&severity)?;
    let kind = body.kind.unwrap_or_else(|| "region".into());
    let dwell = body.dwell_seconds.unwrap_or(0.0).max(0.0);
    let labels = SqlxJson(body.labels.unwrap_or_else(|| json!([])));
    let config = SqlxJson(body.config.unwrap_or_else(|| json!({})));
    let polygon = SqlxJson(body.polygon);
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
    let cur = sqlx::query_as::<_, Zone>("SELECT * FROM zones WHERE id = ?")
        .bind(&zone_id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("zone {zone_id} not found")))?;

    let name = body.name.unwrap_or(cur.name);
    let kind = body.kind.unwrap_or(cur.kind);
    let severity = body.severity.unwrap_or(cur.severity);
    validate_severity(&severity)?;
    let polygon = match body.polygon {
        Some(p) => {
            validate_polygon(&p)?;
            SqlxJson(p)
        }
        None => cur.polygon,
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
    limit: Option<i64>,
}

async fn list_zone_events(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<ZoneEventQuery>,
) -> AppResult<Json<Vec<ZoneEvent>>> {
    principal.require(principal.can_view(), "list zone events")?;
    let _ = load_camera(&st.pool, &id).await?;
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
    let rows = sqlx::query_as::<_, ZoneEvent>(
        "SELECT * FROM zone_events
         WHERE camera_id = ?
           AND (? IS NULL OR timestamp >= ?)
           AND (? IS NULL OR timestamp <= ?)
           AND (? IS NULL OR zone_id = ?)
           AND (? IS NULL OR event_type = ?)
         ORDER BY timestamp DESC LIMIT ?",
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
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}
