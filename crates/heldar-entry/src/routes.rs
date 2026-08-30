//! Stage 4 access-control surface: registered vehicles, visitor passes (+ check-in/out), watchlist,
//! the canonical entry-event feed with a guard confirm/reject workflow, and reports (daily entry
//! log, exceptions, audit). Reads require any authenticated principal; registry mutations require
//! manager+, gate operations require guard+, and the audit report requires manager+.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
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
use heldar_kernel::auth::{self, Cap, Principal};
use heldar_kernel::error::{AppError, AppResult};
use heldar_kernel::state::{camera_scope_filter, scope_denied_owner, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/modules/entry/ui/index.js", get(serve_ui))
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
        .route("/api/v1/entry/gate", get(get_gate_state))
        .route(
            "/api/v1/entry/gate/settings",
            axum::routing::put(put_gate_settings),
        )
        .route(
            "/api/v1/entry/gate/policies/{camera_id}",
            axum::routing::put(put_gate_policy).delete(delete_gate_policy),
        )
        .route("/api/v1/entry/gate/open/{camera_id}", post(gate_open))
}

// ---- Camera scope ---------------------------------------------------------
//
// Entry has two kinds of surface. The REGISTRY (vehicles, visitor passes, watchlist) is keyed on
// plates and people and names no camera at all, so camera scope has nothing to say about it and it is
// deliberately left alone. The LANE surface — the entry-event feed, the reports built from it, and
// gate actuation — is camera-keyed, and that is what the helpers below contain.
//
// Every helper is a discriminant compare that returns `Ok`/`None`/`true` for `Scope::All`, so every
// human role, every key minted without a camera list, and the auth-disabled LAN default (whose
// principal is the unscoped system admin) see no change by construction. None is reachable from a
// background task: the ANPR consumer and the retention loop hold no `Principal` and keep raw queries.

/// Refuse a BOX-LEVEL action to a camera-scoped credential.
///
/// The kernel's equivalent (`routes::cameras::require_fleet_scope`) is `pub(crate)` and so cannot be
/// called from an app crate; the wording is kept identical on purpose.
fn require_fleet_scope(principal: &Principal, action: &str) -> AppResult<()> {
    if principal.camera_scope().is_some() {
        return Err(AppError::Forbidden(format!(
            "credential is scoped to specific cameras and cannot {action}"
        )));
    }
    Ok(())
}

/// Assert camera scope over a resource addressed by its OWN primary key, refusing an out-of-scope
/// resource BEFORE its existence is disclosed.
///
/// This is the app-crate twin of `AppState::resource_camera` (`entry_events` is not one of the
/// kernel's `CameraOwned` tables, so the kernel loader cannot reach it).
///
/// - `Scope::All`: identical to today — the row is looked up and a missing row is the pre-existing
///   404 with its pre-existing wording. A NULL `camera_id` (a guard-recorded manual check-in, which
///   never had a lane) is not a refusal.
/// - `Scope::Cameras`: "another lane's event", "no lane at all" and "does not exist" produce the SAME
///   [`AppError`] value, byte for byte, so the event id space cannot be enumerated by probing.
async fn require_event_scope(
    pool: &sqlx::SqlitePool,
    principal: &Principal,
    id: &str,
    action: &str,
) -> AppResult<()> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT camera_id FROM entry_events WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    match &row {
        Some((Some(cam),)) if principal.camera_allowed(cam) => return Ok(()),
        // Unscoped: `camera_allowed` is always true above, so we only reach here on a NULL lane.
        Some(_) if principal.camera_scope().is_none() => return Ok(()),
        _ => {}
    }
    if principal.camera_scope().is_some() {
        return Err(scope_denied_owner("entry event", action));
    }
    Err(AppError::NotFound(format!("entry event {id} not found")))
}

/// The built entry module UI bundle, embedded at compile time (regenerate with `make module-bundles`
/// after editing `apps/web/src/modules/entry`). It imports React + the shell SDK (`@heldar/shell`) as
/// bare specifiers the dashboard's import map resolves — so this crate ships only the module's own code.
const ENTRY_UI_BUNDLE: &str = include_str!("../ui/entry.js");

/// Serve the runtime-loaded entry module UI (the dashboard imports it via `ModuleHost`). Any
/// authenticated viewer may load it — it is inert frontend code; the data it fetches is separately
/// gated by the kernel's RBAC.
#[utoipa::path(
    get, path = "/api/v1/modules/entry/ui/index.js", tag = "entry",
    operation_id = "getEntryModuleUi",
    responses(
        (status = 200, description = "The module UI bundle, as `text/javascript`"),
        (status = 403, description = "Missing `events:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn serve_ui(principal: Principal) -> AppResult<axum::response::Response> {
    principal.require_cap(Cap::EventsRead, "load the entry module UI")?;
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/javascript; charset=utf-8",
            ),
            // Stable URL; the bundle changes only on redeploy — revalidate so a kernel rebuild never
            // serves a stale module UI from the browser's heuristic cache.
            (axum::http::header::CACHE_CONTROL, "no-cache"),
        ],
        ENTRY_UI_BUNDLE,
    )
        .into_response())
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
pub struct VehicleQuery {
    plate: Option<String>,
    owner_type: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
}

