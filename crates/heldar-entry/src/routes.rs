//! Stage 4 access-control surface: registered vehicles, visitor passes (+ check-in/out), watchlist,
//! the canonical entry-event feed with a guard confirm/reject workflow, and reports (daily entry
//! log, exceptions, audit). Reads require any authenticated principal; registry mutations require
//! manager+, gate operations require guard+, and the audit report requires manager+.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::anpr::normalize_plate;
use crate::models::{
    AuditLog, EntryEvent, Vehicle, VehicleCreate, VehicleUpdate, VisitorPass, VisitorPassCreate,
    VisitorPassUpdate, Watchlist, WatchlistCreate, WatchlistUpdate,
};
use heldar_kernel::auth::{self, Principal};
use heldar_kernel::error::{AppError, AppResult};
use heldar_kernel::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/vehicles", get(list_vehicles).post(create_vehicle))
        .route(
            "/api/v1/vehicles/{id}",
            get(get_vehicle)
                .patch(update_vehicle)
                .delete(delete_vehicle),
        )
        .route("/api/v1/passes", get(list_passes).post(create_pass))
        .route(
            "/api/v1/passes/{id}",
            get(get_pass).patch(update_pass).delete(delete_pass),
        )
        .route("/api/v1/passes/{id}/checkin", post(checkin_pass))
        .route("/api/v1/passes/{id}/checkout", post(checkout_pass))
        .route("/api/v1/watchlist", get(list_watchlist).post(create_watch))
        .route(
            "/api/v1/watchlist/{id}",
            axum::routing::patch(update_watch).delete(delete_watch),
        )
        .route("/api/v1/entry-events", get(list_entry_events))
        .route("/api/v1/entry-events/{id}", get(get_entry_event))
        .route("/api/v1/entry-events/{id}/confirm", post(confirm_event))
        .route("/api/v1/entry-events/{id}/reject", post(reject_event))
        .route("/api/v1/reports/entry-log", get(report_entry_log))
        .route("/api/v1/reports/exceptions", get(report_exceptions))
        .route("/api/v1/audit", get(list_audit))
}

const OWNER_TYPES: [&str; 5] = ["student", "staff", "resident", "contractor", "visitor"];
const WATCH_KINDS: [&str; 3] = ["block", "vip", "alert"];
const SEVERITIES: [&str; 3] = ["info", "warning", "critical"];

fn parse_opt_ts(s: &Option<String>, field: &str) -> AppResult<Option<DateTime<Utc>>> {
    match s {
        Some(v) if !v.trim().is_empty() => heldar_kernel::util::parse_rfc3339(v)
            .map(Some)
            .ok_or_else(|| AppError::BadRequest(format!("invalid `{field}` timestamp"))),
        _ => Ok(None),
    }
}

// ---- Vehicles ------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VehicleQuery {
    plate: Option<String>,
    owner_type: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
}

