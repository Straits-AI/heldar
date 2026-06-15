use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;
use sqlx::SqlitePool;

use crate::auth::{self, Principal};
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
            get(get_camera_handler)
                .patch(update_camera)
                .delete(delete_camera),
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

pub(crate) async fn load_camera(pool: &SqlitePool, id: &str) -> AppResult<Camera> {
    sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("camera {id} not found")))
}

async fn list_cameras(State(st): State<AppState>) -> AppResult<Json<Vec<CameraView>>> {
    let cams = sqlx::query_as::<_, Camera>("SELECT * FROM cameras ORDER BY id ASC")
        .fetch_all(&st.pool)
        .await?;
    Ok(Json(cams.into_iter().map(CameraView::from).collect()))
}

async fn get_camera_handler(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<CameraView>> {
    Ok(Json(load_camera(&st.pool, &id).await?.into()))
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
    let anr_replay_url_template = body
        .anr_replay_url_template
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    sqlx::query(
        "INSERT INTO cameras
           (id, site_id, name, vendor, model, address, rtsp_port, username, password,
            main_stream_url, sub_stream_url, record_stream, capabilities, record_enabled,
            segment_seconds, retention_hours, storage_quota_bytes, record_audio, record_mode,
            pre_roll_seconds, post_roll_seconds, mirror_enabled, anr_enabled, anr_replay_url_template,
            enabled, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&body.site_id)
    .bind(&body.name)
    .bind(&body.vendor)
    .bind(&body.model)
    .bind(&body.address)
    .bind(rtsp_port)
    .bind(&body.username)
    .bind(&body.password)
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
    .bind(enabled)
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
    let cam = load_camera(&st.pool, &id).await?;
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

async fn update_camera(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<CameraUpdate>,
) -> AppResult<Json<CameraView>> {
    principal.require(principal.can_manage_registry(), "update cameras")?;
    let cur = load_camera(&st.pool, &id).await?;

    let record_stream = body.record_stream.unwrap_or(cur.record_stream);
    if !matches!(record_stream.as_str(), "main" | "sub") {
        return Err(AppError::BadRequest(
            "`record_stream` must be 'main' or 'sub'".into(),
        ));
    }

    let name = body.name.unwrap_or(cur.name);
    let site_id = body.site_id.or(cur.site_id);
    let vendor = body.vendor.unwrap_or(cur.vendor);
    let model = body.model.or(cur.model);
    let address = body.address.or(cur.address);
    let rtsp_port = body.rtsp_port.unwrap_or(cur.rtsp_port);
    let username = body.username.or(cur.username);
    let password = body.password.or(cur.password);
    let main_stream_url = body.main_stream_url.or(cur.main_stream_url);
    let sub_stream_url = body.sub_stream_url.or(cur.sub_stream_url);
    for url in [main_stream_url.as_deref(), sub_stream_url.as_deref()]
        .into_iter()
        .flatten()
    {
        camera_url::validate_stream_url(url).map_err(AppError::BadRequest)?;
    }
    let caps = SqlxJson(body.capabilities.unwrap_or(cur.capabilities.0));
    let record_enabled = body.record_enabled.unwrap_or(cur.record_enabled);
    let enabled = body.enabled.unwrap_or(cur.enabled);
    let seg = body
        .segment_seconds
        .map(|v| v.clamp(2, 3600))
        .unwrap_or(cur.segment_seconds);
    let retention = body
        .retention_hours
        .map(|v| v.max(1))
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
    let anr_replay_url_template = body
        .anr_replay_url_template
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or(cur.anr_replay_url_template);

    sqlx::query(
        "UPDATE cameras SET
            name=?, site_id=?, vendor=?, model=?, address=?, rtsp_port=?, username=?, password=?,
            main_stream_url=?, sub_stream_url=?, record_stream=?, capabilities=?, record_enabled=?,
            segment_seconds=?, retention_hours=?, storage_quota_bytes=?, record_audio=?, record_mode=?,
            pre_roll_seconds=?, post_roll_seconds=?, mirror_enabled=?, anr_enabled=?,
            anr_replay_url_template=?, enabled=?, updated_at=?
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
    .bind(enabled)
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
    auth::audit(
        &st.pool,
        &principal,
        "update_camera",
        "camera",
        &id,
        json!({}),
    )
    .await;
    Ok(Json(load_camera(&st.pool, &id).await?.into()))
}

async fn delete_camera(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete cameras")?;
    let _ = load_camera(&st.pool, &id).await?; // 404 if missing
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
    sqlx::query("DELETE FROM cameras WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    // Stop any AI sampler for this camera (its ai_tasks cascade-deleted) and remove its on-disk data.
    st.sampler.reconcile().await;
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
async fn test_camera(State(st): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let cam = load_camera(&st.pool, &id).await?;
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