/// Registered vehicles, newest first.
///
/// `plate` matches the NORMALIZED plate (case and separators folded), so `ABC 123` and `abc-123` are
/// the same query; `q` is a substring search over owner name, plate and owner reference.
#[utoipa::path(
    get, path = "/api/v1/vehicles", tag = "entry-registry",
    operation_id = "listVehicles",
    params(
        ("plate" = Option<String>, Query, description = "Exact match on the normalized plate"),
        ("owner_type" = Option<String>, Query, description = "student|staff|resident|contractor|visitor"),
        ("q" = Option<String>, Query, description = "Substring over owner name, plate and owner ref"),
        ("limit" = Option<i64>, Query, description = "1..=2000, default 200"),
    ),
    responses(
        (status = 200, description = "Matching vehicles", body = Vec<Vehicle>),
        (status = 403, description = "Missing `identity:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn list_vehicles(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<VehicleQuery>,
) -> AppResult<Json<Vec<Vehicle>>> {
    principal.require_cap(Cap::IdentityRead, "view vehicles")?;
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

/// One registered vehicle.
#[utoipa::path(
    get, path = "/api/v1/vehicles/{id}", tag = "entry-registry",
    operation_id = "getVehicle",
    params(("id" = String, Path, description = "Vehicle id")),
    responses(
        (status = 200, description = "The vehicle", body = Vehicle),
        (status = 403, description = "Missing `identity:read`", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such vehicle", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn get_vehicle(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<Vehicle>> {
    principal.require_cap(Cap::IdentityRead, "view vehicles")?;
    let v = sqlx::query_as::<_, Vehicle>("SELECT * FROM vehicles WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("vehicle {id} not found")))?;
    Ok(Json(v))
}

/// Register a vehicle.
///
/// Refused to a camera-scoped credential: the registry carries no camera column and the ANPR
/// pipeline matches on plate alone, so a row written here can auto-open EVERY barrier on the box.
#[utoipa::path(
    post, path = "/api/v1/vehicles", tag = "entry-registry",
    operation_id = "createVehicle",
    request_body = VehicleCreate,
    responses(
        (status = 201, description = "The registered vehicle", body = Vehicle),
        (status = 400, description = "Missing plate, unknown `owner_type`, or `valid_until` before `valid_from`", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn create_vehicle(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<VehicleCreate>,
) -> AppResult<(StatusCode, Json<Vehicle>)> {
    principal.require(principal.can_manage_registry(), "register vehicles")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "register vehicles")?;
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

/// Update a registered vehicle. Omitted fields keep their current value.
///
/// Refused to a camera-scoped credential, for the same reason as registering one.
#[utoipa::path(
    patch, path = "/api/v1/vehicles/{id}", tag = "entry-registry",
    operation_id = "updateVehicle",
    params(("id" = String, Path, description = "Vehicle id")),
    request_body = VehicleUpdate,
    responses(
        (status = 200, description = "The updated vehicle", body = Vehicle),
        (status = 400, description = "Empty plate, unknown `owner_type`, or `valid_until` before `valid_from`", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such vehicle", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn update_vehicle(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<VehicleUpdate>,
) -> AppResult<Json<Vehicle>> {
    principal.require(principal.can_manage_registry(), "modify vehicles")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "modify vehicles")?;
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

/// Delete a registered vehicle.
#[utoipa::path(
    delete, path = "/api/v1/vehicles/{id}", tag = "entry-registry",
    operation_id = "deleteVehicle",
    params(("id" = String, Path, description = "Vehicle id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such vehicle", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn delete_vehicle(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete vehicles")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "delete vehicles")?;
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
pub struct PassQuery {
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
}

/// Visitor passes, newest first.
#[utoipa::path(
    get, path = "/api/v1/passes", tag = "entry-registry",
    operation_id = "listVisitorPasses",
    params(
        ("status" = Option<String>, Query, description = "active|checked_in|checked_out|expired|revoked"),
        ("q" = Option<String>, Query, description = "Substring over visitor name, plate, code and host"),
        ("limit" = Option<i64>, Query, description = "1..=2000, default 200"),
    ),
    responses(
        (status = 200, description = "Matching passes", body = Vec<VisitorPass>),
        (status = 403, description = "Missing `identity:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn list_passes(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<PassQuery>,
) -> AppResult<Json<Vec<VisitorPass>>> {
    principal.require_cap(Cap::IdentityRead, "view passes")?;
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

/// One visitor pass.
#[utoipa::path(
    get, path = "/api/v1/passes/{id}", tag = "entry-registry",
    operation_id = "getVisitorPass",
    params(("id" = String, Path, description = "Pass id")),
    responses(
        (status = 200, description = "The pass", body = VisitorPass),
        (status = 403, description = "Missing `identity:read`", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such pass", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn get_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<VisitorPass>> {
    principal.require_cap(Cap::IdentityRead, "view passes")?;
    Ok(Json(load_pass(&st.pool, &id).await?))
}

async fn load_pass(pool: &sqlx::SqlitePool, id: &str) -> AppResult<VisitorPass> {
    sqlx::query_as::<_, VisitorPass>("SELECT * FROM visitor_passes WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("pass {id} not found")))
}

/// Issue a visitor pass.
///
/// The box mints `code`; `valid_from` defaults to now and `valid_until` to 24 hours out. Refused to
/// a camera-scoped credential — a pass is matched by plate on every lane, not on one.
#[utoipa::path(
    post, path = "/api/v1/passes", tag = "entry-registry",
    operation_id = "createVisitorPass",
    request_body = VisitorPassCreate,
    responses(
        (status = 201, description = "The issued pass", body = VisitorPass),
        (status = 400, description = "Missing `visitor_name`, or `valid_until` before `valid_from`", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `gate:operate`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn create_pass(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<VisitorPassCreate>,
) -> AppResult<(StatusCode, Json<VisitorPass>)> {
    principal.require(principal.can_operate_gate(), "create visitor passes")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "create visitor passes")?;
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

/// Update a visitor pass. Omitted fields keep their current value.
///
/// `revoked` is terminal for a guard: moving a pass OUT of it additionally requires
/// `registry:manage`, so a revoked pass cannot be resurrected by editing its status.
#[utoipa::path(
    patch, path = "/api/v1/passes/{id}", tag = "entry-registry",
    operation_id = "updateVisitorPass",
    params(("id" = String, Path, description = "Pass id")),
    request_body = VisitorPassUpdate,
    responses(
        (status = 200, description = "The updated pass", body = VisitorPass),
        (status = 400, description = "Unknown `status`, or `valid_until` before `valid_from`", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `gate:operate`, a camera-scoped credential, or reinstating a revoked pass without `registry:manage`", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such pass", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn update_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<VisitorPassUpdate>,
) -> AppResult<Json<VisitorPass>> {
    principal.require(principal.can_operate_gate(), "modify visitor passes")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "modify visitor passes")?;
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

/// Delete a visitor pass.
#[utoipa::path(
    delete, path = "/api/v1/passes/{id}", tag = "entry-registry",
    operation_id = "deleteVisitorPass",
    params(("id" = String, Path, description = "Pass id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such pass", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn delete_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete visitor passes")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "delete visitor passes")?;
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

/// Check a visitor in, and record it in the canonical entry feed.
///
/// Only an `active` (or already `checked_in`, so this is idempotent) pass may be checked in — a
/// revoked, expired or checked-out pass is refused rather than silently reactivated.
#[utoipa::path(
    post, path = "/api/v1/passes/{id}/checkin", tag = "entry-registry",
    operation_id = "checkInVisitorPass",
    params(("id" = String, Path, description = "Pass id")),
    responses(
        (status = 200, description = "The checked-in pass", body = VisitorPass),
        (status = 400, description = "The pass is revoked, expired or already checked out", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `gate:operate`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such pass", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn checkin_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<VisitorPass>> {
    principal.require(principal.can_operate_gate(), "check in visitors")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "check in visitors")?;
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

/// Check a visitor out, and record it in the canonical entry feed.
///
/// A `revoked` or `expired` pass is refused: flipping it to `checked_out` would leave it checkable
/// back in.
#[utoipa::path(
    post, path = "/api/v1/passes/{id}/checkout", tag = "entry-registry",
    operation_id = "checkOutVisitorPass",
    params(("id" = String, Path, description = "Pass id")),
    responses(
        (status = 200, description = "The checked-out pass", body = VisitorPass),
        (status = 400, description = "The pass is revoked or expired", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `gate:operate`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such pass", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn checkout_pass(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<VisitorPass>> {
    principal.require(principal.can_operate_gate(), "check out visitors")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "check out visitors")?;
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

/// The plate watchlist, newest first. Capped at 1000 rows — it is an operator view, not an export.
#[utoipa::path(
    get, path = "/api/v1/watchlist", tag = "entry-registry",
    operation_id = "listWatchlist",
    responses(
        (status = 200, description = "Watchlist entries", body = Vec<Watchlist>),
        (status = 403, description = "Missing `identity:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn list_watchlist(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<Watchlist>>> {
    principal.require_cap(Cap::IdentityRead, "view watchlist")?;
    // Bound the result set: the watchlist can grow large, and an unbounded SELECT * would load every
    // row into memory at once (OOM/latency risk). 1000 is well above any realistic operator view.
    let rows = sqlx::query_as::<_, Watchlist>(
        "SELECT * FROM watchlist ORDER BY created_at DESC LIMIT 1000",
    )
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

/// Add a plate to the watchlist.
///
/// Refused to a camera-scoped credential: the ANPR pipeline reads the watchlist by plate alone, so
/// an entry here acts on every lane on the box.
#[utoipa::path(
    post, path = "/api/v1/watchlist", tag = "entry-registry",
    operation_id = "createWatchlistEntry",
    request_body = WatchlistCreate,
    responses(
        (status = 201, description = "The watchlist entry", body = Watchlist),
        (status = 400, description = "Missing plate, or unknown `kind`/`severity`", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn create_watch(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<WatchlistCreate>,
) -> AppResult<(StatusCode, Json<Watchlist>)> {
    principal.require(principal.can_manage_registry(), "manage the watchlist")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "manage the watchlist")?;
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

/// Update a watchlist entry. The plate itself is immutable — delete and re-add to change it.
#[utoipa::path(
    patch, path = "/api/v1/watchlist/{id}", tag = "entry-registry",
    operation_id = "updateWatchlistEntry",
    params(("id" = String, Path, description = "Watchlist entry id")),
    request_body = WatchlistUpdate,
    responses(
        (status = 200, description = "The updated entry", body = Watchlist),
        (status = 400, description = "Unknown `kind` or `severity`", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such watchlist entry", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn update_watch(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<WatchlistUpdate>,
) -> AppResult<Json<Watchlist>> {
    principal.require(principal.can_manage_registry(), "manage the watchlist")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "manage the watchlist")?;
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

/// Remove a watchlist entry.
#[utoipa::path(
    delete, path = "/api/v1/watchlist/{id}", tag = "entry-registry",
    operation_id = "deleteWatchlistEntry",
    params(("id" = String, Path, description = "Watchlist entry id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such watchlist entry", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn delete_watch(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "manage the watchlist")?;
    // The identity registry has NO camera column, and the ANPR pipeline reads it by plate alone
    // (`anpr.rs` looks up watchlist/vehicles/visitor_passes with no camera predicate) before it can
    // auto-open a barrier. So a row written here acts on EVERY camera on the box: the direct
    // actuators are scoped, but this is the indirect path into the same relay. Nothing about a
    // registry row is scopable, so a camera-scoped credential is refused outright.
    heldar_kernel::routes::cameras::require_fleet_scope(&principal, "manage the watchlist")?;
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
pub struct EntryEventQuery {
    from: Option<String>,
    to: Option<String>,
    plate: Option<String>,
    auth_status: Option<String>,
    workflow_status: Option<String>,
    event_type: Option<String>,
    limit: Option<i64>,
}

/// The canonical entry-event feed, newest first.
///
/// A camera-scoped credential sees only its own lanes. Guard-recorded manual check-ins carry no lane
/// at all, so they are absent from a scoped caller's feed — the fail-closed answer, not an omission.
#[utoipa::path(
    get, path = "/api/v1/entry-events", tag = "entry",
    operation_id = "listEntryEvents",
    params(
        ("from" = Option<String>, Query, description = "RFC3339 lower bound (inclusive)"),
        ("to" = Option<String>, Query, description = "RFC3339 upper bound (inclusive)"),
        ("plate" = Option<String>, Query, description = "Exact match on the normalized plate"),
        ("auth_status" = Option<String>, Query, description = "matched|blocked|exception|unmatched"),
        ("workflow_status" = Option<String>, Query, description = "pending|confirmed|rejected"),
        ("event_type" = Option<String>, Query, description = "e.g. anpr, visitor_checkin, visitor_checkout"),
        ("limit" = Option<i64>, Query, description = "1..=5000, default 200"),
    ),
    responses(
        (status = 200, description = "Matching entry events, newest first"),
        (status = 400, description = "Invalid `from`/`to` timestamp", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `events:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn list_entry_events(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<EntryEventQuery>,
) -> AppResult<Json<Vec<EntryEvent>>> {
    principal.require_cap(Cap::EventsRead, "view entry events")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let from = parse_opt_ts(&q.from, "from")?;
    let to = parse_opt_ts(&q.to, "to")?;
    let plate_norm = q.plate.as_deref().map(normalize_plate);
    // The canonical feed carries `camera_id` on every ANPR event, so unfiltered it is the lane roster
    // indexed by time. `IN (…)` excludes the NULL `camera_id` of a guard-recorded manual check-in,
    // which is the fail-closed answer for a scoped caller; unscoped callers get no predicate at all.
    let scope = camera_scope_filter(&principal, "camera_id");
    let mut sql = "SELECT * FROM entry_events
          WHERE (? IS NULL OR timestamp >= ?)
            AND (? IS NULL OR timestamp <= ?)
            AND (? IS NULL OR plate = ?)
            AND (? IS NULL OR auth_status = ?)
            AND (? IS NULL OR workflow_status = ?)
            AND (? IS NULL OR event_type = ?)"
        .to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, EntryEvent>(&sql)
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
        .bind(&q.event_type);
    // Bind from the RETURNED vector, never from `camera_scope()`: the empty-allowlist arm is
    // `" AND 0"` with ZERO binds, and iterating the scope instead would desync the parameter count.
    // Bound TWICE, in predicate order: once for the subject arm, once for the `camera_ids`
    // containment arm added alongside it.
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    let rows = query.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

/// One entry event.
///
/// For a camera-scoped credential another lane's event, a lane-less event and an event that never
/// existed all return the SAME refusal, byte for byte — the event id space is not enumerable.
#[utoipa::path(
    get, path = "/api/v1/entry-events/{id}", tag = "entry",
    operation_id = "getEntryEvent",
    params(("id" = String, Path, description = "Entry event id")),
    responses(
        (status = 200, description = "The entry event"),
        (status = 403, description = "Missing `events:read`, or an event outside this credential's lanes", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such entry event", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn get_entry_event(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<EntryEvent>> {
    principal.require_cap(Cap::EventsRead, "view entry events")?;
    // Before the row is loaded, so the refusal precedes disclosure and a scoped credential cannot
    // tell "another lane's event" from "no such event".
    require_event_scope(&st.pool, &principal, &id, "view entry events").await?;
    let ev = sqlx::query_as::<_, EntryEvent>("SELECT * FROM entry_events WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("entry event {id} not found")))?;
    Ok(Json(ev))
}

#[derive(Debug, Deserialize, Default, utoipa::ToSchema)]
pub struct ResolveBody {
    note: Option<String>,
}

/// Confirm an entry event: a durable, attributed guard judgement recorded on its workflow.
#[utoipa::path(
    post, path = "/api/v1/entry-events/{id}/confirm", tag = "entry",
    operation_id = "confirmEntryEvent",
    params(("id" = String, Path, description = "Entry event id")),
    request_body(content = ResolveBody, description = "Optional resolution note"),
    responses(
        (status = 200, description = "The resolved entry event"),
        (status = 403, description = "Missing `gate:operate`, or an event outside this credential's lanes", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such entry event", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn confirm_event(
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

/// Reject an entry event: a durable, attributed guard judgement recorded on its workflow.
#[utoipa::path(
    post, path = "/api/v1/entry-events/{id}/reject", tag = "entry",
    operation_id = "rejectEntryEvent",
    params(("id" = String, Path, description = "Entry event id")),
    request_body(content = ResolveBody, description = "Optional resolution note"),
    responses(
        (status = 200, description = "The resolved entry event"),
        (status = 403, description = "Missing `gate:operate`, or an event outside this credential's lanes", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such entry event", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn reject_event(
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
    // A confirm/reject is a durable, attributed judgement on a lane's traffic — scope it before the
    // row is read, so the pre-existing 404 cannot map the event id space either.
    require_event_scope(&st.pool, &principal, &id, "resolve entry events").await?;
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
pub struct ReportQuery {
    date: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<i64>,
    /// The IANA zone `date=` is a calendar day IN. Omit and it resolves from the site/box (#125).
    tz: Option<String>,
}

/// The window, and the clock it was computed on.
struct Window {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    tz: chrono_tz::Tz,
    tz_source: &'static str,
    /// True when `date=` (or the implicit today) was used — i.e. when the zone actually mattered.
    calendar_day: bool,
}

impl Window {
    /// What clock this report was computed on, echoed in every response.
    ///
    /// A shifted total looks exactly like a correct one, so the answer has to state its own basis.
    /// This matters most on the first run after a zone is configured: a `date=` report that used to
    /// cover 08:00→08:00 local at a UTC+8 site now covers 00:00→00:00, and the numbers change with
    /// no error. They SHOULD change — that is the fix — but an operator re-running last month's
    /// compliance export deserves to see why.
    fn interpretation(&self) -> Value {
        json!({
            "timezone": self.tz.to_string(),
            "timezone_source": self.tz_source,
            "calendar_day_in": if self.calendar_day { Some(self.tz.to_string()) } else { None },
            "note": "`date` is a calendar day in this zone; `from`/`to` and every timestamp below \
                     are UTC.",
        })
    }
}

/// Resolve a [from, to) window from either an explicit from/to or a `date=YYYY-MM-DD`.
///
/// A calendar day is a wall-clock notion, so `date=` needs a zone. It used to be resolved as the UTC
/// day, which after #125 made "yesterday" in search and "yesterday" in this report two different 24
/// hours on the same box.
///
/// Absolute `from`/`to` are unambiguous instants and are untouched — the zone is irrelevant to them,
/// so they are not refused on a cross-zone box either.
async fn report_window(st: &AppState, principal: &Principal, q: &ReportQuery) -> AppResult<Window> {
    let explicit = q.tz.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (tz, tz_source) = match explicit {
        Some(raw) => (
            heldar_kernel::services::tz::parse(raw).ok_or_else(|| {
                AppError::BadRequest(format!(
                    "`tz` must be an IANA timezone identifier such as `Asia/Kuala_Lumpur` \
                     (got {raw:?})"
                ))
            })?,
            "explicit",
        ),
        None => {
            let cams: Vec<String> = match &principal.scope {
                heldar_kernel::auth::Scope::Cameras(set) => set.iter().cloned().collect(),
                heldar_kernel::auth::Scope::All => Vec::new(),
            };
            let (zones, from_site) = heldar_kernel::services::tz::zones_for(&st.pool, &cams).await;
            if zones.len() > 1 {
                // Only when the zone actually decides the window. Absolute from/to mean the same
                // instants everywhere, so refusing those would be obstruction.
                if q.from.is_none() && q.to.is_none() {
                    return Err(AppError::BadRequest(format!(
                        "a calendar day means something different at each of this report's sites \
                         ({}). Pass an explicit `tz`, or use absolute `from`/`to` — resolving it \
                         silently would shift the totals by hours.",
                        zones.into_iter().collect::<Vec<_>>().join(", ")
                    )));
                }
                (chrono_tz::Tz::UTC, "not_a_calendar_day")
            } else {
                let one = zones
                    .into_iter()
                    .next()
                    .and_then(|z| heldar_kernel::services::tz::parse(&z));
                // The source is what was actually consulted, not something inferred from the
                // value: "the box default happens to be UTC" and "nothing is configured" produce
                // the same zone and mean different things to an operator reading the report.
                let (boxwide, _) = heldar_kernel::services::tz::site_tz(&st.pool, None).await;
                match one {
                    Some(tz) if from_site => (tz, "site"),
                    Some(tz) if boxwide.is_some() => (tz, "default"),
                    Some(tz) => (tz, "utc_fallback"),
                    None => (chrono_tz::Tz::UTC, "utc_fallback"),
                }
            }
        }
    };

    if q.from.is_some() || q.to.is_some() {
        let from = parse_opt_ts(&q.from, "from")?.unwrap_or_else(|| Utc::now() - Duration::days(1));
        let to = parse_opt_ts(&q.to, "to")?.unwrap_or_else(Utc::now);
        if to < from {
            return Err(AppError::BadRequest("`to` must not precede `from`".into()));
        }
        return Ok(Window {
            from,
            to,
            tz,
            tz_source,
            calendar_day: false,
        });
    }

    let day = match &q.date {
        Some(d) => chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d")
            .map_err(|_| AppError::BadRequest("`date` must be YYYY-MM-DD".into()))?,
        None => Utc::now().with_timezone(&tz).date_naive(),
    };
    let naive = day
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| AppError::BadRequest("invalid date".into()))?;
    // `from_wall_clock` resolves the two days a year where a local midnight is skipped or repeated.
    let from = heldar_kernel::services::tz::from_wall_clock(tz, naive);
    let to = heldar_kernel::services::tz::from_wall_clock(
        tz,
        (day + Duration::days(1))
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| AppError::BadRequest("invalid date".into()))?,
    );
    Ok(Window {
        from,
        to,
        tz,
        tz_source,
        calendar_day: true,
    })
}

/// Daily entry log: the events in a window, plus a count by authorization status.
///
/// `date` is a CALENDAR day, which only means something in a timezone, so every response echoes the
/// zone it used under `interpretation`. On a box whose sites span more than one zone, `date` is
/// refused rather than silently resolved — pass `tz`, or absolute `from`/`to`, which are instants
/// and need no zone. The aggregate is scope-filtered too, so it cannot report traffic on lanes this
/// credential does not hold.
#[utoipa::path(
    get, path = "/api/v1/reports/entry-log", tag = "entry",
    operation_id = "getEntryLogReport",
    params(
        ("date" = Option<String>, Query, description = "YYYY-MM-DD, a calendar day in `tz`. Defaults to today when no from/to is given"),
        ("from" = Option<String>, Query, description = "RFC3339 lower bound; overrides `date`"),
        ("to" = Option<String>, Query, description = "RFC3339 upper bound; overrides `date`"),
        ("tz" = Option<String>, Query, description = "IANA zone `date` is read in. Omit to resolve from the site, then the box default"),
        ("limit" = Option<i64>, Query, description = "1..=10000, default 1000"),
    ),
    responses(
        (status = 200, description = "The window, the zone it was computed in, the events and the counts"),
        (status = 400, description = "Bad `date`/`tz`/timestamp, `to` before `from`, or a calendar day across several site zones", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `events:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn report_entry_log(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<ReportQuery>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "view reports")?;
    let w = report_window(&st, &principal, &q).await?;
    let (from, to) = (w.from, w.to);
    let limit = q.limit.unwrap_or(1000).clamp(1, 10000);
    // The daily log is the same rows as the feed, so it gets the same predicate — otherwise the
    // report is a trivial bypass of `list_entry_events`.
    let scope = camera_scope_filter(&principal, "camera_id");
    let mut sql = "SELECT * FROM entry_events WHERE timestamp >= ? AND timestamp < ?".to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, EntryEvent>(&sql).bind(from).bind(to);
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    let events = query.bind(limit).fetch_all(&st.pool).await?;
    let counts = auth_status_counts(&st.pool, &principal, from, to).await?;
    Ok(Json(json!({
        "from": from, "to": to,
        "interpretation": w.interpretation(),
        "total": events.len(),
        "by_auth_status": counts,
        "events": events,
    })))
}

/// Entry exceptions in a window: blocked, exception and unmatched events, plus anything a guard
/// rejected.
///
/// Same window and timezone rules as the entry log.
#[utoipa::path(
    get, path = "/api/v1/reports/exceptions", tag = "entry",
    operation_id = "getEntryExceptionsReport",
    params(
        ("date" = Option<String>, Query, description = "YYYY-MM-DD, a calendar day in `tz`. Defaults to today when no from/to is given"),
        ("from" = Option<String>, Query, description = "RFC3339 lower bound; overrides `date`"),
        ("to" = Option<String>, Query, description = "RFC3339 upper bound; overrides `date`"),
        ("tz" = Option<String>, Query, description = "IANA zone `date` is read in. Omit to resolve from the site, then the box default"),
        ("limit" = Option<i64>, Query, description = "1..=10000, default 1000"),
    ),
    responses(
        (status = 200, description = "The window, the zone it was computed in, and the exception events"),
        (status = 400, description = "Bad `date`/`tz`/timestamp, `to` before `from`, or a calendar day across several site zones", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `events:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn report_exceptions(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<ReportQuery>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "view reports")?;
    let w = report_window(&st, &principal, &q).await?;
    let (from, to) = (w.from, w.to);
    let limit = q.limit.unwrap_or(1000).clamp(1, 10000);
    // Exceptions = anything that is not an automatic clean match: blocked / exception / unmatched,
    // plus any event a guard explicitly rejected.
    let scope = camera_scope_filter(&principal, "camera_id");
    let mut sql = "SELECT * FROM entry_events
          WHERE timestamp >= ? AND timestamp < ?
            AND (auth_status IN ('blocked','exception','unmatched') OR workflow_status = 'rejected')"
        .to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, EntryEvent>(&sql).bind(from).bind(to);
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    let events = query.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(json!({
        "from": from, "to": to,
        "interpretation": w.interpretation(),
        "total": events.len(),
        "events": events,
    })))
}

async fn auth_status_counts(
    pool: &sqlx::SqlitePool,
    principal: &Principal,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<Value> {
    // The aggregate is filtered too: an unfiltered count would report how much traffic the lanes this
    // credential does NOT hold saw, which is the roster's size and shape without its names.
    let scope = camera_scope_filter(principal, "camera_id");
    let mut sql = "SELECT auth_status, COUNT(*) FROM entry_events
          WHERE timestamp >= ? AND timestamp < ?"
        .to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" GROUP BY auth_status");
    let mut query = sqlx::query_as::<_, (String, i64)>(&sql).bind(from).bind(to);
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    let rows: Vec<(String, i64)> = query.fetch_all(pool).await?;
    let mut map = serde_json::Map::new();
    for (k, v) in rows {
        map.insert(k, json!(v));
    }
    Ok(Value::Object(map))
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    from: Option<String>,
    to: Option<String>,
    actor: Option<String>,
    action: Option<String>,
    limit: Option<i64>,
}

/// The audit log — who did what — newest first.
///
/// Manager-gated because it reveals operator activity. A camera-scoped credential sees only rows
/// whose derived subject camera it holds, plus multi-camera acts where it holds EVERY camera named;
/// fleet-level rows naming no camera are hidden from it.
#[utoipa::path(
    get, path = "/api/v1/audit", tag = "entry",
    operation_id = "listAuditLog",
    params(
        ("from" = Option<String>, Query, description = "RFC3339 lower bound (inclusive)"),
        ("to" = Option<String>, Query, description = "RFC3339 upper bound (inclusive)"),
        ("actor" = Option<String>, Query, description = "Exact principal id"),
        ("action" = Option<String>, Query, description = "Exact action slug, e.g. `gate_manual_open`"),
        ("limit" = Option<i64>, Query, description = "1..=5000, default 200"),
    ),
    responses(
        (status = 200, description = "Audit rows visible to this credential, newest first"),
        (status = 400, description = "Invalid `from`/`to` timestamp", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn list_audit(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<Vec<AuditLog>>> {
    // The audit log records who did what — restricted to manager+ (it can reveal operator activity).
    principal.require(principal.can_manage_registry(), "view the audit log")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let from = parse_opt_ts(&q.from, "from")?;
    let to = parse_opt_ts(&q.to, "to")?;
    // Scoped on `subject_camera_id` (kernel migration 0014), the column `crate::auth::audit` derives
    // for every row it writes. The predecessor filtered `target_id` and only where `target_type =
    // 'camera'`, which masked gate rows and let EVERY other row through — while zones, ai_task,
    // camera_schedule, snapshot_schedule and recording_gap all name their camera in the free-form
    // `detail` JSON under a different target_type. `?limit=5000` was therefore the fleet roster plus
    // which cameras carry zones, AI tasks and schedules. Scope cannot be enforced over schemaless
    // JSON; a derived column is the only shape a predicate can hold on to.
    //
    // Fail-closed: for a scoped caller a NULL subject (fleet-level, or about no camera at all) is
    // HIDDEN, not shown. `IN (…)` already drops NULLs — `IS NOT NULL` is stated anyway so the
    // intent survives any future change to the predicate builder. Unscoped callers, which is every
    // human role and every key minted without a camera list, get no predicate at all and read the
    // whole log exactly as before.
    let scope = camera_scope_filter(&principal, "subject_camera_id");
    let mut sql = "SELECT * FROM audit_log
          WHERE (? IS NULL OR created_at >= ?)
            AND (? IS NULL OR created_at <= ?)
            AND (? IS NULL OR actor = ?)
            AND (? IS NULL OR action = ?)"
        .to_string();
    if let Some((pred, binds)) = &scope {
        // A row is visible when its derived subject is held, OR — for an act naming SEVERAL cameras,
        // where one column cannot say "both ends" and the subject is therefore NULL — when every
        // camera it names is held.
        //
        // Without the second arm a credential holding BOTH ends of a movement link could create and
        // delete that link (both allowed, it owns both cameras) and then find its own acts absent
        // from its own audit trail. The reason multi-camera rows carry no subject is that naming
        // ONE end would disclose adjacency to a half-holder — an argument that says nothing about a
        // caller who already holds every camera involved and performed the act itself.
        //
        // Still fail-closed: `json_each` over `detail.camera_ids` yields nothing for a NULL, absent
        // or non-array value, so `NOT EXISTS(... NOT IN scope)` is paired with a non-empty test
        // rather than standing alone — otherwise every subject-less row would match.
        //
        // `json_valid` sits inside a CASE, not beside the other AND terms: AND has no guaranteed
        // evaluation order and `json_type` RAISES on a malformed blob, which would turn one
        // hand-edited row into a 500 for the whole endpoint. Same guard, same reason, as the backfill
        // in migration 0014.
        let placeholders = vec!["?"; binds.len()].join(",");
        sql.push_str(&format!(
            " AND ((subject_camera_id IS NOT NULL{pred})
                   OR (subject_camera_id IS NULL
                       AND CASE WHEN json_valid(detail)
                                THEN json_type(detail, '$.camera_ids') END = 'array'
                       AND json_array_length(detail, '$.camera_ids') > 0
                       AND NOT EXISTS (
                             SELECT 1 FROM json_each(detail, '$.camera_ids')
                              WHERE json_each.value NOT IN ({placeholders}))))"
        ));
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, AuditLog>(&sql)
        .bind(from)
        .bind(from)
        .bind(to)
        .bind(to)
        .bind(&q.actor)
        .bind(&q.actor)
        .bind(&q.action)
        .bind(&q.action);
    // Bound TWICE, in predicate order: once for the subject arm, once for the `camera_ids`
    // containment arm added alongside it.
    for id in scope
        .iter()
        .flat_map(|(_, ids)| ids)
        .chain(scope.iter().flat_map(|(_, ids)| ids))
    {
        query = query.bind(id);
    }
    let rows = query.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

// ---- Gate actuation (issue #44) -------------------------------------------

/// Full gate state: the global kill-switch plus every lane policy this credential may see.
///
/// The policy list is confined to the caller's cameras — unfiltered it is the roster of every camera
/// wired to a barrier, together with its relay port.
#[utoipa::path(
    get, path = "/api/v1/entry/gate", tag = "entry-gate",
    operation_id = "getGateState",
    responses(
        (status = 200, description = "`kill_switch` plus the lane policies this credential may see"),
        (status = 403, description = "Missing `identity:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn get_gate_state(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::IdentityRead, "view gate configuration")?;
    let kill_switch = crate::gate::GateActuator::kill_switch(&st.pool).await;
    // One row per configured LANE, keyed by camera id — an unfiltered list is the roster of every
    // camera wired to a barrier, plus its relay port. Unscoped callers see today's full list.
    let scope = camera_scope_filter(&principal, "camera_id");
    let mut sql = "SELECT * FROM gate_policies WHERE 1=1".to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY camera_id ASC");
    let mut query = sqlx::query_as::<_, crate::gate::GatePolicy>(&sql);
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    let policies = query.fetch_all(&st.pool).await?;
    Ok(Json(
        json!({ "kill_switch": kill_switch, "policies": policies }),
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct GateSettingsUpdate {
    kill_switch: bool,
}

/// Flip the global kill-switch, which halts (or re-enables) ALL actuation, automatic and manual.
///
/// Box-level: there is no camera id to scope it by, so a camera-scoped credential is refused rather
/// than allowed to freeze — or unfreeze — barriers it does not hold.
#[utoipa::path(
    put, path = "/api/v1/entry/gate/settings", tag = "entry-gate",
    operation_id = "updateGateSettings",
    request_body = GateSettingsUpdate,
    responses(
        (status = 200, description = "The kill-switch as it now stands"),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn put_gate_settings(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<GateSettingsUpdate>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "change gate settings")?;
    // BOX-LEVEL and physical: the kill-switch halts (or re-enables) actuation on EVERY lane on the
    // box, and there is no camera id to scope it by. A per-building credential must not be able to
    // freeze — or unfreeze — barriers it does not hold, so containment here is a refusal.
    require_fleet_scope(&principal, "change the global gate kill-switch")?;
    sqlx::query("UPDATE gate_settings SET kill_switch = ?, updated_at = ? WHERE id = 1")
        .bind(body.kill_switch)
        .bind(Utc::now())
        .execute(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "gate_put_settings",
        "gate",
        "global",
        json!({ "kill_switch": body.kill_switch }),
    )
    .await;
    Ok(Json(json!({ "kill_switch": body.kill_switch })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct GatePolicyUpdate {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    output_port: Option<i64>,
    #[serde(default)]
    pulse_ms: Option<i64>,
}

/// Upsert a camera's gate policy: the auto-open flag, the relay output port and the pulse width.
///
/// Omitted fields keep their current value; `output_port` is floored at 1 and `pulse_ms` clamped to
/// 100..=30000. A camera this credential does not hold is 403; a camera the box does not know, 404.
#[utoipa::path(
    put, path = "/api/v1/entry/gate/policies/{camera_id}", tag = "entry-gate",
    operation_id = "putGatePolicy",
    params(("camera_id" = String, Path, description = "Camera id of the lane")),
    request_body = GatePolicyUpdate,
    responses(
        (status = 200, description = "The stored policy", body = crate::gate::GatePolicy),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such camera", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn put_gate_policy(
    State(st): State<AppState>,
    Path(camera_id): Path<String>,
    principal: Principal,
    Json(body): Json<GatePolicyUpdate>,
) -> AppResult<Json<crate::gate::GatePolicy>> {
    principal.require(principal.can_manage_registry(), "configure gate policies")?;
    // BEFORE the existence probe below, which would otherwise answer 404 for a camera that is not on
    // the box and 200 for one that is — an id-space oracle over the whole fleet. The id is the
    // caller's own path input, so naming it in the refusal discloses nothing it did not already say.
    principal.require_camera(&camera_id, "configure gate policies")?;
    // The camera must exist (a policy against a ghost camera can never actuate).
    let known: Option<(String,)> = sqlx::query_as("SELECT id FROM cameras WHERE id = ?")
        .bind(&camera_id)
        .fetch_optional(&st.pool)
        .await?;
    if known.is_none() {
        return Err(AppError::NotFound(format!("camera {camera_id} not found")));
    }
    let cur = crate::gate::GateActuator::policy(&st.pool, &camera_id).await;
    let enabled = body
        .enabled
        .or(cur.as_ref().map(|p| p.enabled))
        .unwrap_or(false);
    let output_port = body
        .output_port
        .or(cur.as_ref().map(|p| p.output_port))
        .unwrap_or(1)
        .max(1);
    let pulse_ms = body
        .pulse_ms
        .or(cur.as_ref().map(|p| p.pulse_ms))
        .unwrap_or(1000)
        .clamp(100, 30_000);
    sqlx::query(
        "INSERT INTO gate_policies (camera_id, enabled, output_port, pulse_ms, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            enabled = excluded.enabled,
            output_port = excluded.output_port,
            pulse_ms = excluded.pulse_ms,
            updated_at = excluded.updated_at",
    )
    .bind(&camera_id)
    .bind(enabled)
    .bind(output_port)
    .bind(pulse_ms)
    .bind(Utc::now())
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "gate_put_policy",
        "camera",
        &camera_id,
        json!({ "enabled": enabled, "output_port": output_port, "pulse_ms": pulse_ms }),
    )
    .await;
    let policy = crate::gate::GateActuator::policy(&st.pool, &camera_id)
        .await
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("gate policy write not visible")))?;
    Ok(Json(policy))
}

/// Remove a camera's gate policy, disabling both auto-open and manual open for that lane.
#[utoipa::path(
    delete, path = "/api/v1/entry/gate/policies/{camera_id}", tag = "entry-gate",
    operation_id = "deleteGatePolicy",
    params(("camera_id" = String, Path, description = "Camera id of the lane")),
    responses(
        (status = 204, description = "Policy removed"),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "That camera has no gate policy", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn delete_gate_policy(
    State(st): State<AppState>,
    Path(camera_id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "remove gate policies")?;
    // Before the DELETE: removing another lane's policy disables its auto-open, and the 204-vs-404
    // split would otherwise report which cameras have a barrier configured.
    principal.require_camera(&camera_id, "remove gate policies")?;
    let res = sqlx::query("DELETE FROM gate_policies WHERE camera_id = ?")
        .bind(&camera_id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "no gate policy for camera {camera_id}"
        )));
    }
    auth::audit(
        &st.pool,
        &principal,
        "gate_delete_policy",
        "camera",
        &camera_id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Pulse a lane's configured relay now — a PHYSICAL-WORLD side effect, audited with the acting
/// principal.
///
/// 400, not 500, when the lane has no policy, when the kill-switch is on, or when the device itself
/// refuses the pulse: a camera with no relay port has to say so in words an operator can act on.
#[utoipa::path(
    post, path = "/api/v1/entry/gate/open/{camera_id}", tag = "entry-gate",
    operation_id = "openGate",
    params(("camera_id" = String, Path, description = "Camera id of the lane")),
    responses(
        (status = 200, description = "Pulsed; the body reports the pulse width actually used"),
        (status = 400, description = "No policy on this lane, the kill-switch is on, or the device refused the pulse", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `gate:operate`, or a camera this credential does not hold", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn gate_open(
    State(st): State<AppState>,
    Path(camera_id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_operate_gate(), "open the gate")?;
    // The sharpest route in this crate: a PHYSICAL-WORLD side effect on a named lane. Checked before
    // the actuator is built, so a credential scoped to one building cannot raise another's barrier,
    // and before `manual_open`'s own BadRequest/NotFound would report whether a lane has a policy.
    principal.require_camera(&camera_id, "open the gate")?;
    let actuator = crate::gate::GateActuator::new(st.pool.clone(), st.http.clone(), st.cfg.clone());
    let pulse_ms = actuator.manual_open(&camera_id, &principal.id).await?;
    auth::audit(
        &st.pool,
        &principal,
        "gate_manual_open",
        "camera",
        &camera_id,
        json!({ "pulse_ms": pulse_ms }),
    )
    .await;
    Ok(Json(json!({ "ok": true, "pulse_ms": pulse_ms })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use heldar_kernel::auth::Scope;
    use std::collections::HashSet;
    use std::sync::Arc;

    /// A camera-scoped credential holding every capability — the attacker in the audit report. Only
    /// `scope` differs from the auth-disabled system admin, so any behaviour difference below is
    /// attributable to camera scope and to nothing else.
    fn scoped(cameras: &[&str]) -> Principal {
        let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: Scope::Cameras(Arc::new(set)),
            ..Principal::system_admin()
        }
    }

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        heldar_kernel::db::run_migrations(&pool).await.unwrap();
        crate::schema::init(&pool).await.unwrap();
        let cfg = Arc::new(heldar_kernel::config::Config::from_env());
        AppState {
            recorder: heldar_kernel::services::recorder::RecorderManager::new(
                pool.clone(),
                cfg.clone(),
            ),
            sampler: heldar_kernel::services::sampler::SamplerManager::new(
                pool.clone(),
                cfg.clone(),
            ),
            live: heldar_kernel::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                heldar_kernel::reqwest::Client::new(),
            ),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(heldar_kernel::services::registry::CatalogService::new(&cfg)),
            http: heldar_kernel::reqwest::Client::new(),
            media_jobs: heldar_kernel::services::media_jobs::MediaJobGovernor::new(2),
            started_at: Utc::now(),
            pool,
            cfg,
        }
    }

    async fn camera(pool: &sqlx::SqlitePool, id: &str) {
        let now = Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
            .bind(id)
            .bind(id)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn event(pool: &sqlx::SqlitePool, id: &str, cam: Option<&str>) {
        sqlx::query(
            "INSERT INTO entry_events (id, camera_id, event_type, timestamp, direction, plate,
                subject, authorization, auth_status, evidence, workflow_status, workflow, audit, created_at)
             VALUES (?, ?, 'anpr', ?, 'inbound', 'ABC123', '{}', '{}', 'matched', '{}', 'pending', '{}', '{}', ?)",
        )
        .bind(id)
        .bind(cam)
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    async fn policy(pool: &sqlx::SqlitePool, cam: &str) {
        sqlx::query(
            "INSERT INTO gate_policies (camera_id, enabled, output_port, pulse_ms, updated_at)
             VALUES (?, 1, 1, 1000, ?)",
        )
        .bind(cam)
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn require_fleet_scope_is_a_no_op_for_an_unscoped_principal() {
        // CONSTRAINT 1 + 2: auth-disabled and every human role are untouched.
        assert!(require_fleet_scope(&Principal::system_admin(), "flip").is_ok());
        // Any camera scope, INCLUDING an empty one, is still a scope and is refused.
        assert!(require_fleet_scope(&scoped(&["cam_a"]), "flip").is_err());
        assert!(require_fleet_scope(&scoped(&[]), "flip").is_err());
    }

    #[tokio::test]
    async fn require_event_scope_is_unchanged_for_an_unscoped_principal() {
        let st = test_state().await;
        event(&st.pool, "evt_a", Some("cam_a")).await;
        event(&st.pool, "evt_manual", None).await;
        let admin = Principal::system_admin();
        assert!(require_event_scope(&st.pool, &admin, "evt_a", "view")
            .await
            .is_ok());
        // A guard-recorded manual check-in has no lane and has never been a refusal.
        assert!(require_event_scope(&st.pool, &admin, "evt_manual", "view")
            .await
            .is_ok());
        let err = require_event_scope(&st.pool, &admin, "evt_zzz", "view")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
        assert!(err.to_string().contains("entry event evt_zzz not found"));
    }

    #[tokio::test]
    async fn require_event_scope_never_becomes_an_existence_oracle() {
        let st = test_state().await;
        event(&st.pool, "evt_a", Some("cam_a")).await;
        event(&st.pool, "evt_b", Some("cam_SENTINEL_B")).await;
        event(&st.pool, "evt_manual", None).await;
        let p = scoped(&["cam_a"]);

        assert!(require_event_scope(&st.pool, &p, "evt_a", "view")
            .await
            .is_ok());
        // Another lane's event, a lane-less event, and one that never existed are indistinguishable.
        let other = require_event_scope(&st.pool, &p, "evt_b", "view")
            .await
            .unwrap_err();
        let laneless = require_event_scope(&st.pool, &p, "evt_manual", "view")
            .await
            .unwrap_err();
        let ghost = require_event_scope(&st.pool, &p, "evt_zzz", "view")
            .await
            .unwrap_err();
        assert!(matches!(other, AppError::Forbidden(_)), "got {other:?}");
        assert_eq!(other.to_string(), laneless.to_string());
        assert_eq!(other.to_string(), ghost.to_string());
        let msg = other.to_string();
        assert!(!msg.contains("cam_SENTINEL_B"), "{msg}");
        assert!(!msg.contains("evt_b"), "{msg}");
    }

    #[tokio::test]
    async fn the_entry_feed_and_its_reports_are_confined_to_the_credentials_lanes() {
        let st = test_state().await;
        event(&st.pool, "evt_a", Some("cam_a")).await;
        event(&st.pool, "evt_b", Some("cam_SENTINEL_B")).await;
        event(&st.pool, "evt_manual", None).await;
        let p = scoped(&["cam_a"]);
        let eq = || EntryEventQuery {
            from: None,
            to: None,
            plate: None,
            auth_status: None,
            workflow_status: None,
            event_type: None,
            limit: None,
        };
        let rq = || ReportQuery {
            date: None,
            from: Some((Utc::now() - Duration::days(1)).to_rfc3339()),
            to: Some((Utc::now() + Duration::days(1)).to_rfc3339()),
            limit: None,
            // Absolute from/to, so the zone does not decide this window (#125).
            tz: None,
        };

        let feed = list_entry_events(State(st.clone()), p.clone(), Query(eq()))
            .await
            .unwrap();
        assert_eq!(feed.0.len(), 1);
        assert_eq!(feed.0[0].id, "evt_a");
        // Roster containment over the SERIALIZED body: the sentinel must appear nowhere.
        assert!(!serde_json::to_string(&feed.0)
            .unwrap()
            .contains("cam_SENTINEL_B"));

        // The report is the same rows, so it must not be a bypass — including its aggregate, which
        // would otherwise report how much traffic the unheld lanes saw.
        let rep = report_entry_log(State(st.clone()), p.clone(), Query(rq()))
            .await
            .unwrap();
        assert_eq!(rep.0["total"].as_u64(), Some(1));
        assert_eq!(rep.0["by_auth_status"]["matched"].as_u64(), Some(1));
        assert!(!serde_json::to_string(&rep.0)
            .unwrap()
            .contains("cam_SENTINEL_B"));

        // CONSTRAINT 2: an unscoped credential still sees every event, lane-less ones included.
        let admin = Principal::system_admin();
        assert_eq!(
            list_entry_events(State(st.clone()), admin.clone(), Query(eq()))
                .await
                .unwrap()
                .0
                .len(),
            3
        );
        assert_eq!(
            report_entry_log(State(st.clone()), admin, Query(rq()))
                .await
                .unwrap()
                .0["total"]
                .as_u64(),
            Some(3)
        );

        // An empty allowlist is fail-closed (`" AND 0"`, zero binds) and does not desync the binds.
        assert!(list_entry_events(State(st), scoped(&[]), Query(eq()))
            .await
            .unwrap()
            .0
            .is_empty());
    }

    #[tokio::test]
    async fn the_lane_roster_and_gate_actuation_are_confined() {
        let st = test_state().await;
        camera(&st.pool, "cam_a").await;
        camera(&st.pool, "cam_SENTINEL_B").await;
        policy(&st.pool, "cam_a").await;
        policy(&st.pool, "cam_SENTINEL_B").await;
        let p = scoped(&["cam_a"]);

        // The lane roster: one row per camera wired to a barrier.
        let state = get_gate_state(State(st.clone()), p.clone()).await.unwrap();
        let body = serde_json::to_string(&state.0).unwrap();
        assert!(!body.contains("cam_SENTINEL_B"), "{body}");
        assert_eq!(state.0["policies"].as_array().unwrap().len(), 1);

        // Physical actuation on a lane this credential does not hold.
        let err = gate_open(
            State(st.clone()),
            Path("cam_SENTINEL_B".to_string()),
            p.clone(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)), "got {err:?}");

        // Configuring and removing another lane's policy, both refused BEFORE the existence probe,
        // so a camera that is on the box and one that is not answer identically.
        let cfg_err = put_gate_policy(
            State(st.clone()),
            Path("cam_SENTINEL_B".to_string()),
            p.clone(),
            Json(GatePolicyUpdate {
                enabled: Some(false),
                output_port: None,
                pulse_ms: None,
            }),
        )
        .await
        .unwrap_err();
        let ghost_err = put_gate_policy(
            State(st.clone()),
            Path("cam_zzz".to_string()),
            p.clone(),
            Json(GatePolicyUpdate {
                enabled: Some(false),
                output_port: None,
                pulse_ms: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(cfg_err, AppError::Forbidden(_)), "got {cfg_err:?}");
        assert!(matches!(ghost_err, AppError::Forbidden(_)));
        let del_err = delete_gate_policy(
            State(st.clone()),
            Path("cam_SENTINEL_B".to_string()),
            p.clone(),
        )
        .await
        .unwrap_err();
        assert!(matches!(del_err, AppError::Forbidden(_)));
        // ...and the policy it targeted is still there.
        let survivors: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gate_policies")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(survivors.0, 2);

        // The global kill-switch is box-level: refused to a scoped credential in BOTH directions.
        assert!(put_gate_settings(
            State(st.clone()),
            p,
            Json(GateSettingsUpdate { kill_switch: true })
        )
        .await
        .is_err());
        assert!(!crate::gate::GateActuator::kill_switch(&st.pool).await);

        // CONSTRAINT 2: an unscoped credential still sees both lanes and still flips the switch.
        let admin = Principal::system_admin();
        let all = get_gate_state(State(st.clone()), admin.clone())
            .await
            .unwrap();
        assert_eq!(all.0["policies"].as_array().unwrap().len(), 2);
        assert!(put_gate_settings(
            State(st.clone()),
            admin,
            Json(GateSettingsUpdate { kill_switch: true })
        )
        .await
        .is_ok());
        assert!(crate::gate::GateActuator::kill_switch(&st.pool).await);
    }

    fn audit_query() -> AuditQuery {
        AuditQuery {
            from: None,
            to: None,
            actor: None,
            action: None,
            limit: None,
        }
    }

    /// Write through the REAL writer, never raw SQL.
    ///
    /// `auth::audit` is where `subject_camera_id` is derived, so a test that inserted rows by hand
    /// would be asserting against a column the box never populates that way — it would pass while the
    /// shipped path leaked. Going through the writer is what makes producer and reader agree.
    async fn audited(
        st: &AppState,
        action: &str,
        target_type: &str,
        target_id: &str,
        detail: Value,
    ) {
        let guard = Principal {
            id: "guard".into(),
            name: "guard".into(),
            ..Principal::system_admin()
        };
        auth::audit(&st.pool, &guard, action, target_type, target_id, detail).await;
        // RFC3339 strings sort as text and `Utc::now()` can repeat within one microsecond; a beat
        // between rows keeps `ORDER BY created_at DESC` deterministic for the assertions below.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }

    fn actions_of(rows: &[AuditLog]) -> Vec<&str> {
        rows.iter().map(|r| r.action.as_str()).collect()
    }

    #[tokio::test]
    async fn the_audit_log_hides_camera_targeted_rows_for_other_lanes() {
        let st = test_state().await;
        audited(&st, "open_a", "camera", "cam_a", json!({})).await;
        audited(&st, "open_b", "camera", "cam_SENTINEL_B", json!({})).await;
        audited(&st, "delete_vehicle", "vehicle", "veh_1", json!({})).await;
        audited(&st, "login", "", "", json!({})).await;

        let rows = list_audit(State(st.clone()), scoped(&["cam_a"]), Query(audit_query()))
            .await
            .unwrap();
        // Fail-closed: a scoped caller now sees ONLY rows it can be shown to own. The non-camera rows
        // that used to survive are hidden too — see
        // `the_audit_log_never_hands_a_scoped_caller_a_camera_it_does_not_hold` for why they cannot
        // be waved through: whether a row names a camera is a property of schemaless JSON.
        assert_eq!(actions_of(&rows.0), vec!["open_a"]);
        assert!(!serde_json::to_string(&rows.0)
            .unwrap()
            .contains("cam_SENTINEL_B"));

        // An empty allowlist owns nothing and therefore sees nothing.
        let none = list_audit(State(st.clone()), scoped(&[]), Query(audit_query()))
            .await
            .unwrap();
        assert!(none.0.is_empty());

        // CONSTRAINT 2: an unscoped credential still reads the whole log, unchanged.
        assert_eq!(
            list_audit(State(st), Principal::system_admin(), Query(audit_query()))
                .await
                .unwrap()
                .0
                .len(),
            4
        );
    }

    /// The audit-log camera leak: the owning camera lives in free-form `detail`, not in `target_id`.
    ///
    /// The predecessor filter masked rows whose `target_type` was `'camera'` and passed everything
    /// else. But zones, ai_task, camera_schedule, snapshot_schedule and recording_gap all record
    /// their camera as `detail.camera_id` under their OWN target_type, so one
    /// `GET /api/v1/audit?limit=5000` handed a lane-scoped manager the fleet roster plus which
    /// cameras carry zones, AI tasks and schedules. `detail` is `Json<Value>` with no schema, so no
    /// predicate can be trusted to read it — the subject has to be a column.
    #[tokio::test]
    async fn the_audit_log_never_hands_a_scoped_caller_a_camera_it_does_not_hold() {
        let st = test_state().await;
        // Verbatim shapes from routes/zones.rs, ai.rs, schedules.rs, snapshot_schedules.rs, anr.rs.
        audited(
            &st,
            "create_zone",
            "zone",
            "zone_1",
            json!({ "camera_id": "cam_SENTINEL_B", "name": "dock", "kind": "line" }),
        )
        .await;
        audited(
            &st,
            "create_ai_task",
            "ai_task",
            "task_1",
            json!({ "camera_id": "cam_SENTINEL_C", "task_type": "anpr" }),
        )
        .await;
        audited(
            &st,
            "create_schedule",
            "camera_schedule",
            "sch_1",
            json!({ "camera_id": "cam_SENTINEL_D", "time_start": "08:00" }),
        )
        .await;
        audited(
            &st,
            "anr_backfill",
            "recording_gap",
            "gap_1",
            json!({ "camera_id": "cam_SENTINEL_E" }),
        )
        .await;
        // ..and one the caller genuinely owns, so the route is proven filtered rather than emptied.
        audited(
            &st,
            "create_zone_mine",
            "zone",
            "zone_2",
            json!({ "camera_id": "cam_a", "name": "bay" }),
        )
        .await;

        let rows = list_audit(State(st.clone()), scoped(&["cam_a"]), Query(audit_query()))
            .await
            .unwrap();
        let body = serde_json::to_string(&rows.0).unwrap();
        assert!(
            !body.contains("SENTINEL"),
            "a camera named only in `detail` still reached a credential scoped elsewhere: {body}"
        );
        assert_eq!(actions_of(&rows.0), vec!["create_zone_mine"]);

        // CONSTRAINT 2: nothing changed for an unscoped credential — all five rows, detail intact.
        assert_eq!(
            list_audit(State(st), Principal::system_admin(), Query(audit_query()))
                .await
                .unwrap()
                .0
                .len(),
            5
        );
    }

    /// A row about SEVERAL cameras is fleet-level and is hidden from every scoped caller.
    ///
    /// An archive export over four lanes, or an API key mint that lists its own scope, is one act
    /// about the fleet. Attributing it to any single lane would show that lane's holder a `detail`
    /// containing the other camera ids — which is the leak again, one row at a time.
    #[tokio::test]
    async fn a_fleet_level_audit_row_is_not_visible_to_any_single_lane() {
        let st = test_state().await;
        audited(
            &st,
            "create_archive_export",
            "backup_job",
            "bkp_1",
            json!({ "camera_ids": ["cam_a", "cam_SENTINEL_B"], "trim": false }),
        )
        .await;
        audited(
            &st,
            "create_api_key",
            "api_key",
            "key_1",
            json!({ "scope_kind": "cameras", "scope_cameras": ["cam_a", "cam_SENTINEL_B"] }),
        )
        .await;

        for holder in [scoped(&["cam_a"]), scoped(&["cam_SENTINEL_B"])] {
            let rows = list_audit(State(st.clone()), holder, Query(audit_query()))
                .await
                .unwrap();
            assert!(
                rows.0.is_empty(),
                "a multi-camera row must not resolve to one of its cameras: {:?}",
                actions_of(&rows.0)
            );
        }
        assert_eq!(
            list_audit(State(st), Principal::system_admin(), Query(audit_query()))
                .await
                .unwrap()
                .0
                .len(),
            2
        );
    }
}
