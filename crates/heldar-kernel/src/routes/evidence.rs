//! Evidence bundle export (#118).
//!
//! `POST /api/v1/evidence/exports` plans or produces a bundle; the plan is the default so an
//! operator sees the gaps, the size and what will be included before anything is written.
//!
//! CAMERA SCOPE. Every route here resolves a camera id and checks it against the caller's scope
//! before touching footage — including the incident path, where the camera is derived rather than
//! supplied. An acceptance criterion of #118 is that a camera-scoped principal cannot reach another
//! camera's footage "through incident or artifact IDs", which is precisely a case where the id the
//! caller sends is not the id the export reads.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::services::evidence;
use crate::state::AppState;
use crate::util;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/evidence/exports", post(create).get(list))
        .route("/api/v1/evidence/exports/{id}", get(get_one))
        .route("/api/v1/evidence/signing-key", get(signing_key))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExportRequest {
    pub camera_id: Option<String>,
    /// Derive the camera from an incident's locked segments instead of naming it.
    incident_id: Option<String>,
    from: String,
    to: String,
    /// Default TRUE. An export writes footage off the box under a signature; making the destructive
    /// direction the one you have to ask for is the right way round.
    #[serde(default = "yes")]
    dry_run: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ExportResponse {
    Plan(evidence::BundlePlan),
    Bundle(Box<evidence::BundleResult>),
}

/// Plan or produce a signed evidence bundle.
///
/// Defaults to a DRY RUN: the response is the plan (media size, coverage, gaps, record counts) and
/// nothing is written. Pass `dry_run: false` to produce the bundle.
#[utoipa::path(
    post, path = "/api/v1/evidence/exports", tag = "evidence",
    operation_id = "createEvidenceExport",
    responses(
        (status = 200, description = "The export plan (dry run), or the bundle that was written"),
        (status = 400, description = "Bad time range, or neither/both of camera_id and incident_id", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `video:export`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera, or no footage in the range", body = crate::openapi::ErrorBody),
        (status = 503, description = "Media job concurrency limit reached", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn create(
    State(st): State<AppState>,
    principal: Principal,
    Json(req): Json<ExportRequest>,
) -> AppResult<Json<ExportResponse>> {
    principal.require_cap(Cap::VideoExport, "export evidence bundles")?;

    // `resolve_camera` performs the scope check for the DERIVED (incident) case itself, because a
    // refusal there must not name the camera it resolved. What remains here is the caller-supplied
    // case, where the id is already known to them and a specific message discloses nothing.
    let camera_id = resolve_camera(&st, &principal, &req).await?;
    st.camera_scope_check(&principal, &camera_id)?;

    let from = util::parse_rfc3339(&req.from)
        .ok_or_else(|| AppError::BadRequest("invalid `from` timestamp".into()))?;
    let to = util::parse_rfc3339(&req.to)
        .ok_or_else(|| AppError::BadRequest("invalid `to` timestamp".into()))?;

    if req.dry_run {
        return Ok(Json(ExportResponse::Plan(
            evidence::plan(&st, &camera_id, from, to).await?,
        )));
    }

    // The id the caller was actually handed back, not the raw inbound header (#169).
    //
    // `request_id::layer` puts the correlation id in a task-local and on the RESPONSE; it does not
    // write it back onto the request. So reading the header here recorded NULL for every export
    // where the client sent no id — the normal case — while the response and the audit row carried
    // `req_...`. A bundle whose manifest says `request_id: null` cannot be joined back to the call
    // that produced it, which is exactly the asymmetry this work exists to close, and this route was
    // the one place already claiming to have closed it.
    //
    // Reading the task-local also means the value is the SANITISED one: a caller-supplied id is
    // bounded and stripped before it reaches a log line, and a signed manifest deserves the same.
    let request_id = crate::request_id::current();
    // Audit BEFORE the export, so the bundle can carry the audit id it is recorded under. An audit
    // row written afterwards could not be referenced by the document it describes.
    let audit_id = auth::audit(
        &st.pool,
        &principal,
        "export_evidence_bundle",
        "camera",
        &camera_id,
        json!({
            "from": req.from, "to": req.to,
            "incident_id": req.incident_id,
            "request_id": request_id,
        }),
    )
    .await;

    let result = evidence::export(
        &st,
        &principal,
        &camera_id,
        from,
        to,
        req.incident_id.as_deref(),
        audit_id.as_deref(),
        request_id.as_deref(),
    )
    .await?;
    Ok(Json(ExportResponse::Bundle(Box::new(result))))
}

/// The camera an export addresses: named directly, or derived from an incident's segments.
///
/// # Why this takes the principal
///
/// A camera id in the request body is something the caller already knows, so refusing it can say so
/// plainly. An id DERIVED from an incident is not: naming it in a 403 hands a scoped caller a camera
/// they do not hold, and answering 404-vs-403-vs-400 tells them whether the incident exists and how
/// many cameras it spans. That is an id-space oracle, and `AppState::resource_camera`'s doc comment
/// already forbids exactly this shape — "its message embeds the owning camera id, which hands a
/// scoped caller the fleet roster one probe at a time". This function was the one place in the tree
/// that resolved a camera from a resource id and then used the raw caller-supplied-id check anyway.
///
/// So for a SCOPED caller every outcome that is not "exactly one camera you hold" collapses to one
/// identical refusal, following the same convention `resource_camera` uses. An unscoped caller keeps
/// today's specific messages: nothing in them is new information to someone who holds every camera.
async fn resolve_camera(
    st: &AppState,
    principal: &Principal,
    req: &ExportRequest,
) -> AppResult<String> {
    match (&req.camera_id, &req.incident_id) {
        (Some(_), Some(_)) => Err(AppError::BadRequest(
            "give either `camera_id` or `incident_id`, not both — an incident names its own camera, \
             and reconciling a disagreement between them would mean guessing which one the operator \
             meant"
                .into(),
        )),
        (Some(c), None) => Ok(c.clone()),
        (None, Some(inc)) => {
            let cams: Vec<(String,)> = sqlx::query_as(
                "SELECT DISTINCT camera_id FROM segments WHERE incident_id = ? ORDER BY camera_id",
            )
            .bind(inc)
            .fetch_all(&st.pool)
            .await?;
            let scoped = principal.camera_scope().is_some();
            let all_visible = cams.iter().all(|c| principal.camera_allowed(&c.0));

            // One refusal for every "not exactly one camera you hold" outcome, so a scoped caller
            // cannot tell an incident that does not exist from one on somebody else's camera from
            // one spanning cameras they do not hold. Byte for byte identical — the count, the
            // camera id and the incident id all stay out of it.
            if scoped && !(all_visible && !cams.is_empty()) {
                return Err(crate::state::scope_denied_owner(
                    "incident",
                    "export evidence for this incident",
                ));
            }

            match cams.len() {
                0 => Err(AppError::NotFound(format!(
                    "incident {inc} has no recorded segments"
                ))),
                1 => Ok(cams[0].0.clone()),
                // A multi-camera incident is a real thing, but a bundle attests to ONE camera's
                // footage. Picking one silently would produce a document whose scope is narrower
                // than the incident an investigator believes it covers.
                //
                // Reachable by a scoped caller only when they hold EVERY camera the incident spans,
                // in which case the count tells them nothing they could not already count.
                _ => Err(AppError::BadRequest(format!(
                    "incident {inc} spans {} cameras — export one bundle per camera by naming \
                     `camera_id`, so each bundle attests to footage it actually contains",
                    cams.len()
                ))),
            }
        }
        (None, None) => Err(AppError::BadRequest(
            "one of `camera_id` or `incident_id` is required".into(),
        )),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListQuery {
    camera_id: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// Bundles this credential may see. Camera-scoped: a scoped credential gets its cameras only.
#[utoipa::path(
    get, path = "/api/v1/evidence/exports", tag = "evidence",
    operation_id = "listEvidenceExports",
    params(
        ("camera_id" = Option<String>, Query, description = "Restrict to one camera"),
        ("limit" = Option<i64>, Query, description = "Max rows (1..=500, default 100)"),
    ),
    responses(
        (status = 200, description = "Exported bundles, newest first"),
        (status = 403, description = "Missing `video:export`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<serde_json::Value>> {
    principal.require_cap(Cap::VideoExport, "list evidence bundles")?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);

    let mut sql = String::from(
        "SELECT id, camera_id, site_id, incident_id, filename, from_time, to_time, size_bytes, \
         sha256, manifest_sha256, key_id, exported_by, audit_id, request_id, created_at \
         FROM evidence_bundles WHERE 1=1",
    );
    let mut binds: Vec<String> = Vec::new();
    if let Some(c) = &q.camera_id {
        st.camera_scope_check(&principal, c)?;
        sql.push_str(" AND camera_id = ?");
        binds.push(c.clone());
    }
    // Read confinement: an unfiltered list is fleet-wide, so a scoped credential gets its cameras
    // and nothing else — the same shape as every other scoped read.
    if let crate::auth::Scope::Cameras(allowed) = &principal.scope {
        if allowed.is_empty() {
            return Ok(Json(json!({"bundles": []})));
        }
        sql.push_str(" AND camera_id IN (");
        sql.push_str(&vec!["?"; allowed.len()].join(","));
        sql.push(')');
        binds.extend(allowed.iter().cloned());
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");

    let mut qy = sqlx::query_as::<_, BundleRow>(&sql);
    for b in &binds {
        qy = qy.bind(b);
    }
    let rows = qy.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(json!({"bundles": rows})))
}

/// One bundle's record. An out-of-scope bundle is refused rather than described.
#[utoipa::path(
    get, path = "/api/v1/evidence/exports/{id}", tag = "evidence",
    operation_id = "getEvidenceExport",
    params(("id" = String, Path, description = "Bundle id")),
    responses(
        (status = 200, description = "The bundle record"),
        (status = 403, description = "A bundle for a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown bundle", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_one(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<BundleRow>> {
    principal.require_cap(Cap::VideoExport, "read an evidence bundle")?;
    let row: Option<BundleRow> = sqlx::query_as(
        "SELECT id, camera_id, site_id, incident_id, filename, from_time, to_time, size_bytes, \
         sha256, manifest_sha256, key_id, exported_by, audit_id, request_id, created_at \
         FROM evidence_bundles WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&st.pool)
    .await?;
    let missing = || AppError::NotFound(format!("evidence bundle {id} not found"));
    let row = row.ok_or_else(missing)?;
    // The bundle row names its camera; a scoped caller must hold it. Without this, a bundle id is a
    // way to learn another camera's export window, size and hash.
    //
    // The refusal is 404 and NOT 403, because 403-here/404-there is an existence oracle: a scoped
    // caller could walk the id space and learn exactly which windows of another camera's footage
    // have been exported. The route census caught this — the first version of this handler returned
    // the scope check's own 403 and told a probing credential which bundle ids were real.
    if st.camera_scope_check(&principal, &row.camera_id).is_err() {
        return Err(missing());
    }
    Ok(Json(row))
}

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct BundleRow {
    id: String,
    camera_id: String,
    site_id: Option<String>,
    incident_id: Option<String>,
    filename: String,
    from_time: String,
    to_time: String,
    size_bytes: i64,
    sha256: String,
    manifest_sha256: String,
    key_id: String,
    exported_by: String,
    audit_id: Option<String>,
    request_id: Option<String>,
    created_at: String,
}

/// The appliance's evidence-signing public key.
///
/// Deliberately readable by any authenticated principal, camera-scoped included: a public key is
/// public by construction, and someone verifying a bundle needs it. It is `CameraRead` rather than
/// unauthenticated only because the key identifies the appliance.
#[utoipa::path(
    get, path = "/api/v1/evidence/signing-key", tag = "evidence",
    operation_id = "getEvidenceSigningKey",
    responses(
        (status = 200, description = "The appliance's Ed25519 evidence-signing public key and its id"),
        (status = 403, description = "Missing `camera:read`", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn signing_key(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<serde_json::Value>> {
    principal.require_cap(Cap::CameraRead, "read the evidence signing key")?;
    let key = crate::services::evidence_key::EvidenceKey::load_or_create(&st.cfg.data_dir)
        .map_err(AppError::Other)?;
    Ok(Json(json!({
        "algorithm": "ed25519",
        "key_id": key.key_id,
        "public_key": key.public_key_b64,
        "format": evidence::FORMAT,
        "note": "Publish this key out of band. A bundle verified against a key taken from the same \
                 place as the bundle proves only that they were produced together.",
    })))
}
