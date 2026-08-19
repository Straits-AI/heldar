//! Movement-intelligence HTTP surface: camera-topology CRUD, the ReID candidate review workflow, the
//! red-zone breach incident workflow, and audited identity-search (plate trail + low-confidence person
//! candidates). Reads need can_view; reviews need can_operate_gate; topology edits need manage. Every
//! search is written to the kernel audit log (privacy gate).

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use heldar_kernel::auth::{self, Cap, Principal};
use heldar_kernel::error::{AppError, AppResult};
use heldar_kernel::state::{camera_scope_filter, scope_denied_owner, AppState};

use crate::config::MovementConfig;
use crate::models::{BreachAlert, CameraLink, CameraLinkCreate, MovementCandidate};

pub fn router(cfg: Arc<MovementConfig>) -> Router<AppState> {
    Router::new()
        .route("/api/v1/modules/movement/ui/index.js", get(serve_ui))
        .route("/api/v1/movement/run", post(trigger_run))
        .route("/api/v1/movement/links", get(list_links).post(create_link))
        .route(
            "/api/v1/movement/links/{id}",
            axum::routing::delete(delete_link),
        )
        .route("/api/v1/movement/candidates", get(list_candidates))
        .route(
            "/api/v1/movement/candidates/{id}/confirm",
            post(confirm_candidate),
        )
        .route(
            "/api/v1/movement/candidates/{id}/reject",
            post(reject_candidate),
        )
        .route("/api/v1/movement/breaches", get(list_breaches))
        .route("/api/v1/movement/breaches/{id}/ack", post(ack_breach))
        .route(
            "/api/v1/movement/breaches/{id}/resolve",
            post(resolve_breach),
        )
        .route("/api/v1/movement/search/plate/{plate}", get(search_plate))
        .route("/api/v1/movement/search/person", get(search_person))
        .layer(Extension(cfg))
}

// ---- Camera scope ---------------------------------------------------------
//
// Movement is the one surface in the tree whose subject matter IS the relationship between two
// cameras, so the containment rule differs from the kernel's: a resource that names two cameras is
// visible to a camera-scoped credential only when it holds BOTH ends. Anything less would let a
// credential scoped to `cam_a` learn that `cam_b` exists, is adjacent, and how long the walk between
// them takes — which is the camera roster plus the site's physical layout.
//
// Every helper below is a discriminant compare that returns `Ok`/`None` for `Scope::All`, so every
// human role, every key minted without a camera list, and the auth-disabled LAN default (whose
// principal is the unscoped system admin) are unaffected by construction rather than by convention.
// None of them is reachable from a background task: the engines (`reid::run_once`, `breach::run_once`)
// hold no `Principal` and keep their raw queries.

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
/// resource BEFORE its existence is disclosed. Returns the row's non-NULL camera columns, which the
/// caller needs to attribute its audit entry (see [`audit_subject`]).
///
/// This is the app-crate twin of `AppState::resource_camera`. `cameras_sql` must select exactly the
/// row's camera columns for `WHERE id = ?` — one for `breach_alerts`, two for `camera_links` and
/// `movement_candidates` — and EVERY selected column must be in scope, because a two-camera row
/// discloses both ends.
///
/// - `Scope::All`: behaviour is identical to today — the row is looked up and a missing row is the
///   pre-existing 404 naming the resource. A NULL camera column is not a refusal for an unscoped
///   caller; the movement tables allow one (an unlinked candidate, a zone-less breach).
/// - `Scope::Cameras`: "belongs to a camera you do not hold", "carries no camera at all" and "does
///   not exist" produce the SAME [`AppError`] value, byte for byte, so the id space cannot be
///   enumerated by probing.
async fn require_resource_scope(
    pool: &sqlx::SqlitePool,
    principal: &Principal,
    cameras_sql: &str,
    id: &str,
    noun: &str,
    action: &str,
) -> AppResult<Vec<String>> {
    use sqlx::Row as _;
    let row = sqlx::query(cameras_sql)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let cameras: Vec<Option<String>> = row
        .as_ref()
        .map(|r| {
            (0..r.columns().len())
                .map(|i| r.try_get::<Option<String>, _>(i).ok().flatten())
                .collect()
        })
        .unwrap_or_default();
    let all_held = row.is_some()
        && cameras
            .iter()
            .all(|c| c.as_deref().is_some_and(|c| principal.camera_allowed(c)));
    // Only the columns that actually carry a camera; an unscoped caller may hold a row with none.
    let held: Vec<String> = cameras.iter().flatten().cloned().collect();
    if all_held {
        return Ok(held);
    }
    if principal.camera_scope().is_some() {
        // Missing and out-of-scope are deliberately indistinguishable.
        return Err(scope_denied_owner(noun, action));
    }
    match row {
        // Unscoped: `camera_allowed` is always true, so we only land here on a NULL camera column,
        // which has never been a refusal and must not become one.
        Some(_) => Ok(held),
        None => Err(AppError::NotFound(format!("{noun} {id} not found"))),
    }
}

