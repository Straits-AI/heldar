//! Movement-intelligence HTTP surface: camera-topology CRUD, the ReID candidate review workflow, the
//! red-zone breach incident workflow, and audited identity-search (plate trail + low-confidence person
//! candidates). Reads need can_view; reviews need can_operate_gate; topology edits need manage. Every
//! search is written to the kernel audit log (privacy gate).

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use heldar_kernel::auth::{self, Principal};
use heldar_kernel::error::{AppError, AppResult};
use heldar_kernel::state::AppState;

use crate::config::MovementConfig;
use crate::models::{BreachAlert, CameraLink, CameraLinkCreate, MovementCandidate};

pub fn router(cfg: Arc<MovementConfig>) -> Router<AppState> {
    Router::new()
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

/// Run the ReID proposer + breach sweep once (ops / testing); both also run on a timer.
async fn trigger_run(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<MovementConfig>>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "run movement engines")?;
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
    principal.require(principal.can_view(), "view camera topology")?;
    let rows = sqlx::query_as::<_, CameraLink>(
        "SELECT * FROM camera_links ORDER BY from_camera, to_camera",
    )
    .fetch_all(&st.pool)
    .await?;
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
    if body.from_camera == body.to_camera {
        return Err(AppError::BadRequest(
            "a camera cannot link to itself".into(),
        ));
    }
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
    auth::audit(
        &st.pool,
        &principal,
        "movement_link_create",
        "camera_link",
        &id,
        json!({}),
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
    let res = sqlx::query("DELETE FROM camera_links WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("camera link {id} not found")));
    }
    auth::audit(
        &st.pool,
        &principal,
        "movement_link_delete",
        "camera_link",
        &id,
        json!({}),
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
    principal.require(principal.can_view(), "view movement candidates")?;
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
    let rows = sqlx::query_as::<_, MovementCandidate>(
        "SELECT * FROM movement_candidates
          WHERE (? IS NULL OR status = ?) AND (? IS NULL OR anchor = ?)
          ORDER BY score DESC, created_at DESC LIMIT ?",
    )
    .bind(&q.status)
    .bind(&q.status)
    .bind(&anchor)
    .bind(&anchor)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
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
    principal.require(principal.can_view(), "view breach alerts")?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let rows = sqlx::query_as::<_, BreachAlert>(
        "SELECT * FROM breach_alerts WHERE (? IS NULL OR status = ?) ORDER BY created_at DESC LIMIT ?",
    )
    .bind(&q.status)
    .bind(&q.status)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
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
    auth::audit(
        &st.pool,
        &principal,
        &format!("breach_{status}"),
        "breach_alert",
        &id,
        json!({}),
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
    principal.require(principal.can_view(), "search movement by plate")?;
    let norm = normalize_plate(&plate);
    if norm.is_empty() {
        return Err(AppError::BadRequest("empty plate".into()));
    }
    // Privacy gate: every identity-like query is audited.
    auth::audit(
        &st.pool,
        &principal,
        "movement_search_plate",
        "plate",
        &norm,
        json!({}),
    )
    .await;
    let trail = crate::reid::trail_for_plate(&st.pool, &norm).await?;
    let candidates = sqlx::query_as::<_, MovementCandidate>(
        "SELECT * FROM movement_candidates WHERE anchor = ? ORDER BY to_time DESC LIMIT 200",
    )
    .bind(&norm)
    .fetch_all(&st.pool)
    .await?;
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
    principal.require(principal.can_view(), "search movement by person track")?;
    let at = heldar_kernel::util::parse_rfc3339(&q.at)
        .ok_or_else(|| AppError::BadRequest("invalid `at` timestamp".into()))?;
    auth::audit(
        &st.pool,
        &principal,
        "movement_search_person",
        "track",
        &format!("{}:{}", q.camera, q.track),
        json!({ "at": q.at }),
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
