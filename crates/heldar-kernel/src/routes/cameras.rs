use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;
use sqlx::SqlitePool;

use crate::auth::{self, Cap, Principal};
use crate::camera_url;
use crate::error::{AppError, AppResult};
use crate::models::{Camera, CameraCreate, CameraUpdate, CameraView};
use crate::state::AppState;
use crate::util;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/cameras", get(list_cameras).post(create_camera))
        .route(
            "/api/v1/cameras/{id}",
            get(get_camera).patch(update_camera).delete(delete_camera),
        )
        .route(
            "/api/v1/cameras/{id}/test",
            get(test_camera).post(test_camera),
        )
}

/// Accepted `record_mode` values. `event` / `scheduled_event` event-triggering is wired in a later
/// batch; this batch honors `continuous` (always) and the time-of-day window for `scheduled` /
/// `scheduled_event`.
fn validate_record_mode(mode: &str) -> AppResult<()> {
    if matches!(
        mode,
        "continuous" | "scheduled" | "event" | "scheduled_event"
    ) {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "`record_mode` must be continuous|scheduled|event|scheduled_event".into(),
        ))
    }
}

/// Reject a camera `address` containing whitespace or control characters. The address flows into the
/// RTSP URL handed to the kernel-spawned ffmpeg as a single argv element (services/live_publisher.rs —
/// no whitespace splitting), but a hostname/IP never legitimately contains them, so this stays as
/// input validation + defense in depth.
fn validate_address(address: Option<&str>) -> AppResult<()> {
    if let Some(a) = address {
        if a.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(AppError::BadRequest(
                "`address` must not contain whitespace or control characters".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) async fn load_camera(pool: &SqlitePool, id: &str) -> AppResult<Camera> {
    sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("camera {id} not found")))
}

/// Refuse a BOX-LEVEL action to a camera-scoped credential.
///
/// The camera-scope loaders ([`AppState::camera_for`], `resource_camera`, `camera_scope_filter`)
/// all answer the question "may this credential touch camera X". Some actions have no camera to ask
/// about: a LAN sweep that enrolls whatever it finds, a WS-Discovery probe of the whole segment.
/// A camera-scoped credential can never legitimately reach those — by construction the devices they
/// enroll are not in its allowlist, and their results disclose the existence of cameras outside it
/// (network discovery flags each address `already_registered`, which is the fleet roster in address
/// space). Refusing outright is the only containment available, exactly as for an off-box backup
/// destination.
///
/// Unscoped credentials — every human role, and every key minted without a camera list — are
/// unaffected, and with auth disabled the principal is the unscoped system admin, so this is a
/// structural no-op on the LAN default rather than a promise.
pub fn require_fleet_scope(principal: &Principal, action: &str) -> AppResult<()> {
    if principal.camera_scope().is_some() {
        return Err(AppError::Forbidden(format!(
            "credential is scoped to specific cameras and cannot {action}"
        )));
    }
    Ok(())
}

/// List cameras.
///
/// Camera-scoped: a credential carrying a camera list sees only those cameras. The refusal is a
/// filtered list, not a 403 — a complete inventory would make the per-camera 403s pointless.
#[utoipa::path(
    get, path = "/api/v1/cameras", tag = "cameras",
    responses(
        (status = 200, description = "Cameras visible to this credential", body = [CameraView]),
        (status = 401, description = "No credential", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `camera:read`", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_cameras(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<CameraView>>> {
    principal.require_cap(Cap::CameraRead, "list cameras")?;
    // A camera-scoped credential sees only its cameras here — otherwise the list would be a complete
    // inventory disclosure that the per-camera 403 then pointlessly guards.
    let mut sql = "SELECT * FROM cameras WHERE 1=1".to_string();
    let scope = crate::state::camera_scope_filter(&principal, "id");
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY id ASC");
    let mut q = sqlx::query_as::<_, Camera>(&sql);
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        q = q.bind(id);
    }
    let cams = q.fetch_all(&st.pool).await?;
    Ok(Json(cams.into_iter().map(CameraView::from).collect()))
}

/// Fetch one camera.
///
/// An out-of-scope camera answers exactly as an unknown one (404), so the response cannot be used to
/// discover which cameras exist.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}", tag = "cameras",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The camera", body = CameraView),
        (status = 404, description = "Unknown camera, or one this credential does not hold", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_camera(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<CameraView>> {
    principal.require_cap(Cap::CameraRead, "view a camera")?;
    Ok(Json(st.camera_for(&principal, &id).await?.into()))
}

async fn create_camera(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<CameraCreate>,
) -> AppResult<(StatusCode, Json<CameraView>)> {
    principal.require(principal.can_manage_registry(), "create cameras")?;
    let id = body
        .id
        .as_deref()
        .map(util::slugify)
        .unwrap_or_else(|| util::slugify(&body.name));
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    // Placing a camera on a site decides which clock its recording schedule is read in (#125), and
    // an unknown site_id 400s on the foreign key while a real one succeeds — so allowing it here
    // would hand a camera-scoped credential the same site-enumeration oracle the update path had.
    // Enroll the camera, then move it with a fleet-wide credential.
    if body.site_id.is_some() {
        require_fleet_scope(&principal, "place a camera on a site")?;
    }
    // A camera-scoped credential may only mint a camera it ALREADY holds. Without this a
    // `scope_kind: cameras` key carrying RegistryManage could enroll cameras outside its allowlist —
    // spawning a recorder, a live publisher and a capability probe for a camera it does not hold, and
    // then failing the read-back at the end of this handler, leaving an orphan behind.
    //
    // Placed BEFORE the existence probe on purpose: the 409 below distinguishes "id taken" from "id
    // free", so checking scope afterwards would turn camera creation into an id-space oracle. The id
    // is the caller's own input, so naming it in the refusal discloses nothing.
    principal.require_camera(&id, "create cameras")?;

    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM cameras WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?;
    if exists.is_some() {
        return Err(AppError::Conflict(format!(
            "camera id `{id}` already exists"
        )));
    }

    let record_stream = body.record_stream.unwrap_or_else(|| "main".into());
    if !matches!(record_stream.as_str(), "main" | "sub") {
        return Err(AppError::BadRequest(
            "`record_stream` must be 'main' or 'sub'".into(),
        ));
    }
    for url in [
        body.main_stream_url.as_deref(),
        body.sub_stream_url.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        camera_url::validate_stream_url(url).map_err(AppError::BadRequest)?;
    }

    let now = Utc::now();
    let caps = SqlxJson(body.capabilities.unwrap_or_else(|| json!({})));
    let rtsp_port = body.rtsp_port.unwrap_or(554);
    let record_enabled = body.record_enabled.unwrap_or(true);
    let enabled = body.enabled.unwrap_or(true);
    let seg = body
        .segment_seconds
        .unwrap_or(st.cfg.default_segment_seconds)
        .clamp(2, 3600);
    let retention = body
        .retention_hours
        .unwrap_or(st.cfg.default_retention_hours)
        .max(1);
    // Fall back to the configured default quota when omitted; a default of 0 means "no quota" and is
    // stored as NULL (no per-camera cap).
    let storage_quota_bytes =
        body.storage_quota_bytes
            .or_else(|| match st.cfg.default_camera_quota_bytes {
                0 => None,
                q => Some(q as i64),
            });
    let record_audio = body.record_audio.unwrap_or(st.cfg.default_record_audio);
    let record_mode = body.record_mode.unwrap_or_else(|| "continuous".into());
    validate_record_mode(&record_mode)?;
    let pre_roll_seconds = body
        .pre_roll_seconds
        .unwrap_or(st.cfg.default_pre_roll_seconds)
        .clamp(0, 300);
    let post_roll_seconds = body
        .post_roll_seconds
        .unwrap_or(st.cfg.default_post_roll_seconds)
        .clamp(0, 3600);
    let mirror_enabled = body.mirror_enabled.unwrap_or(false);
    let anr_enabled = body.anr_enabled.unwrap_or(false);
    let native_anpr_enabled = body.native_anpr_enabled.unwrap_or(false);
    let native_events_enabled = body.native_events_enabled.unwrap_or(false);
    let anr_replay_url_template = body
        .anr_replay_url_template
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // The address flows into the RTSP URL (and thence the MediaMTX ffmpeg command); reject whitespace/
    // control chars that could inject ffmpeg args. The ANR replay template is passed straight to
    // `ffmpeg -i`, so hold it to the same scheme allow-list as stream URLs (blocks file:/gopher:/…).
    validate_address(body.address.as_deref())?;
    if let Some(tpl) = anr_replay_url_template.as_deref() {
        camera_url::validate_stream_url(tpl)
            .map_err(|e| AppError::BadRequest(format!("`anr_replay_url_template`: {e}")))?;
    }

    // Encrypt the camera password at rest when HELDAR_SECRET_KEY is configured (plaintext otherwise).
    let password = body
        .password
        .as_deref()
        .map(crate::services::secrets::encrypt_for_storage)
        .transpose()?;

    sqlx::query(
        "INSERT INTO cameras
           (id, site_id, name, vendor, model, address, rtsp_port, username, password,
            main_stream_url, sub_stream_url, record_stream, capabilities, record_enabled,
            segment_seconds, retention_hours, storage_quota_bytes, record_audio, record_mode,
            pre_roll_seconds, post_roll_seconds, mirror_enabled, anr_enabled, anr_replay_url_template,
            native_anpr_enabled, native_events_enabled, enabled, live_warm, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&body.site_id)
    .bind(&body.name)
    .bind(&body.vendor)
    .bind(&body.model)
    .bind(&body.address)
    .bind(rtsp_port)
    .bind(&body.username)
    .bind(&password)
    .bind(&body.main_stream_url)
    .bind(&body.sub_stream_url)
    .bind(&record_stream)
    .bind(caps)
    .bind(record_enabled)
    .bind(seg)
    .bind(retention)
    .bind(storage_quota_bytes)
    .bind(record_audio)
    .bind(&record_mode)
    .bind(pre_roll_seconds)
    .bind(post_roll_seconds)
    .bind(mirror_enabled)
    .bind(anr_enabled)
    .bind(&anr_replay_url_template)
    .bind(native_anpr_enabled)
    .bind(native_events_enabled)
    .bind(enabled)
    .bind(body.live_warm.unwrap_or(false))
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;

    sqlx::query(
        "INSERT INTO camera_status (camera_id, state, updated_at) VALUES (?, 'unknown', ?)
         ON CONFLICT(camera_id) DO NOTHING",
    )
    .bind(&id)
    .bind(now)
    .execute(&st.pool)
    .await?;

    st.recorder.reconcile(&id).await;
    if let Some(m) = &st.mirror {
        m.reconcile(&id).await;
    }
    st.live.reconcile(&id).await;
    // Discover the device's control capabilities in the background (day/night, lighting, relay
    // outputs, built-in detections, on-board ANPR) so the dashboard's Device panel is populated
    // without anyone pressing "Detect features". Best-effort: never blocks or fails the create.
    crate::services::camera_control::spawn_probe(&st, &id);
    let cam = st.camera_for(&principal, &id).await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_camera",
        "camera",
        &id,
        json!({ "name": &body.name, "vendor": &body.vendor }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(cam.into())))
}

pub async fn update_camera(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<CameraUpdate>,
) -> AppResult<Json<CameraView>> {
    principal.require(principal.can_manage_registry(), "update cameras")?;
    let cur = st.camera_for(&principal, &id).await?;

    let record_stream = body.record_stream.unwrap_or(cur.record_stream);
    if !matches!(record_stream.as_str(), "main" | "sub") {
        return Err(AppError::BadRequest(
            "`record_stream` must be 'main' or 'sub'".into(),
        ));
    }

    let name = body.name.unwrap_or(cur.name);
    // Absent leaves it; explicit null detaches. See `CameraUpdate::site_id` — a camera's site
    // carries the clock its schedule is read in, so this is not a label edit.
    let previous_site = cur.site_id.clone();
    let site_id = match body.site_id {
        None => cur.site_id,
        Some(v) => v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
    };

    // MOVING A CAMERA BETWEEN SITES MOVES THE HOURS IT RECORDS, so it is fleet-only and reported.
    //
    // `PATCH /api/v1/sites/{id}` guards and announces exactly this effect behind admin + fleet
    // scope. Reaching the same effect through the camera required only `RegistryManage` and a
    // camera scope, said nothing in the response, and audited `{}` — and, because an unknown
    // site_id 400s on the foreign key while a real one succeeds, it also let a scoped credential
    // enumerate every site id on the box. Refusing uniformly closes both.
    if site_id != previous_site {
        crate::routes::cameras::require_fleet_scope(&principal, "move a camera between sites")?;
    }
    let vendor = body.vendor.unwrap_or(cur.vendor);
    let model = body.model.or(cur.model);
    let address = body.address.or(cur.address);
    let rtsp_port = body.rtsp_port.unwrap_or(cur.rtsp_port);
    let username = body.username.or(cur.username);
    // A new password is plaintext from the client → encrypt at rest; otherwise keep the stored value
    // (already in its at-rest form — do not re-encrypt).
    let password = match body.password {
        Some(p) => Some(crate::services::secrets::encrypt_for_storage(&p)?),
        None => cur.password,
    };
    let main_stream_url = body.main_stream_url.or(cur.main_stream_url);
    let sub_stream_url = body.sub_stream_url.or(cur.sub_stream_url);
    for url in [main_stream_url.as_deref(), sub_stream_url.as_deref()]
        .into_iter()
        .flatten()
    {
        camera_url::validate_stream_url(url).map_err(AppError::BadRequest)?;
    }
    validate_address(address.as_deref())?;
    let caps = SqlxJson(body.capabilities.unwrap_or(cur.capabilities.0));
    let record_enabled = body.record_enabled.unwrap_or(cur.record_enabled);
    let enabled = body.enabled.unwrap_or(cur.enabled);
    let priority = body.priority.unwrap_or(cur.priority);
    let live_warm = body.live_warm.unwrap_or(cur.live_warm);
    let seg = body
        .segment_seconds
        .map(|v| v.clamp(2, 3600))
        .unwrap_or(cur.segment_seconds);
    // Clamped at both ends. The upper bound is belt-and-braces rather than the real fix — retention
    // eviction is now fair-share, so a camera opting out of age-pruning only spends its OWN share of
    // the disk instead of pushing deletion onto whoever is next-oldest. But an unbounded value is
    // still nonsense (10 years on a 2 TB box), and the pre-fix version of this line was the write
    // that a scoped credential used to destroy another camera's footage entirely.
    const MAX_RETENTION_HOURS: i64 = 24 * 365 * 5;
    let retention = body
        .retention_hours
        .map(|v| v.clamp(1, MAX_RETENTION_HOURS))
        .unwrap_or(cur.retention_hours);
    let storage_quota_bytes = body.storage_quota_bytes.or(cur.storage_quota_bytes);
    let record_audio = body.record_audio.unwrap_or(cur.record_audio);
    let record_mode = body.record_mode.unwrap_or(cur.record_mode);
    validate_record_mode(&record_mode)?;
    let pre_roll_seconds = body
        .pre_roll_seconds
        .map(|v| v.clamp(0, 300))
        .unwrap_or(cur.pre_roll_seconds);
    let post_roll_seconds = body
        .post_roll_seconds
        .map(|v| v.clamp(0, 3600))
        .unwrap_or(cur.post_roll_seconds);
    let mirror_enabled = body.mirror_enabled.unwrap_or(cur.mirror_enabled);
    let anr_enabled = body.anr_enabled.unwrap_or(cur.anr_enabled);
    let native_anpr_enabled = body.native_anpr_enabled.unwrap_or(cur.native_anpr_enabled);
    let native_events_enabled = body
        .native_events_enabled
        .unwrap_or(cur.native_events_enabled);
    let anr_replay_url_template = body
        .anr_replay_url_template
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(cur.anr_replay_url_template);
    if let Some(tpl) = anr_replay_url_template.as_deref() {
        camera_url::validate_stream_url(tpl)
            .map_err(|e| AppError::BadRequest(format!("`anr_replay_url_template`: {e}")))?;
    }

    sqlx::query(
        "UPDATE cameras SET
            name=?, site_id=?, vendor=?, model=?, address=?, rtsp_port=?, username=?, password=?,
            main_stream_url=?, sub_stream_url=?, record_stream=?, capabilities=?, record_enabled=?,
            segment_seconds=?, retention_hours=?, storage_quota_bytes=?, record_audio=?, record_mode=?,
            pre_roll_seconds=?, post_roll_seconds=?, mirror_enabled=?, anr_enabled=?,
            anr_replay_url_template=?, native_anpr_enabled=?, native_events_enabled=?, enabled=?, priority=?, live_warm=?, updated_at=?
         WHERE id=?",
    )
    .bind(&name)
    .bind(&site_id)
    .bind(&vendor)
    .bind(&model)
    .bind(&address)
    .bind(rtsp_port)
    .bind(&username)
    .bind(&password)
    .bind(&main_stream_url)
    .bind(&sub_stream_url)
    .bind(&record_stream)
    .bind(caps)
    .bind(record_enabled)
    .bind(seg)
    .bind(retention)
    .bind(storage_quota_bytes)
    .bind(record_audio)
    .bind(&record_mode)
    .bind(pre_roll_seconds)
    .bind(post_roll_seconds)
    .bind(mirror_enabled)
    .bind(anr_enabled)
    .bind(&anr_replay_url_template)
    .bind(native_anpr_enabled)
    .bind(native_events_enabled)
    .bind(enabled)
    .bind(priority)
    .bind(live_warm)
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;

    st.recorder.reconcile(&id).await;
    if let Some(m) = &st.mirror {
        m.reconcile(&id).await;
    }
    // A disable / URL change / enable also affects AI sampling for this camera.
    st.sampler.reconcile().await;
    // …and the live preview publisher (warm toggle, enable/disable, credential/URL change).
    st.live.reconcile(&id).await;
    // Re-discover device capabilities in the background — an address/credential/vendor change can
    // change what the camera exposes. Cheap (a few LAN calls) and best-effort.
    crate::services::camera_control::spawn_probe(&st, &id);
    // The audit row carried `{}`, so a site change — which moves the camera's recording windows —
    // left no record of what it had been.
    let site_moved = site_id != previous_site;
    let (new_tz, _) = if site_moved {
        crate::services::tz::site_tz(&st.pool, Some(&id)).await
    } else {
        (None, crate::services::tz::TzSource::Unset)
    };
    auth::audit(
        &st.pool,
        &principal,
        "update_camera",
        "camera",
        &id,
        json!({
            "site_changed": site_moved,
            "previous_site_id": previous_site,
            "site_id": site_id,
            "timezone_now": new_tz.map(|t| t.to_string()),
        }),
    )
    .await;
    if site_moved {
        tracing::warn!(
            target: "heldar::security",
            camera = %id,
            from = ?previous_site,
            to = ?site_id,
            "cameras: site changed — this camera's recording schedule now follows a different clock"
        );
    }
    Ok(Json(st.camera_for(&principal, &id).await?.into()))
}

/// Delete a camera and its registry rows.
#[utoipa::path(
    delete, path = "/api/v1/cameras/{id}", tag = "cameras",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Not permitted, or outside this credential's camera scope", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn delete_camera(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    // Deleting a camera PURGES its recordings, so it is an admin action, not a manager one. It is
    // also the one path that could destroy footage the retention sweeper is forbidden to touch.
    principal.require_cap(Cap::Admin, "delete cameras (purges all of their footage)")?;
    let _ = st.camera_for(&principal, &id).await?; // 404 if missing

    // EVIDENCE HOLD. Evidence-locked segments are protected from retention, but nothing stopped a
    // camera delete from removing the recordings directory out from under them — so the lock held
    // right up until someone removed the camera, which is precisely when footage under investigation
    // would be lost. Refuse while a hold exists and say how to release it, rather than destroying
    // evidence and writing an audit row about it afterwards.
    let held: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM segments WHERE camera_id = ? AND evidence_locked = 1",
    )
    .bind(&id)
    .fetch_one(&st.pool)
    .await
    .unwrap_or(0);
    if held > 0 {
        return Err(AppError::Conflict(format!(
            "camera {id} has {held} evidence-locked segment(s) and cannot be deleted. Release the \
             hold first (DELETE /api/v1/segments/{{segment_id}}/evidence-lock), then retry \
             — or keep the camera and disable it instead."
        )));
    }

    st.recorder.stop(&id).await;
    if let Some(m) = &st.mirror {
        m.stop(&id).await;
    }
    // Clean up zone-event evidence files + rows for this camera (zone_events has no FK cascade).
    let evidence: Vec<(Option<String>,)> =
        sqlx::query_as("SELECT evidence_path FROM zone_events WHERE camera_id = ?")
            .bind(&id)
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default();
    for (ev,) in &evidence {
        if let Some(name) = ev.as_deref().and_then(|u| u.rsplit('/').next()) {
            let _ = tokio::fs::remove_file(st.cfg.snapshots_dir.join(name)).await;
        }
    }
    let _ = sqlx::query("DELETE FROM zone_events WHERE camera_id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await;
    // Unlink this camera's embedding crop thumbs BEFORE the cameras delete: the embeddings rows
    // cascade away with the camera, and the rows are the only reference to the thumb files.
    let emb_evidence: Vec<(Option<String>,)> =
        sqlx::query_as("SELECT evidence_path FROM embeddings WHERE camera_id = ?")
            .bind(&id)
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default();
    for (ev,) in &emb_evidence {
        if let Some(name) = ev.as_deref().and_then(|u| u.rsplit('/').next()) {
            let _ = tokio::fs::remove_file(st.cfg.snapshots_dir.join(name)).await;
        }
    }
    sqlx::query("DELETE FROM cameras WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    // Stop any AI sampler for this camera (its ai_tasks cascade-deleted) and remove its on-disk data.
    st.sampler.reconcile().await;
    // Stop the live publisher and remove the camera's MediaMTX path (reconcile sees the row is gone).
    st.live.reconcile(&id).await;
    let _ = tokio::fs::remove_dir_all(st.cfg.camera_recordings_dir(&id)).await;
    let _ = tokio::fs::remove_dir_all(st.cfg.camera_frames_dir(&id)).await;
    if let Some(dir) = &st.cfg.mirror_recordings_dir {
        let _ = tokio::fs::remove_dir_all(dir.join(&id)).await;
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_camera",
        "camera",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Probe the camera's recording stream to confirm reachability and read its codec/dimensions.
async fn test_camera(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::CameraRead, "test camera connectivity")?;
    let cam = st.camera_for(&principal, &id).await?;
    let url = camera_url::record_url(&cam)
        .ok_or_else(|| AppError::BadRequest("camera has no stream URL".into()))?;

    let probe = tokio::time::timeout(
        std::time::Duration::from_secs(12),
        util::ffprobe_stream(&st.cfg.ffprobe_bin, &url),
    )
    .await;

    let result = match probe {
        Ok(Ok(info)) => json!({
            "reachable": true,
            "codec": info.codec,
            "width": info.width,
            "height": info.height,
            "url": camera_url::mask_url(&url),
        }),
        Ok(Err(e)) => json!({
            "reachable": false,
            "error": camera_url::mask_url(&e.to_string()),
            "url": camera_url::mask_url(&url),
        }),
        Err(_) => json!({
            "reachable": false,
            "error": "probe timed out after 12s",
            "url": camera_url::mask_url(&url),
        }),
    };
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::services::recorder::RecorderManager;
    use crate::services::sampler::SamplerManager;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use tower::Service;

    /// Build a minimal in-memory AppState (single-connection so migrations persist) with auth
    /// toggled, for exercising the route-level Principal gate end to end.
    async fn test_state(auth_enabled: bool) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let mut cfg = Config::from_env();
        cfg.auth_enabled = auth_enabled;
        let cfg = Arc::new(cfg);
        AppState {
            recorder: RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: SamplerManager::new(pool.clone(), cfg.clone()),
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
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    /// Send an unauthenticated GET /api/v1/cameras through the real router and report the status.
    async fn unauthenticated_list_status(auth_enabled: bool) -> StatusCode {
        let st = test_state(auth_enabled).await;
        let mut app = super::router().with_state(st);
        let req = Request::builder()
            .method("GET")
            .uri("/api/v1/cameras")
            .body(Body::empty())
            .unwrap();
        app.call(req).await.unwrap().status()
    }

    /// With auth ENABLED, an unauthenticated request to a representative legacy route is rejected by
    /// the Principal extractor (401) — the auth gap this batch closes.
    #[tokio::test]
    async fn legacy_route_rejects_unauthenticated_when_auth_enabled() {
        assert_eq!(
            unauthenticated_list_status(true).await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// With auth DISABLED the Principal is the permissive system admin, so the new `require()` guard
    /// is a behavioral NO-OP and the legacy route stays open (200).
    #[tokio::test]
    async fn legacy_route_open_when_auth_disabled() {
        assert_eq!(unauthenticated_list_status(false).await, StatusCode::OK);
    }

    async fn seed_camera_with_segment(st: &AppState, cam: &str, locked: bool) {
        let now = chrono::Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(cam)
            .bind("Held Camera")
            .bind(now)
            .bind(now)
            .execute(&st.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, container,
                                   size_bytes, created_at, evidence_locked)
             VALUES ('seg_hold', ?, '/x.mp4', ?, ?, 5.0, 'mp4', 0, ?, ?)",
        )
        .bind(cam)
        .bind(now)
        .bind(now + chrono::Duration::seconds(5))
        .bind(now)
        .bind(locked)
        .execute(&st.pool)
        .await
        .unwrap();
    }

    async fn delete_camera_status(st: AppState, cam: &str) -> StatusCode {
        let mut app = super::router().with_state(st);
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/cameras/{cam}"))
            .body(Body::empty())
            .unwrap();
        app.call(req).await.unwrap().status()
    }

    /// An evidence hold outranks a camera delete. Locked segments are protected from the retention
    /// sweeper, but deleting the camera used to remove the recordings directory anyway — losing
    /// footage under investigation at exactly the moment someone decommissioned the camera.
    #[tokio::test]
    async fn a_camera_with_held_evidence_cannot_be_deleted() {
        let st = test_state(false).await; // auth off -> system admin, so this is the HOLD, not RBAC
        seed_camera_with_segment(&st, "cam_held", true).await;

        assert_eq!(
            delete_camera_status(st.clone(), "cam_held").await,
            StatusCode::CONFLICT
        );
        // ...and the camera is still there. A refused delete must not be a partial delete.
        let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE id = 'cam_held'")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(still, 1, "a refused delete must leave the camera intact");
    }

    fn scoped(cameras: &[&str]) -> crate::auth::Principal {
        let set: std::collections::HashSet<String> =
            cameras.iter().map(|c| c.to_string()).collect();
        crate::auth::Principal {
            scope: crate::auth::Scope::Cameras(Arc::new(set)),
            ..crate::auth::Principal::system_admin()
        }
    }

    fn create_body(id: &str) -> crate::models::CameraCreate {
        serde_json::from_value(serde_json::json!({ "id": id, "name": id })).unwrap()
    }

    /// A `scope_kind: cameras` key carrying RegistryManage could enroll cameras OUTSIDE its
    /// allowlist: the insert succeeded, the recorder / live publisher / capability probe all started
    /// for a camera the caller does not hold, and only the read-back at the end of the handler
    /// failed — leaving an orphan behind. The refusal must come first.
    #[tokio::test]
    async fn a_scoped_key_cannot_enroll_a_camera_outside_its_scope() {
        let st = test_state(false).await;
        let p = scoped(&["cam_a"]);

        let err = super::create_camera(
            axum::extract::State(st.clone()),
            p.clone(),
            axum::Json(create_body("cam_sentinel_b")),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, crate::error::AppError::Forbidden(_)));
        // Nothing was created, and no recorder/publisher was reconciled for it.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE id = 'cam_sentinel_b'")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "a refused create must not be a partial create");

        // No over-blocking: a camera the credential DOES hold still enrolls.
        let (status, _) = super::create_camera(
            axum::extract::State(st.clone()),
            p,
            axum::Json(create_body("cam_a")),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
    }

    /// The scope check sits BEFORE the id-collision probe, so `create` cannot be used to ask "does
    /// this camera id already exist" one probe at a time: a taken out-of-scope id and a free
    /// out-of-scope id must produce the same refusal, not 409-vs-403.
    #[tokio::test]
    async fn camera_create_is_not_an_id_space_oracle() {
        let st = test_state(false).await;
        seed_camera_with_segment(&st, "cam_sentinel_b", false).await;
        let p = scoped(&["cam_a"]);

        let taken = super::create_camera(
            axum::extract::State(st.clone()),
            p.clone(),
            axum::Json(create_body("cam_sentinel_b")),
        )
        .await
        .unwrap_err();
        let free = super::create_camera(
            axum::extract::State(st.clone()),
            p,
            axum::Json(create_body("cam_zzz")),
        )
        .await
        .unwrap_err();
        assert!(matches!(taken, crate::error::AppError::Forbidden(_)));
        assert!(matches!(free, crate::error::AppError::Forbidden(_)));
        // Same SHAPE; the messages differ only by the caller's own input id, which it already knows.
        assert!(taken.to_string().contains("cam_sentinel_b"));
        assert!(free.to_string().contains("cam_zzz"));
    }

    /// CONSTRAINT 2: an unscoped credential is unaffected — it still enrolls anything, and a
    /// duplicate id is still the pre-existing 409.
    #[tokio::test]
    async fn an_unscoped_credential_still_enrolls_any_camera() {
        let st = test_state(false).await;
        let admin = crate::auth::Principal::system_admin();
        let (status, _) = super::create_camera(
            axum::extract::State(st.clone()),
            admin.clone(),
            axum::Json(create_body("cam_sentinel_b")),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        let dup = super::create_camera(
            axum::extract::State(st.clone()),
            admin,
            axum::Json(create_body("cam_sentinel_b")),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(dup, crate::error::AppError::Conflict(_)),
            "{dup:?}"
        );
    }

    /// `require_fleet_scope` is the containment for actions that have NO camera to scope by. It must
    /// be a discriminant compare that is a no-op for every unscoped principal — including the system
    /// admin the auth-disabled LAN default runs as.
    #[test]
    fn require_fleet_scope_is_a_no_op_for_an_unscoped_principal() {
        assert!(
            super::require_fleet_scope(&crate::auth::Principal::system_admin(), "scan").is_ok(),
            "auth-disabled operation must be unchanged"
        );
        let err = super::require_fleet_scope(&scoped(&["cam_a"]), "scan").unwrap_err();
        assert!(matches!(err, crate::error::AppError::Forbidden(_)));
        // Even an EMPTY camera list is a scope, and is refused rather than read as "unrestricted".
        assert!(super::require_fleet_scope(&scoped(&[]), "scan").is_err());
    }

    /// Roster containment for the camera list itself: a scoped credential must not be handed the ids
    /// of cameras it does not hold, since those ids are the input every camera-keyed route takes.
    #[tokio::test]
    async fn the_camera_list_is_confined_to_the_credentials_cameras() {
        let st = test_state(false).await;
        seed_camera_with_segment(&st, "cam_a", false).await;
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
            .bind("cam_sentinel_b")
            .bind("B")
            .bind(chrono::Utc::now())
            .bind(chrono::Utc::now())
            .execute(&st.pool)
            .await
            .unwrap();

        let scoped_list = super::list_cameras(axum::extract::State(st.clone()), scoped(&["cam_a"]))
            .await
            .unwrap()
            .0;
        let body = serde_json::to_string(&scoped_list).unwrap();
        assert!(!body.contains("cam_sentinel_b"), "{body}");

        // ...and the unscoped view is unchanged.
        let all = super::list_cameras(
            axum::extract::State(st.clone()),
            crate::auth::Principal::system_admin(),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(all.len(), 2);
    }

    /// THE POSITIVE CONTROL: without a hold the delete still works. Over-correcting into "cameras
    /// can never be deleted" would be its own bug.
    #[tokio::test]
    async fn a_camera_without_held_evidence_still_deletes() {
        let st = test_state(false).await;
        seed_camera_with_segment(&st, "cam_free", false).await;
        assert_eq!(
            delete_camera_status(st.clone(), "cam_free").await,
            StatusCode::NO_CONTENT
        );
        let gone: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE id = 'cam_free'")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(gone, 0);
    }
}