/// The audit `detail` naming the camera an action is ABOUT, for `crate::auth::audit`'s
/// `subject_camera_id` derivation.
///
/// The kernel derives that column from `detail.camera_id` (or a ONE-element `camera_ids`), and
/// `GET /api/v1/audit` filters on it fail-closed: for a camera-scoped reader a NULL subject is
/// HIDDEN. Movement audited every one of its actions with `{}`, so every movement act — including a
/// breach a scoped operator acknowledged on its OWN camera — was invisible in that operator's own
/// audit trail while the fleet auditor saw it. That is the same accountability hole the kernel's
/// archive export had, described in `auth::subject_camera`, reached from this crate instead.
///
/// The rule this encodes, and why it is not "name the first camera":
///
/// * Exactly ONE camera ⇒ name it. The act is about that camera and its holder is entitled to see
///   it, whoever performed it.
/// * TWO cameras (a link, a candidate) ⇒ NULL, deliberately. `subject_camera_id` is a single column
///   and cannot express "both ends", so naming either one would show the row to a credential holding
///   only that end — telling it that its camera is adjacent to something, which is exactly the
///   inference the both-ends rule above exists to deny. Fail closed instead: a cross-camera act is a
///   fleet act, visible in the fleet audit trail only.
/// * NO camera ⇒ NULL, unchanged (a camera-less breach is about no camera).
fn audit_subject(cameras: &[String]) -> Value {
    match cameras {
        [only] => json!({ "camera_id": only }),
        _ => json!({}),
    }
}

/// Is a row carrying an OPTIONAL camera visible to this principal? An unscoped caller sees
/// everything including camera-less rows; a scoped caller sees only its own cameras, and a camera-less
/// row is refused rather than shown, because there is no camera by which it could be held.
fn row_in_scope(principal: &Principal, camera_id: Option<&str>) -> bool {
    if principal.camera_scope().is_none() {
        return true;
    }
    camera_id.is_some_and(|c| principal.camera_allowed(c))
}

/// The built movement module UI bundle, embedded at compile time (regenerate with `make module-bundles`
/// after editing `apps/web/src/modules/movement`). It imports React + the shell SDK (`@heldar/shell`) as
/// bare specifiers the dashboard's import map resolves — so this crate ships only the module's own code.
const MOVEMENT_UI_BUNDLE: &str = include_str!("../ui/movement.js");

/// Serve the runtime-loaded movement module UI (the dashboard imports it via `ModuleHost`). Any
/// authenticated viewer may load it — it is inert frontend code; the data it fetches is separately
/// gated by the kernel's RBAC.
async fn serve_ui(principal: Principal) -> AppResult<axum::response::Response> {
    principal.require_cap(Cap::EventsRead, "load the movement module UI")?;
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
        MOVEMENT_UI_BUNDLE,
    )
        .into_response())
}

/// Run the ReID proposer + breach sweep once (ops / testing); both also run on a timer.
async fn trigger_run(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<MovementConfig>>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "run movement engines")?;
    // Box-level: both engines sweep EVERY camera link and EVERY zone event on the box, and there is
    // no camera id to scope the run by. A camera-scoped credential can never legitimately drive it,
    // so containment here is a refusal — the same reasoning applied to network discovery and to
    // off-box backup destinations.
    require_fleet_scope(&principal, "run the movement engines")?;
    crate::reid::run_once(&st.pool, &cfg)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("reid: {e}")))?;
    crate::breach::run_once(&st.pool, &cfg)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("breach: {e}")))?;
    Ok(Json(json!({ "ok": true })))
}