async fn list_vehicles(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<VehicleQuery>,
) -> AppResult<Json<Vec<Vehicle>>> {
    principal.require(principal.can_view(), "view vehicles")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    let plate_norm = q.plate.as_deref().map(normalize_plate);
    let like = q.q.as_deref().map(|s| format!("%{}%", s.trim()));
    let rows = sqlx::query_as::<_, Vehicle>(
        "SELECT * FROM vehicles
          WHERE (? IS NULL OR plate_norm = ?)
            AND (? IS NULL OR owner_type = ?)
            AND (? IS NULL OR owner_name LIKE ? OR plate LIKE ? OR owner_ref LIKE ?)
          ORDER BY created_at DESC LIMIT ?",
    )
    .bind(&plate_norm)
    .bind(&plate_norm)
    .bind(&q.owner_type)
    .bind(&q.owner_type)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

async fn get_vehicle(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<Vehicle>> {
    principal.require(principal.can_view(), "view vehicles")?;
    let v = sqlx::query_as::<_, Vehicle>("SELECT * FROM vehicles WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("vehicle {id} not found")))?;
    Ok(Json(v))
}

async fn create_vehicle(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<VehicleCreate>,
) -> AppResult<(StatusCode, Json<Vehicle>)> {
    principal.require(principal.can_manage_registry(), "register vehicles")?;
    let plate_norm = normalize_plate(&body.plate);
    if plate_norm.is_empty() {
        return Err(AppError::BadRequest("`plate` is required".into()));
    }
    let owner_type = body.owner_type.unwrap_or_else(|| "visitor".into());
    if !OWNER_TYPES.contains(&owner_type.as_str()) {
        return Err(AppError::BadRequest(format!(
            "`owner_type` must be one of {OWNER_TYPES:?}"
        )));
    }
    let valid_from = parse_opt_ts(&body.valid_from, "valid_from")?;
    let valid_until = parse_opt_ts(&body.valid_until, "valid_until")?;
    if let (Some(f), Some(u)) = (valid_from, valid_until) {
        if u < f {
            return Err(AppError::BadRequest(
                "`valid_until` must not precede `valid_from`".into(),
            ));
        }
    }
    let id = format!("veh_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO vehicles
           (id, plate, plate_norm, owner_name, owner_type, owner_ref, site_id, vehicle_type,
            make, model, color, notes, active, valid_from, valid_until, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(body.plate.trim())
    .bind(&plate_norm)
    .bind(&body.owner_name)
    .bind(&owner_type)
    .bind(&body.owner_ref)
    .bind(&body.site_id)
    .bind(&body.vehicle_type)
    .bind(&body.make)
    .bind(&body.model)
    .bind(&body.color)
    .bind(&body.notes)
    .bind(body.active.unwrap_or(true))
    .bind(valid_from)
    .bind(valid_until)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_vehicle",
        "vehicle",
        &id,
        json!({ "plate": plate_norm }),
    )
    .await;
    let v = sqlx::query_as::<_, Vehicle>("SELECT * FROM vehicles WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok((StatusCode::CREATED, Json(v)))
}

async fn update_vehicle(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<VehicleUpdate>,
) -> AppResult<Json<Vehicle>> {
    principal.require(principal.can_manage_registry(), "modify vehicles")?;
    let cur = sqlx::query_as::<_, Vehicle>("SELECT * FROM vehicles WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("vehicle {id} not found")))?;

    let (plate, plate_norm) = match body.plate {
        Some(p) => {
            let n = normalize_plate(&p);
            if n.is_empty() {
                return Err(AppError::BadRequest("`plate` cannot be empty".into()));
            }
            (p.trim().to_string(), n)
        }
        None => (cur.plate, cur.plate_norm),
    };
    let owner_type = body.owner_type.unwrap_or(cur.owner_type);
    if !OWNER_TYPES.contains(&owner_type.as_str()) {
        return Err(AppError::BadRequest(format!(
            "`owner_type` must be one of {OWNER_TYPES:?}"
        )));
    }
    let valid_from = match &body.valid_from {
        Some(_) => parse_opt_ts(&body.valid_from, "valid_from")?,
        None => cur.valid_from,
    };
    let valid_until = match &body.valid_until {
        Some(_) => parse_opt_ts(&body.valid_until, "valid_until")?,
        None => cur.valid_until,
    };
    if let (Some(f), Some(u)) = (valid_from, valid_until) {
        if u < f {
            return Err(AppError::BadRequest(
                "`valid_until` must not precede `valid_from`".into(),
            ));
        }
    }
    sqlx::query(
        "UPDATE vehicles SET plate=?, plate_norm=?, owner_name=?, owner_type=?, owner_ref=?,
            site_id=?, vehicle_type=?, make=?, model=?, color=?, notes=?, active=?,
            valid_from=?, valid_until=?, updated_at=? WHERE id=?",
    )
    .bind(&plate)
    .bind(&plate_norm)
    .bind(body.owner_name.or(cur.owner_name))
    .bind(&owner_type)
    .bind(body.owner_ref.or(cur.owner_ref))
    .bind(body.site_id.or(cur.site_id))
    .bind(body.vehicle_type.or(cur.vehicle_type))
    .bind(body.make.or(cur.make))
    .bind(body.model.or(cur.model))
    .bind(body.color.or(cur.color))
    .bind(body.notes.or(cur.notes))
    .bind(body.active.unwrap_or(cur.active))
    .bind(valid_from)
    .bind(valid_until)
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_vehicle",
        "vehicle",
        &id,
        json!({}),
    )
    .await;
    let v = sqlx::query_as::<_, Vehicle>("SELECT * FROM vehicles WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(v))
}

async fn delete_vehicle(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete vehicles")?;
    let res = sqlx::query("DELETE FROM vehicles WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("vehicle {id} not found")));
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_vehicle",
        "vehicle",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Visitor passes ------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PassQuery {
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
}

async fn list_passes(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<PassQuery>,
) -> AppResult<Json<Vec<VisitorPass>>> {
    principal.require(principal.can_view(), "view passes")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 2000);
    let like = q.q.as_deref().map(|s| format!("%{}%", s.trim()));
    let rows = sqlx::query_as::<_, VisitorPass>(
        "SELECT * FROM visitor_passes
          WHERE (? IS NULL OR status = ?)
            AND (? IS NULL OR visitor_name LIKE ? OR plate LIKE ? OR code LIKE ? OR host LIKE ?)
          ORDER BY created_at DESC LIMIT ?",
    )
    .bind(&q.status)
    .bind(&q.status)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

async fn get_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<VisitorPass>> {
    principal.require(principal.can_view(), "view passes")?;
    Ok(Json(load_pass(&st.pool, &id).await?))
}

async fn load_pass(pool: &sqlx::SqlitePool, id: &str) -> AppResult<VisitorPass> {
    sqlx::query_as::<_, VisitorPass>("SELECT * FROM visitor_passes WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("pass {id} not found")))
}

async fn create_pass(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<VisitorPassCreate>,
) -> AppResult<(StatusCode, Json<VisitorPass>)> {
    principal.require(principal.can_operate_gate(), "create visitor passes")?;
    if body.visitor_name.trim().is_empty() {
        return Err(AppError::BadRequest("`visitor_name` is required".into()));
    }
    let now = Utc::now();
    let valid_from = parse_opt_ts(&body.valid_from, "valid_from")?.unwrap_or(now);
    let valid_until = parse_opt_ts(&body.valid_until, "valid_until")?
        .unwrap_or_else(|| now + Duration::hours(24));
    if valid_until < valid_from {
        return Err(AppError::BadRequest(
            "`valid_until` must not precede `valid_from`".into(),
        ));
    }
    let plate_norm = body
        .plate
        .as_deref()
        .map(normalize_plate)
        .filter(|s| !s.is_empty());
    let id = format!("pass_{}", Uuid::new_v4().simple());
    let code = format!(
        "V-{}",
        Uuid::new_v4().simple().to_string()[..6].to_uppercase()
    );
    sqlx::query(
        "INSERT INTO visitor_passes
           (id, code, visitor_name, phone, company, host, purpose, plate, plate_norm, vehicle_desc,
            site_id, valid_from, valid_until, status, created_by, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,'active',?,?,?)",
    )
    .bind(&id)
    .bind(&code)
    .bind(body.visitor_name.trim())
    .bind(&body.phone)
    .bind(&body.company)
    .bind(&body.host)
    .bind(&body.purpose)
    .bind(body.plate.as_deref().map(|p| p.trim().to_string()))
    .bind(&plate_norm)
    .bind(&body.vehicle_desc)
    .bind(&body.site_id)
    .bind(valid_from)
    .bind(valid_until)
    .bind(&principal.id)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_pass",
        "pass",
        &id,
        json!({ "code": code }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(load_pass(&st.pool, &id).await?)))
}

async fn update_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<VisitorPassUpdate>,
) -> AppResult<Json<VisitorPass>> {
    principal.require(principal.can_operate_gate(), "modify visitor passes")?;
    let cur = load_pass(&st.pool, &id).await?;
    let status = body.status.unwrap_or_else(|| cur.status.clone());
    if !["active", "checked_in", "checked_out", "expired", "revoked"].contains(&status.as_str()) {
        return Err(AppError::BadRequest(
            "`status` must be active|checked_in|checked_out|expired|revoked".into(),
        ));
    }
    // `revoked` is a terminal state: only a manager+ may reinstate it (a guard cannot resurrect a
    // revoked pass by editing its status).
    if cur.status == "revoked" && status != "revoked" {
        principal.require(principal.can_manage_registry(), "reinstate a revoked pass")?;
    }
    let valid_from = match &body.valid_from {
        Some(_) => parse_opt_ts(&body.valid_from, "valid_from")?.unwrap_or(cur.valid_from),
        None => cur.valid_from,
    };
    let valid_until = match &body.valid_until {
        Some(_) => parse_opt_ts(&body.valid_until, "valid_until")?.unwrap_or(cur.valid_until),
        None => cur.valid_until,
    };
    if valid_until < valid_from {
        return Err(AppError::BadRequest(
            "`valid_until` must not precede `valid_from`".into(),
        ));
    }
    let (plate, plate_norm) = match body.plate {
        Some(p) => {
            let n = normalize_plate(&p);
            (Some(p.trim().to_string()), (!n.is_empty()).then_some(n))
        }
        None => (cur.plate, cur.plate_norm),
    };
    sqlx::query(
        "UPDATE visitor_passes SET visitor_name=?, phone=?, company=?, host=?, purpose=?, plate=?,
            plate_norm=?, vehicle_desc=?, valid_from=?, valid_until=?, status=?, updated_at=? WHERE id=?",
    )
    .bind(body.visitor_name.unwrap_or(cur.visitor_name))
    .bind(body.phone.or(cur.phone))
    .bind(body.company.or(cur.company))
    .bind(body.host.or(cur.host))
    .bind(body.purpose.or(cur.purpose))
    .bind(&plate)
    .bind(&plate_norm)
    .bind(body.vehicle_desc.or(cur.vehicle_desc))
    .bind(valid_from)
    .bind(valid_until)
    .bind(&status)
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_pass",
        "pass",
        &id,
        json!({ "status": status }),
    )
    .await;
    Ok(Json(load_pass(&st.pool, &id).await?))
}

async fn delete_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete visitor passes")?;
    let res = sqlx::query("DELETE FROM visitor_passes WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("pass {id} not found")));
    }
    auth::audit(&st.pool, &principal, "delete_pass", "pass", &id, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn checkin_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<VisitorPass>> {
    principal.require(principal.can_operate_gate(), "check in visitors")?;
    let pass = load_pass(&st.pool, &id).await?;
    // Only an active (or already-checked-in, idempotent) pass can be checked in. revoked / expired /
    // checked_out are terminal-ish and must not be silently reactivated.
    if !matches!(pass.status.as_str(), "active" | "checked_in") {
        return Err(AppError::BadRequest(format!(
            "pass is {} and cannot be checked in",
            pass.status
        )));
    }
    let now = Utc::now();
    sqlx::query(
        "UPDATE visitor_passes SET status='checked_in', checked_in_at=?, updated_at=? WHERE id=?",
    )
    .bind(now)
    .bind(now)
    .bind(&id)
    .execute(&st.pool)
    .await?;
    record_manual_entry(&st, &principal, &pass, "visitor_checkin", "inbound", now).await;
    auth::audit(&st.pool, &principal, "checkin_pass", "pass", &id, json!({})).await;
    Ok(Json(load_pass(&st.pool, &id).await?))
}

async fn checkout_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<VisitorPass>> {
    principal.require(principal.can_operate_gate(), "check out visitors")?;
    let pass = load_pass(&st.pool, &id).await?;
    // A revoked / expired pass is terminal — do not flip it to checked_out (which would also let it
    // be resurrected via a later check-in).
    if matches!(pass.status.as_str(), "revoked" | "expired") {
        return Err(AppError::BadRequest(format!(
            "pass is {} and cannot be checked out",
            pass.status
        )));
    }
    let now = Utc::now();
    sqlx::query(
        "UPDATE visitor_passes SET status='checked_out', checked_out_at=?, updated_at=? WHERE id=?",
    )
    .bind(now)
    .bind(now)
    .bind(&id)
    .execute(&st.pool)
    .await?;
    record_manual_entry(&st, &principal, &pass, "visitor_checkout", "outbound", now).await;
    auth::audit(
        &st.pool,
        &principal,
        "checkout_pass",
        "pass",
        &id,
        json!({}),
    )
    .await;
    Ok(Json(load_pass(&st.pool, &id).await?))
}

/// Write a guard-initiated entry event (manual check-in/out) into the canonical feed.
async fn record_manual_entry(
    st: &AppState,
    principal: &Principal,
    pass: &VisitorPass,
    event_type: &str,
    direction: &str,
    now: DateTime<Utc>,
) {
    let id = format!("evt_{}", Uuid::new_v4().simple());
    let subject = json!({
        "type": "visitor",
        "visitor_name": pass.visitor_name,
        "plate": pass.plate,
        "pass_code": pass.code,
    });
    let authorization =
        json!({ "status": "matched", "source": "visitor_pass", "pass_id": pass.id });
    let workflow = json!({ "status": "confirmed", "resolved_by": principal.name });
    let audit_j = json!({ "created_by": principal.id });
    let _ = sqlx::query(
        "INSERT INTO entry_events
           (id, site_id, camera_id, event_type, timestamp, direction, plate, plate_confidence,
            subject, authorization, auth_status, evidence, workflow_status, workflow, audit,
            track_id, created_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&pass.site_id)
    .bind(Option::<String>::None)
    .bind(event_type)
    .bind(now)
    .bind(direction)
    .bind(&pass.plate_norm)
    .bind(Option::<f64>::None)
    .bind(SqlxJson(&subject))
    .bind(SqlxJson(&authorization))
    .bind("matched")
    .bind(SqlxJson(json!({})))
    .bind("confirmed")
    .bind(SqlxJson(&workflow))
    .bind(SqlxJson(&audit_j))
    .bind(Option::<String>::None)
    .bind(now)
    .execute(&st.pool)
    .await;
}

// ---- Watchlist -----------------------------------------------------------

async fn list_watchlist(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<Watchlist>>> {
    principal.require(principal.can_view(), "view watchlist")?;
    // Bound the result set: the watchlist can grow large, and an unbounded SELECT * would load every
    // row into memory at once (OOM/latency risk). 1000 is well above any realistic operator view.
    let rows = sqlx::query_as::<_, Watchlist>(
        "SELECT * FROM watchlist ORDER BY created_at DESC LIMIT 1000",
    )
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

async fn create_watch(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<WatchlistCreate>,
) -> AppResult<(StatusCode, Json<Watchlist>)> {
    principal.require(principal.can_manage_registry(), "manage the watchlist")?;
    let plate_norm = normalize_plate(&body.plate);
    if plate_norm.is_empty() {
        return Err(AppError::BadRequest("`plate` is required".into()));
    }
    let kind = body.kind.unwrap_or_else(|| "block".into());
    if !WATCH_KINDS.contains(&kind.as_str()) {
        return Err(AppError::BadRequest(format!(
            "`kind` must be one of {WATCH_KINDS:?}"
        )));
    }
    let severity = body.severity.unwrap_or_else(|| "warning".into());
    if !SEVERITIES.contains(&severity.as_str()) {
        return Err(AppError::BadRequest(format!(
            "`severity` must be one of {SEVERITIES:?}"
        )));
    }
    let id = format!("wl_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO watchlist (id, plate, plate_norm, kind, reason, severity, active, created_by, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(body.plate.trim())
    .bind(&plate_norm)
    .bind(&kind)
    .bind(&body.reason)
    .bind(&severity)
    .bind(body.active.unwrap_or(true))
    .bind(&principal.id)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_watchlist",
        "watchlist",
        &id,
        json!({ "plate": plate_norm, "kind": kind }),
    )
    .await;
    let w = sqlx::query_as::<_, Watchlist>("SELECT * FROM watchlist WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok((StatusCode::CREATED, Json(w)))
}

async fn update_watch(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<WatchlistUpdate>,
) -> AppResult<Json<Watchlist>> {
    principal.require(principal.can_manage_registry(), "manage the watchlist")?;
    let cur = sqlx::query_as::<_, Watchlist>("SELECT * FROM watchlist WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("watchlist entry {id} not found")))?;
    let kind = body.kind.unwrap_or(cur.kind);
    if !WATCH_KINDS.contains(&kind.as_str()) {
        return Err(AppError::BadRequest(format!(
            "`kind` must be one of {WATCH_KINDS:?}"
        )));
    }
    let severity = body.severity.unwrap_or(cur.severity);
    if !SEVERITIES.contains(&severity.as_str()) {
        return Err(AppError::BadRequest(format!(
            "`severity` must be one of {SEVERITIES:?}"
        )));
    }
    sqlx::query(
        "UPDATE watchlist SET kind=?, reason=?, severity=?, active=?, updated_at=? WHERE id=?",
    )
    .bind(&kind)
    .bind(body.reason.or(cur.reason))
    .bind(&severity)
    .bind(body.active.unwrap_or(cur.active))
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_watchlist",
        "watchlist",
        &id,
        json!({}),
    )
    .await;
    let w = sqlx::query_as::<_, Watchlist>("SELECT * FROM watchlist WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(w))
}

async fn delete_watch(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "manage the watchlist")?;
    let res = sqlx::query("DELETE FROM watchlist WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "watchlist entry {id} not found"
        )));
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_watchlist",
        "watchlist",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Entry events + guard workflow --------------------------------------

#[derive(Debug, Deserialize)]
struct EntryEventQuery {
    from: Option<String>,
    to: Option<String>,
    plate: Option<String>,
    auth_status: Option<String>,
    workflow_status: Option<String>,
    event_type: Option<String>,
    limit: Option<i64>,
}

async fn list_entry_events(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<EntryEventQuery>,
) -> AppResult<Json<Vec<EntryEvent>>> {
    principal.require(principal.can_view(), "view entry events")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let from = parse_opt_ts(&q.from, "from")?;
    let to = parse_opt_ts(&q.to, "to")?;
    let plate_norm = q.plate.as_deref().map(normalize_plate);
    let rows = sqlx::query_as::<_, EntryEvent>(
        "SELECT * FROM entry_events
          WHERE (? IS NULL OR timestamp >= ?)
            AND (? IS NULL OR timestamp <= ?)
            AND (? IS NULL OR plate = ?)
            AND (? IS NULL OR auth_status = ?)
            AND (? IS NULL OR workflow_status = ?)
            AND (? IS NULL OR event_type = ?)
          ORDER BY timestamp DESC LIMIT ?",
    )
    .bind(from)
    .bind(from)
    .bind(to)
    .bind(to)
    .bind(&plate_norm)
    .bind(&plate_norm)
    .bind(&q.auth_status)
    .bind(&q.auth_status)
    .bind(&q.workflow_status)
    .bind(&q.workflow_status)
    .bind(&q.event_type)
    .bind(&q.event_type)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

async fn get_entry_event(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<EntryEvent>> {
    principal.require(principal.can_view(), "view entry events")?;
    let ev = sqlx::query_as::<_, EntryEvent>("SELECT * FROM entry_events WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("entry event {id} not found")))?;
    Ok(Json(ev))
}

#[derive(Debug, Deserialize, Default)]
struct ResolveBody {
    note: Option<String>,
}

async fn confirm_event(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    body: Option<Json<ResolveBody>>,
) -> AppResult<Json<EntryEvent>> {
    resolve_event(
        st,
        principal,
        id,
        "confirmed",
        body.map(|b| b.0).unwrap_or_default(),
    )
    .await
}

async fn reject_event(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    body: Option<Json<ResolveBody>>,
) -> AppResult<Json<EntryEvent>> {
    resolve_event(
        st,
        principal,
        id,
        "rejected",
        body.map(|b| b.0).unwrap_or_default(),
    )
    .await
}

async fn resolve_event(
    st: AppState,
    principal: Principal,
    id: String,
    status: &str,
    body: ResolveBody,
) -> AppResult<Json<EntryEvent>> {
    principal.require(principal.can_operate_gate(), "resolve entry events")?;
    let ev = sqlx::query_as::<_, EntryEvent>("SELECT * FROM entry_events WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("entry event {id} not found")))?;
    let now = Utc::now();
    let mut workflow = ev.workflow.0.clone();
    if let Some(obj) = workflow.as_object_mut() {
        obj.insert("status".into(), json!(status));
        obj.insert("resolved_by".into(), json!(principal.name));
        obj.insert("resolved_by_id".into(), json!(principal.id));
        obj.insert("resolved_at".into(), json!(now.to_rfc3339()));
        if let Some(note) = &body.note {
            obj.insert("note".into(), json!(note));
        }
    }
    sqlx::query("UPDATE entry_events SET workflow_status=?, workflow=? WHERE id=?")
        .bind(status)
        .bind(SqlxJson(&workflow))
        .bind(&id)
        .execute(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        &format!("entry_{status}"),
        "entry_event",
        &id,
        json!({ "plate": ev.plate, "note": body.note }),
    )
    .await;
    let ev = sqlx::query_as::<_, EntryEvent>("SELECT * FROM entry_events WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(ev))
}

// ---- Reports -------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ReportQuery {
    date: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<i64>,
}

/// Resolve a [from, to) window from either an explicit from/to or a `date=YYYY-MM-DD` (UTC day).
fn report_window(q: &ReportQuery) -> AppResult<(DateTime<Utc>, DateTime<Utc>)> {
    if q.from.is_some() || q.to.is_some() {
        let from = parse_opt_ts(&q.from, "from")?.unwrap_or_else(|| Utc::now() - Duration::days(1));
        let to = parse_opt_ts(&q.to, "to")?.unwrap_or_else(Utc::now);
        if to < from {
            return Err(AppError::BadRequest("`to` must not precede `from`".into()));
        }
        return Ok((from, to));
    }
    let day = match &q.date {
        Some(d) => chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d")
            .map_err(|_| AppError::BadRequest("`date` must be YYYY-MM-DD".into()))?,
        None => Utc::now().date_naive(),
    };
    let start = day
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| AppError::BadRequest("invalid date".into()))?
        .and_utc();
    Ok((start, start + Duration::days(1)))
}

async fn report_entry_log(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<ReportQuery>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_view(), "view reports")?;
    let (from, to) = report_window(&q)?;
    let limit = q.limit.unwrap_or(1000).clamp(1, 10000);
    let events = sqlx::query_as::<_, EntryEvent>(
        "SELECT * FROM entry_events WHERE timestamp >= ? AND timestamp < ? ORDER BY timestamp DESC LIMIT ?",
    )
    .bind(from)
    .bind(to)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    let counts = auth_status_counts(&st.pool, from, to).await?;
    Ok(Json(json!({
        "from": from, "to": to,
        "total": events.len(),
        "by_auth_status": counts,
        "events": events,
    })))
}

async fn report_exceptions(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<ReportQuery>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_view(), "view reports")?;
    let (from, to) = report_window(&q)?;
    let limit = q.limit.unwrap_or(1000).clamp(1, 10000);
    // Exceptions = anything that is not an automatic clean match: blocked / exception / unmatched,
    // plus any event a guard explicitly rejected.
    let events = sqlx::query_as::<_, EntryEvent>(
        "SELECT * FROM entry_events
          WHERE timestamp >= ? AND timestamp < ?
            AND (auth_status IN ('blocked','exception','unmatched') OR workflow_status = 'rejected')
          ORDER BY timestamp DESC LIMIT ?",
    )
    .bind(from)
    .bind(to)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(json!({
        "from": from, "to": to,
        "total": events.len(),
        "events": events,
    })))
}

async fn auth_status_counts(
    pool: &sqlx::SqlitePool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<Value> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT auth_status, COUNT(*) FROM entry_events
          WHERE timestamp >= ? AND timestamp < ? GROUP BY auth_status",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    let mut map = serde_json::Map::new();
    for (k, v) in rows {
        map.insert(k, json!(v));
    }
    Ok(Value::Object(map))
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    from: Option<String>,
    to: Option<String>,
    actor: Option<String>,
    action: Option<String>,
    limit: Option<i64>,
}

async fn list_audit(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<Vec<AuditLog>>> {
    // The audit log records who did what — restricted to manager+ (it can reveal operator activity).
    principal.require(principal.can_manage_registry(), "view the audit log")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let from = parse_opt_ts(&q.from, "from")?;
    let to = parse_opt_ts(&q.to, "to")?;
    let rows = sqlx::query_as::<_, AuditLog>(
        "SELECT * FROM audit_log
          WHERE (? IS NULL OR created_at >= ?)
            AND (? IS NULL OR created_at <= ?)
            AND (? IS NULL OR actor = ?)
            AND (? IS NULL OR action = ?)
          ORDER BY created_at DESC LIMIT ?",
    )
    .bind(from)
    .bind(from)
    .bind(to)
    .bind(to)
    .bind(&q.actor)
    .bind(&q.actor)
    .bind(&q.action)
    .bind(&q.action)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}