/// Normalize a plate to the entry-engine's lookup form (uppercase, alphanumeric only).
fn normalize_plate(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

// ---- Topology ----

async fn list_links(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<CameraLink>>> {
    principal.require_cap(Cap::EventsRead, "view camera topology")?;
    // A link names two cameras, so a scoped credential sees it only when it holds BOTH ends —
    // otherwise the topology is a roster of the cameras it does not hold, annotated with the physical
    // distance to each. `camera_scope_filter` returns None for an unscoped caller, so the query below
    // is byte-identical to today's for every human role.
    let from_scope = camera_scope_filter(&principal, "from_camera");
    let to_scope = camera_scope_filter(&principal, "to_camera");
    let mut sql = "SELECT * FROM camera_links WHERE 1=1".to_string();
    if let Some((pred, _)) = &from_scope {
        sql.push_str(pred);
    }
    if let Some((pred, _)) = &to_scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY from_camera, to_camera");
    let mut q = sqlx::query_as::<_, CameraLink>(&sql);
    // Bind from the RETURNED vectors, never from `camera_scope()`: an empty allowlist yields the
    // zero-bind `" AND 0"` arm, and iterating the scope instead would desync the parameter count.
    for id in from_scope.iter().flat_map(|(_, ids)| ids) {
        q = q.bind(id);
    }
    for id in to_scope.iter().flat_map(|(_, ids)| ids) {
        q = q.bind(id);
    }
    let rows = q.fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

async fn create_link(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<CameraLinkCreate>,
) -> AppResult<(StatusCode, Json<CameraLink>)> {
    principal.require(principal.can_manage_registry(), "edit camera topology")?;
    if body.from_camera.trim().is_empty() || body.to_camera.trim().is_empty() {
        return Err(AppError::BadRequest(
            "`from_camera` and `to_camera` are required".into(),
        ));
    }
    // Compared TRIMMED, because the trimmed values are what `require_camera` checks and what the
    // INSERT below binds. Comparing the raw fields let `{"from_camera":"cam_a","to_camera":" cam_a"}`
    // past this guard and then store the self-link it forbids.
    if body.from_camera.trim() == body.to_camera.trim() {
        return Err(AppError::BadRequest(
            "a camera cannot link to itself".into(),
        ));
    }
    // Both ends must be held. Naming the ids in the refusal is safe here and only here: they are the
    // caller's own input on a create, so the message confirms nothing the caller did not already say.
    principal.require_camera(body.from_camera.trim(), "link cameras")?;
    principal.require_camera(body.to_camera.trim(), "link cameras")?;
    let id = format!("lnk_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_links (id, from_camera, to_camera, transit_seconds, bidirectional, note, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(body.from_camera.trim())
    .bind(body.to_camera.trim())
    .bind(body.transit_seconds.unwrap_or(120).clamp(1, 86400))
    .bind(body.bidirectional.unwrap_or(false))
    .bind(&body.note)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;
    // A link names TWO cameras, so it carries no single audit SUBJECT — `subject_camera` resolves a
    // multi-camera `camera_ids` to NULL deliberately, because naming one end would disclose adjacency
    // to that end's holder. Recording BOTH ids is what lets `list_audit` show the row to a caller
    // holding every camera involved: it already knows both, and it performed the act. Without this
    // the act was invisible in its own audit trail.
    auth::audit(
        &st.pool,
        &principal,
        "movement_link_create",
        "camera_link",
        &id,
        json!({ "camera_ids": [body.from_camera.trim(), body.to_camera.trim()] }),
    )
    .await;
    let link = sqlx::query_as::<_, CameraLink>("SELECT * FROM camera_links WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok((StatusCode::CREATED, Json(link)))
}

async fn delete_link(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "edit camera topology")?;
    // Before the DELETE, so a scoped credential cannot use the 204-vs-404 split to enumerate the
    // link id space (and cannot sever the topology between two cameras it does not hold).
    require_resource_scope(
        &st.pool,
        &principal,
        "SELECT from_camera, to_camera FROM camera_links WHERE id = ?",
        &id,
        "camera link",
        "edit camera topology",
    )
    .await?;
    // Read the pair BEFORE the delete: the audit row needs both ids, and after the DELETE the row
    // that names them is gone.
    let pair: Option<(String, String)> =
        sqlx::query_as("SELECT from_camera, to_camera FROM camera_links WHERE id = ?")
            .bind(&id)
            .fetch_optional(&st.pool)
            .await?;
    let res = sqlx::query("DELETE FROM camera_links WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("camera link {id} not found")));
    }
    // Two cameras ⇒ no single audit subject, same as the create above; both ids recorded so a
    // both-ends holder can see its own deletion.
    auth::audit(
        &st.pool,
        &principal,
        "movement_link_delete",
        "camera_link",
        &id,
        json!({ "camera_ids": pair
            .as_ref()
            .map(|(f, t)| vec![f.as_str(), t.as_str()])
            .unwrap_or_default() }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Candidate review workflow ----

#[derive(Debug, Deserialize)]
struct CandQuery {
    status: Option<String>,
    anchor: Option<String>,
    limit: Option<i64>,
}

async fn list_candidates(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<CandQuery>,
) -> AppResult<Json<Vec<MovementCandidate>>> {
    principal.require_cap(Cap::EventsRead, "view movement candidates")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let anchor = q
        .anchor
        .as_deref()
        .map(normalize_plate)
        .filter(|s| !s.is_empty());
    // A plate-anchored candidate query is an identity-like lookup — audit it, same as /search/plate.
    if let Some(a) = &anchor {
        auth::audit(
            &st.pool,
            &principal,
            "movement_search_plate",
            "plate",
            a,
            json!({ "via": "candidates_filter" }),
        )
        .await;
    }
    // A candidate is a claim ABOUT two cameras — it carries both ids, both timestamps and the transit
    // time between them — so a scoped credential sees it only when it holds both ends. `IN (…)`
    // naturally excludes the NULL endpoints a partially-resolved candidate can carry, which is the
    // fail-closed answer; unscoped callers get no predicate at all and see exactly today's rows.
    let from_scope = camera_scope_filter(&principal, "from_camera");
    let to_scope = camera_scope_filter(&principal, "to_camera");
    let mut sql = "SELECT * FROM movement_candidates
          WHERE (? IS NULL OR status = ?) AND (? IS NULL OR anchor = ?)"
        .to_string();
    if let Some((pred, _)) = &from_scope {
        sql.push_str(pred);
    }
    if let Some((pred, _)) = &to_scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY score DESC, created_at DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, MovementCandidate>(&sql)
        .bind(&q.status)
        .bind(&q.status)
        .bind(&anchor)
        .bind(&anchor);
    for id in from_scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    for id in to_scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    let rows = query.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

async fn confirm_candidate(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<MovementCandidate>> {
    resolve_candidate(st, principal, id, "confirmed").await
}
async fn reject_candidate(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<MovementCandidate>> {
    resolve_candidate(st, principal, id, "rejected").await
}

async fn resolve_candidate(
    st: AppState,
    principal: Principal,
    id: String,
    status: &str,
) -> AppResult<Json<MovementCandidate>> {
    // ReID is candidate matching, not identity — a human makes the call, and it is audited.
    principal.require(principal.can_operate_gate(), "review movement candidates")?;
    // Before the UPDATE: a review is a durable, attributed judgement on someone else's cameras, and
    // the pre-existing 404-on-no-rows would otherwise map the candidate id space.
    require_resource_scope(
        &st.pool,
        &principal,
        "SELECT from_camera, to_camera FROM movement_candidates WHERE id = ?",
        &id,
        "candidate",
        "review movement candidates",
    )
    .await?;
    let res = sqlx::query(
        "UPDATE movement_candidates SET status=?, reviewed_by=?, reviewed_at=? WHERE id=?",
    )
    .bind(status)
    .bind(&principal.name)
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("candidate {id} not found")));
    }
    // A candidate is a claim about TWO cameras ⇒ no single audit subject, same as a link.
    auth::audit(
        &st.pool,
        &principal,
        &format!("movement_candidate_{status}"),
        "movement_candidate",
        &id,
        json!({}),
    )
    .await;
    let c =
        sqlx::query_as::<_, MovementCandidate>("SELECT * FROM movement_candidates WHERE id = ?")
            .bind(&id)
            .fetch_one(&st.pool)
            .await?;
    Ok(Json(c))
}

// ---- Breach incident workflow ----

#[derive(Debug, Deserialize)]
struct BreachQuery {
    status: Option<String>,
    limit: Option<i64>,
}

async fn list_breaches(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<BreachQuery>,
) -> AppResult<Json<Vec<BreachAlert>>> {
    principal.require_cap(Cap::EventsRead, "view breach alerts")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    // A breach alert carries `camera_id`, `zone_name` and an evidence frame path — the roster plus a
    // picture. `IN (…)` excludes the NULL `camera_id` a camera-less alert can carry, which is the
    // fail-closed answer for a scoped caller; unscoped callers see today's rows unchanged.
    let scope = camera_scope_filter(&principal, "camera_id");
    let mut sql = "SELECT * FROM breach_alerts WHERE (? IS NULL OR status = ?)".to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, BreachAlert>(&sql)
        .bind(&q.status)
        .bind(&q.status);
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        query = query.bind(id);
    }
    let rows = query.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

async fn ack_breach(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<BreachAlert>> {
    set_breach_status(st, principal, id, "acknowledged").await
}
async fn resolve_breach(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<BreachAlert>> {
    set_breach_status(st, principal, id, "resolved").await
}

async fn set_breach_status(
    st: AppState,
    principal: Principal,
    id: String,
    status: &str,
) -> AppResult<Json<BreachAlert>> {
    principal.require(principal.can_operate_gate(), "work breach alerts")?;
    // Before the UPDATE: acknowledging or resolving another camera's alert silently retires an open
    // incident for the operator who owns it, and the 404-on-no-rows would map the alert id space.
    let cameras = require_resource_scope(
        &st.pool,
        &principal,
        "SELECT camera_id FROM breach_alerts WHERE id = ?",
        &id,
        "breach",
        "work breach alerts",
    )
    .await?;
    let (rby, rat) = if status == "resolved" {
        (Some(principal.name.clone()), Some(Utc::now()))
    } else {
        (None, None)
    };
    let res = sqlx::query(
        "UPDATE breach_alerts SET status=?, resolved_by=COALESCE(?, resolved_by), resolved_at=COALESCE(?, resolved_at) WHERE id=?",
    )
    .bind(status)
    .bind(&rby)
    .bind(rat)
    .bind(&id)
    .execute(&st.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("breach {id} not found")));
    }
    // A breach names ONE camera, so the act has a subject and the camera's holder is entitled to see
    // it in `GET /api/v1/audit` — whoever worked the alert. Retiring an open incident is precisely
    // the kind of durable, attributed judgement whose accountability must not depend on holding a
    // fleet-wide credential. `require_resource_scope` above already proved the caller may act on it.
    auth::audit(
        &st.pool,
        &principal,
        &format!("breach_{status}"),
        "breach_alert",
        &id,
        audit_subject(&cameras),
    )
    .await;
    let b = sqlx::query_as::<_, BreachAlert>("SELECT * FROM breach_alerts WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(b))
}

// ---- Audited identity-search ----

async fn search_plate(
    State(st): State<AppState>,
    principal: Principal,
    Path(plate): Path<String>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "search movement by plate")?;
    let norm = normalize_plate(&plate);
    if norm.is_empty() {
        return Err(AppError::BadRequest("empty plate".into()));
    }
    // Privacy gate: every identity-like query is audited. A plate trail is anchored on a PLATE and
    // spans whatever cameras saw it, so there is no one camera it is about — no audit subject, and
    // the entry stays fleet-only. Same for the plate-anchored candidate filter above.
    auth::audit(
        &st.pool,
        &principal,
        "movement_search_plate",
        "plate",
        &norm,
        json!({}),
    )
    .await;
    let mut trail = crate::reid::trail_for_plate(&st.pool, &norm).await?;
    let mut candidates = sqlx::query_as::<_, MovementCandidate>(
        "SELECT * FROM movement_candidates WHERE anchor = ? ORDER BY to_time DESC LIMIT 200",
    )
    .bind(&norm)
    .fetch_all(&st.pool)
    .await?;
    // A plate trail is the fleet's sighting history for one vehicle: every appearance names the camera
    // that saw it, so an unfiltered trail is the roster indexed by plate. A scoped credential gets the
    // trail AS ITS OWN CAMERAS SAW IT — the route stays useful rather than being refused, and the
    // cameras it does not hold simply do not appear. Both are no-ops for `Scope::All`.
    trail.retain(|a| row_in_scope(&principal, a.camera_id.as_deref()));
    candidates.retain(|c| {
        row_in_scope(&principal, c.from_camera.as_deref())
            && row_in_scope(&principal, c.to_camera.as_deref())
    });
    Ok(Json(json!({
        "plate": norm,
        "appearances": trail,
        "candidates": candidates,
        "note": "Cross-camera correlation by plate is probabilistic (OCR can err / plates can be cloned); appearances are anchored on the resolved plate and require human judgement, not legal identity.",
    })))
}

#[derive(Debug, Deserialize)]
struct PersonQuery {
    camera: String,
    track: String,
    /// RFC3339 time of the source appearance.
    at: String,
}

async fn search_person(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<PersonQuery>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "search movement by person track")?;
    // The source camera is caller input, so naming it in the refusal discloses nothing. Checked
    // before the timestamp parse would be pointless (the parse reveals nothing about the fleet), but
    // it IS checked before the audit write and before any detection row is read.
    principal.require_camera(&q.camera, "search movement by person track")?;
    let at = heldar_kernel::util::parse_rfc3339(&q.at)
        .ok_or_else(|| AppError::BadRequest("invalid `at` timestamp".into()))?;
    // Anchored on ONE camera — the one `require_camera` just proved the caller holds — so the search
    // has an audit subject. Without it, "someone ran an identity-like search over a person track on
    // your camera" was readable only by a fleet-wide credential.
    auth::audit(
        &st.pool,
        &principal,
        "movement_search_person",
        "track",
        &format!("{}:{}", q.camera, q.track),
        json!({ "at": q.at, "camera_id": q.camera }),
    )
    .await;

    // Linked downstream cameras + their transit windows.
    let links: Vec<(String, i64)> = sqlx::query_as(
        "SELECT to_camera, transit_seconds FROM camera_links WHERE from_camera = ?
         UNION
         SELECT from_camera, transit_seconds FROM camera_links WHERE to_camera = ? AND bidirectional = 1",
    )
    .bind(&q.camera)
    .bind(&q.camera)
    .fetch_all(&st.pool)
    .await?;

    let mut candidates: Vec<Value> = Vec::new();
    for (cam, transit) in links {
        // Drop the linked cameras this credential does not hold: the walk would otherwise name them
        // (and hand back their person tracks) purely because they are adjacent to one it does hold.
        if !principal.camera_allowed(&cam) {
            continue;
        }
        let hi = at + chrono::TimeDelta::try_seconds(transit * 4).unwrap();
        // Distinct downstream person tracks first seen within the transit window.
        let tracks: Vec<(String, chrono::DateTime<Utc>)> = sqlx::query_as(
            "SELECT track_id, MIN(timestamp) FROM detections
              WHERE camera_id = ? AND label = 'person' AND track_id IS NOT NULL
                AND timestamp > ? AND timestamp <= ?
              GROUP BY track_id ORDER BY MIN(timestamp) ASC LIMIT 50",
        )
        .bind(&cam)
        .bind(at)
        .bind(hi)
        .fetch_all(&st.pool)
        .await?;
        for (track, first) in tracks {
            let gap = (first - at).num_seconds() as f64;
            // Topology + time only — no plate, no appearance embedding. Deliberately low confidence.
            let score = if transit > 0 && gap <= transit as f64 {
                0.4
            } else {
                0.25
            };
            candidates.push(json!({
                "to_camera": cam, "to_track": track, "to_time": first,
                "transit_seconds": gap, "score": score,
            }));
        }
    }
    candidates.sort_by(|a, b| {
        b["score"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&a["score"].as_f64().unwrap_or(0.0))
    });
    Ok(Json(json!({
        "from": { "camera": q.camera, "track": q.track, "at": q.at },
        "candidates": candidates,
        "note": "Person ReID here uses ONLY camera topology + transit time (no plate, no appearance embedding). These are weak, low-confidence candidates for human triage — never identity.",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use heldar_kernel::auth::Scope;
    use std::collections::HashSet;

    /// A camera-scoped credential holding every capability — the attacker in the audit report. Only
    /// `scope` differs from the auth-disabled system admin, so any behaviour difference between the
    /// two below is attributable to camera scope and nothing else.
    fn scoped(cameras: &[&str]) -> Principal {
        let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: Scope::Cameras(std::sync::Arc::new(set)),
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

    async fn link(pool: &sqlx::SqlitePool, id: &str, from: &str, to: &str) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO camera_links (id, from_camera, to_camera, transit_seconds, bidirectional, created_at, updated_at)
             VALUES (?,?,?,120,0,?,?)",
        )
        .bind(id)
        .bind(from)
        .bind(to)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn candidate(pool: &sqlx::SqlitePool, id: &str, from: &str, to: &str) {
        sqlx::query(
            "INSERT INTO movement_candidates (id, subject_type, anchor, from_camera, from_ref, to_camera, to_ref, score, signals, status, created_at)
             VALUES (?, 'vehicle', 'ABC123', ?, ?, ?, ?, 0.5, '{}', 'pending', ?)",
        )
        .bind(id)
        .bind(from)
        // `UNIQUE(subject_type, from_ref, to_ref)`: derive the refs from the row id so fixtures differ.
        .bind(format!("{id}_from"))
        .bind(to)
        .bind(format!("{id}_to"))
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
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

    async fn breach(pool: &sqlx::SqlitePool, id: &str, camera: Option<&str>) {
        sqlx::query(
            "INSERT INTO breach_alerts (id, camera_id, rule, severity, status, detail, created_at)
             VALUES (?, ?, 'red_zone_entry', 'warning', 'open', '{}', ?)",
        )
        .bind(id)
        .bind(camera)
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn require_fleet_scope_is_a_no_op_for_an_unscoped_principal() {
        // CONSTRAINT 1 + 2: the auth-disabled default and every human role are untouched.
        assert!(require_fleet_scope(&Principal::system_admin(), "run").is_ok());
        // Any camera scope, INCLUDING an empty one, is still a scope and is refused.
        assert!(require_fleet_scope(&scoped(&["cam_a"]), "run").is_err());
        assert!(require_fleet_scope(&scoped(&[]), "run").is_err());
    }

    #[test]
    fn row_in_scope_refuses_a_camera_less_row_only_for_a_scoped_principal() {
        let admin = Principal::system_admin();
        assert!(row_in_scope(&admin, None));
        assert!(row_in_scope(&admin, Some("cam_SENTINEL_B")));
        let p = scoped(&["cam_a"]);
        assert!(row_in_scope(&p, Some("cam_a")));
        assert!(!row_in_scope(&p, Some("cam_SENTINEL_B")));
        // No camera at all: there is no camera by which a scoped credential could hold it.
        assert!(!row_in_scope(&p, None));
    }

    #[tokio::test]
    async fn require_resource_scope_is_unchanged_for_an_unscoped_principal() {
        let st = test_state().await;
        link(&st.pool, "lnk_ab", "cam_a", "cam_SENTINEL_B").await;
        breach(&st.pool, "brc_null", None).await;
        let admin = Principal::system_admin();
        const LINK_SQL: &str = "SELECT from_camera, to_camera FROM camera_links WHERE id = ?";

        assert!(require_resource_scope(
            &st.pool,
            &admin,
            LINK_SQL,
            "lnk_ab",
            "camera link",
            "edit"
        )
        .await
        .is_ok());
        // A camera-less row has never been a refusal for an unscoped caller and must not become one.
        assert!(require_resource_scope(
            &st.pool,
            &admin,
            "SELECT camera_id FROM breach_alerts WHERE id = ?",
            "brc_null",
            "breach",
            "work"
        )
        .await
        .is_ok());
        // Missing: still the pre-existing 404 with the pre-existing wording.
        let err =
            require_resource_scope(&st.pool, &admin, LINK_SQL, "lnk_zzz", "camera link", "edit")
                .await
                .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
        assert!(err.to_string().contains("camera link lnk_zzz not found"));
    }

    #[tokio::test]
    async fn require_resource_scope_never_becomes_an_existence_oracle() {
        let st = test_state().await;
        link(&st.pool, "lnk_held", "cam_a", "cam_a2").await;
        link(&st.pool, "lnk_half", "cam_a", "cam_SENTINEL_B").await;
        link(&st.pool, "lnk_none", "cam_SENTINEL_B", "cam_SENTINEL_C").await;
        let p = scoped(&["cam_a", "cam_a2"]);
        const LINK_SQL: &str = "SELECT from_camera, to_camera FROM camera_links WHERE id = ?";

        // Both ends held: allowed.
        assert!(
            require_resource_scope(&st.pool, &p, LINK_SQL, "lnk_held", "camera link", "edit")
                .await
                .is_ok()
        );

        // Half-held, not held at all, and NONEXISTENT must be indistinguishable — byte for byte.
        let half =
            require_resource_scope(&st.pool, &p, LINK_SQL, "lnk_half", "camera link", "edit")
                .await
                .unwrap_err();
        let none =
            require_resource_scope(&st.pool, &p, LINK_SQL, "lnk_none", "camera link", "edit")
                .await
                .unwrap_err();
        let ghost =
            require_resource_scope(&st.pool, &p, LINK_SQL, "lnk_zzz", "camera link", "edit")
                .await
                .unwrap_err();
        assert!(matches!(half, AppError::Forbidden(_)), "got {half:?}");
        assert_eq!(half.to_string(), none.to_string());
        assert_eq!(half.to_string(), ghost.to_string());
        // And the refusal names neither the out-of-scope camera nor the probed id.
        let msg = half.to_string();
        assert!(!msg.contains("cam_SENTINEL_B"), "{msg}");
        assert!(!msg.contains("lnk_half"), "{msg}");
    }

    #[tokio::test]
    async fn topology_is_visible_only_when_both_ends_are_held() {
        let st = test_state().await;
        link(&st.pool, "lnk_held", "cam_a", "cam_a2").await;
        link(&st.pool, "lnk_half", "cam_a", "cam_SENTINEL_B").await;

        let held = list_links(State(st.clone()), scoped(&["cam_a", "cam_a2"]))
            .await
            .unwrap();
        assert_eq!(held.0.len(), 1);
        assert_eq!(held.0[0].id, "lnk_held");
        // Roster containment over the SERIALIZED body: the sentinel must not appear anywhere.
        let body = serde_json::to_string(&held.0).unwrap();
        assert!(!body.contains("cam_SENTINEL_B"), "{body}");

        // CONSTRAINT 2: an unscoped credential still sees the whole topology.
        let all = list_links(State(st.clone()), Principal::system_admin())
            .await
            .unwrap();
        assert_eq!(all.0.len(), 2);

        // An empty allowlist is fail-closed (the `" AND 0"` zero-bind arm) and does not desync binds.
        let none = list_links(State(st), scoped(&[])).await.unwrap();
        assert!(none.0.is_empty());
    }

    #[tokio::test]
    async fn candidates_and_breaches_are_confined_to_the_credentials_cameras() {
        let st = test_state().await;
        candidate(&st.pool, "mc_held", "cam_a", "cam_a2").await;
        candidate(&st.pool, "mc_half", "cam_a", "cam_SENTINEL_B").await;
        breach(&st.pool, "brc_held", Some("cam_a")).await;
        breach(&st.pool, "brc_other", Some("cam_SENTINEL_B")).await;
        breach(&st.pool, "brc_null", None).await;
        let p = scoped(&["cam_a", "cam_a2"]);
        let cq = || CandQuery {
            status: None,
            anchor: None,
            limit: None,
        };
        let bq = || BreachQuery {
            status: None,
            limit: None,
        };

        let cands = list_candidates(State(st.clone()), p.clone(), Query(cq()))
            .await
            .unwrap();
        assert_eq!(cands.0.len(), 1);
        assert_eq!(cands.0[0].id, "mc_held");
        assert!(!serde_json::to_string(&cands.0)
            .unwrap()
            .contains("cam_SENTINEL_B"));

        let brs = list_breaches(State(st.clone()), p, Query(bq()))
            .await
            .unwrap();
        assert_eq!(brs.0.len(), 1);
        assert_eq!(brs.0[0].id, "brc_held");
        assert!(!serde_json::to_string(&brs.0)
            .unwrap()
            .contains("cam_SENTINEL_B"));

        // CONSTRAINT 2: unscoped sees every row, camera-less ones included.
        let admin = Principal::system_admin();
        assert_eq!(
            list_candidates(State(st.clone()), admin.clone(), Query(cq()))
                .await
                .unwrap()
                .0
                .len(),
            2
        );
        assert_eq!(
            list_breaches(State(st), admin, Query(bq()))
                .await
                .unwrap()
                .0
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn a_plate_trail_shows_only_the_cameras_the_credential_holds() {
        let st = test_state().await;
        // `trail_for_plate` reads `entry_events_read`, the heldar-entry crate's published read
        // contract. Movement does not depend on that crate, so the fixture stands the contract's
        // columns up directly — exactly what the deployed view exposes to this consumer.
        sqlx::query(
            "CREATE TABLE entry_events_read (
                 id TEXT PRIMARY KEY, camera_id TEXT, timestamp TEXT NOT NULL,
                 event_type TEXT NOT NULL, plate TEXT, auth_status TEXT NOT NULL,
                 direction TEXT NOT NULL)",
        )
        .execute(&st.pool)
        .await
        .unwrap();
        for (id, cam) in [("evt_a", "cam_a"), ("evt_b", "cam_SENTINEL_B")] {
            sqlx::query(
                "INSERT INTO entry_events_read (id, camera_id, timestamp, event_type, plate, auth_status, direction)
                 VALUES (?, ?, ?, 'anpr', 'ABC123', 'matched', 'inbound')",
            )
            .bind(id)
            .bind(cam)
            .bind(Utc::now())
            .execute(&st.pool)
            .await
            .unwrap();
        }
        candidate(&st.pool, "mc_half", "cam_a", "cam_SENTINEL_B").await;

        let out = search_plate(
            State(st.clone()),
            scoped(&["cam_a"]),
            Path("ABC-123".to_string()),
        )
        .await
        .unwrap();
        let body = serde_json::to_string(&out.0).unwrap();
        assert!(!body.contains("cam_SENTINEL_B"), "{body}");
        assert_eq!(out.0["appearances"].as_array().unwrap().len(), 1);
        assert!(out.0["candidates"].as_array().unwrap().is_empty());

        // CONSTRAINT 2: the full cross-camera trail is unchanged for an unscoped credential.
        let all = search_plate(
            State(st),
            Principal::system_admin(),
            Path("ABC-123".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(all.0["appearances"].as_array().unwrap().len(), 2);
        assert_eq!(all.0["candidates"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_person_walk_refuses_an_out_of_scope_source_and_drops_out_of_scope_links() {
        let st = test_state().await;
        link(&st.pool, "lnk_half", "cam_a", "cam_SENTINEL_B").await;
        camera(&st.pool, "cam_a").await;
        camera(&st.pool, "cam_SENTINEL_B").await;
        sqlx::query(
            "INSERT INTO detections (id, camera_id, task_type, timestamp, label, confidence, bbox, track_id, attributes, created_at)
             VALUES ('det_1', 'cam_SENTINEL_B', 'detection', ?, 'person', 0.9, '[0,0,1,1]', 'trk_9', '{}', ?)",
        )
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&st.pool)
        .await
        .unwrap();
        let q = |cam: &str| PersonQuery {
            camera: cam.to_string(),
            track: "trk_1".to_string(),
            at: (Utc::now() - chrono::TimeDelta::try_minutes(5).unwrap()).to_rfc3339(),
        };

        // The source camera itself is refused when out of scope.
        let err = search_person(
            State(st.clone()),
            scoped(&["cam_a"]),
            Query(q("cam_SENTINEL_B")),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)), "got {err:?}");

        // From a held camera, the linked-but-unheld camera is dropped from the walk entirely.
        let out = search_person(State(st.clone()), scoped(&["cam_a"]), Query(q("cam_a")))
            .await
            .unwrap();
        let body = serde_json::to_string(&out.0).unwrap();
        assert!(!body.contains("cam_SENTINEL_B"), "{body}");
        assert!(out.0["candidates"].as_array().unwrap().is_empty());

        // CONSTRAINT 2: an unscoped credential still walks the link and finds the downstream track.
        let all = search_person(State(st), Principal::system_admin(), Query(q("cam_a")))
            .await
            .unwrap();
        assert_eq!(all.0["candidates"].as_array().unwrap().len(), 1);
    }
}
