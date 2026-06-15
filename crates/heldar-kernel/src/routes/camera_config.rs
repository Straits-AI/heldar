//! Camera configuration (HikVision ISAPI) API: device identity, video-encoding, clock/NTP, ONVIF +
//! ISAPI integration, on-screen-display overlays, reboot, and a cross-camera bulk apply.
//!
//! Reads (GET) are open to any authenticated principal (`can_view`); every mutation (PUT/POST) is
//! gated by manager+ (`can_manage_registry`) and written to the immutable audit log. The handlers
//! own persistence/audit; the device protocol lives behind the vendor-agnostic
//! [`CameraConfigProvider`] built per-camera by [`camera_config::for_camera`]. `GET .../device_info`
//! and `POST .../onvif/ensure_user` also refresh the `camera_isapi` cache row.
//!
//! The reboot endpoint is DISRUPTIVE: it refuses to act unless the body carries `confirm: true`. The
//! bulk endpoint walks its target cameras SERIALLY, bounding each camera's action with a timeout and
//! collecting a per-camera result so one unreachable device never aborts the run.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::routes::cameras::load_camera;
use crate::services::camera_config::types::{
    BulkAction, BulkCameraResult, BulkConfigRequest, BulkConfigResponse, DeviceInfo,
    EnsureOnvifUserRequest, NtpConfig, OnvifSettings, OnvifUserType, OsdConfig, RebootRequest,
    TimeConfig, VideoConfig, VideoConfigPatch,
};
use crate::services::camera_config::{self, CameraConfigProvider};
use crate::state::AppState;

/// HikVision main streaming channel; the bulk `SetVideo` action defaults to it when no channel given.
const MAIN_CHANNEL: i64 = 101;
/// Least-privilege ONVIF role provisioned by default (media + PTZ for Profile S; not device config).
const DEFAULT_ONVIF_USER_TYPE: OnvifUserType = OnvifUserType::Operator;
/// A bulk per-camera action runs several ISAPI requests (e.g. EnableOnvif = GET+PUT Integrate then
/// GET+POST users); bound the whole action at this multiple of the single-request timeout so one
/// stuck camera cannot stall the serial loop.
const BULK_REQUEST_FACTOR: u64 = 6;

pub fn router() -> Router<AppState> {
    Router::new()
        // Literal sibling of the `/{id}/config/*` param routes; axum/matchit resolves the static
        // `config/bulk` ahead of capturing `id = "config"`, so it reaches the bulk handler.
        .route("/api/v1/cameras/config/bulk", post(bulk_config))
        .route(
            "/api/v1/cameras/{id}/config/device_info",
            get(get_device_info),
        )
        .route("/api/v1/cameras/{id}/config/video", get(get_video_list))
        .route(
            "/api/v1/cameras/{id}/config/video/{channel}",
            get(get_video).put(put_video),
        )
        .route(
            "/api/v1/cameras/{id}/config/time",
            get(get_time).put(put_time),
        )
        .route(
            "/api/v1/cameras/{id}/config/time/ntp",
            get(get_ntp).put(put_ntp),
        )
        .route("/api/v1/cameras/{id}/config/time/sync_now", post(sync_now))
        .route(
            "/api/v1/cameras/{id}/config/onvif",
            get(get_onvif_settings).put(put_onvif_settings),
        )
        .route(
            "/api/v1/cameras/{id}/config/onvif/ensure_user",
            post(ensure_onvif_user),
        )
        .route("/api/v1/cameras/{id}/config/osd", get(get_osd).put(put_osd))
        .route("/api/v1/cameras/{id}/config/reboot", post(reboot))
}

/// Build a camera-config provider for `id`, 404ing when the camera is unknown and 400ing when it is
/// not configurable (no address/credentials, or an unsupported vendor).
async fn provider_for(st: &AppState, id: &str) -> AppResult<Box<dyn CameraConfigProvider>> {
    let cam = load_camera(&st.pool, id).await?;
    camera_config::for_camera(&cam, &st.http, st.cfg.isapi_request_timeout_ms)
}

// ========================= Device identity =========================

async fn get_device_info(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<DeviceInfo>> {
    principal.require(principal.can_view(), "view camera configuration")?;
    let provider = provider_for(&st, &id).await?;
    let info = provider.get_device_info().await?;
    // Refresh the per-camera ISAPI cache (identity columns only; integration/clock state preserved).
    sqlx::query(
        "INSERT INTO camera_isapi (camera_id, device_name, model, firmware_version, serial_number, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            device_name = excluded.device_name,
            model = excluded.model,
            firmware_version = excluded.firmware_version,
            serial_number = excluded.serial_number,
            fetched_at = excluded.fetched_at",
    )
    .bind(&id)
    .bind(&info.device_name)
    .bind(&info.model)
    .bind(&info.firmware_version)
    .bind(&info.serial_number)
    .bind(Utc::now())
    .execute(&st.pool)
    .await?;
    Ok(Json(info))
}

// ========================= Video encoding =========================

async fn get_video_list(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Vec<VideoConfig>>> {
    principal.require(principal.can_view(), "view camera video configuration")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.list_video_configs().await?))
}

async fn get_video(
    State(st): State<AppState>,
    Path((id, channel)): Path<(String, u32)>,
    principal: Principal,
) -> AppResult<Json<VideoConfig>> {
    principal.require(principal.can_view(), "view camera video configuration")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.get_video_config(channel).await?))
}

async fn put_video(
    State(st): State<AppState>,
    Path((id, channel)): Path<(String, u32)>,
    principal: Principal,
    Json(patch): Json<VideoConfigPatch>,
) -> AppResult<Json<VideoConfig>> {
    principal.require(principal.can_manage_registry(), "configure camera video")?;
    let provider = provider_for(&st, &id).await?;
    let merged = merge_video(provider.get_video_config(channel).await?, &patch);
    provider.put_video_config(channel, &merged).await?;
    let updated = provider.get_video_config(channel).await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_put_video",
        "camera",
        &id,
        json!({
            "channel": channel,
            "codec": updated.codec,
            "width": updated.width,
            "height": updated.height,
            "fps": updated.fps,
        }),
    )
    .await;
    Ok(Json(updated))
}

// ========================= Clock / NTP =========================

async fn get_time(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<TimeConfig>> {
    principal.require(principal.can_view(), "view camera clock")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.get_time_config().await?))
}

async fn put_time(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(cfg): Json<TimeConfig>,
) -> AppResult<Json<TimeConfig>> {
    principal.require(principal.can_manage_registry(), "configure camera clock")?;
    let provider = provider_for(&st, &id).await?;
    provider.put_time_config(&cfg).await?;
    let updated = provider.get_time_config().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_put_time",
        "camera",
        &id,
        json!({ "time_mode": updated.time_mode, "time_zone": updated.time_zone }),
    )
    .await;
    Ok(Json(updated))
}

async fn get_ntp(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<NtpConfig>> {
    principal.require(principal.can_view(), "view camera NTP server")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.get_ntp_config().await?))
}

async fn put_ntp(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(cfg): Json<NtpConfig>,
) -> AppResult<Json<NtpConfig>> {
    principal.require(
        principal.can_manage_registry(),
        "configure camera NTP server",
    )?;
    let provider = provider_for(&st, &id).await?;
    provider.put_ntp_config(&cfg).await?;
    let updated = provider.get_ntp_config().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_put_ntp",
        "camera",
        &id,
        json!({ "host_name": updated.host_name, "addressing_format": updated.addressing_format }),
    )
    .await;
    Ok(Json(updated))
}

async fn sync_now(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<TimeConfig>> {
    principal.require(principal.can_manage_registry(), "sync camera clock")?;
    let provider = provider_for(&st, &id).await?;
    let updated = provider.sync_time_now().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_sync_time",
        "camera",
        &id,
        json!({ "time_mode": updated.time_mode }),
    )
    .await;
    Ok(Json(updated))
}

// ========================= ONVIF / ISAPI integration =========================

async fn get_onvif_settings(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<OnvifSettings>> {
    principal.require(principal.can_view(), "view camera ONVIF settings")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.get_onvif_settings().await?))
}

async fn put_onvif_settings(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(cfg): Json<OnvifSettings>,
) -> AppResult<Json<OnvifSettings>> {
    principal.require(
        principal.can_manage_registry(),
        "configure camera ONVIF settings",
    )?;
    let provider = provider_for(&st, &id).await?;
    provider.put_onvif_settings(&cfg).await?;
    let updated = provider.get_onvif_settings().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_put_onvif",
        "camera",
        &id,
        json!({ "onvif_enabled": updated.onvif_enabled, "isapi_enabled": updated.isapi_enabled }),
    )
    .await;
    Ok(Json(updated))
}

async fn ensure_onvif_user(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<EnsureOnvifUserRequest>,
) -> AppResult<Json<Value>> {
    principal.require(
        principal.can_manage_registry(),
        "provision a camera ONVIF user",
    )?;
    let provider = provider_for(&st, &id).await?;
    // The provider treats a duplicate create as success and does not report created-vs-existed, so
    // the cache flag is the kernel's record of whether it has already provisioned this user.
    let already: bool = sqlx::query_scalar::<_, i64>(
        "SELECT onvif_user_created FROM camera_isapi WHERE camera_id = ?",
    )
    .bind(&id)
    .fetch_optional(&st.pool)
    .await?
    .map(|v| v != 0)
    .unwrap_or(false);
    let user_type = body.user_type.unwrap_or(DEFAULT_ONVIF_USER_TYPE);
    provider
        .ensure_onvif_user(&body.username, &body.password, user_type)
        .await?;
    sqlx::query(
        "INSERT INTO camera_isapi (camera_id, onvif_user_created, fetched_at)
         VALUES (?, 1, ?)
         ON CONFLICT(camera_id) DO UPDATE SET onvif_user_created = 1, fetched_at = excluded.fetched_at",
    )
    .bind(&id)
    .bind(Utc::now())
    .execute(&st.pool)
    .await?;
    let created = !already;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_ensure_onvif_user",
        "camera",
        &id,
        json!({ "username": body.username, "created": created }),
    )
    .await;
    Ok(Json(json!({ "ok": true, "created": created })))
}

// ========================= On-screen-display overlays =========================

async fn get_osd(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<OsdConfig>> {
    principal.require(principal.can_view(), "view camera OSD overlays")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.get_osd_config().await?))
}

async fn put_osd(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(cfg): Json<OsdConfig>,
) -> AppResult<Json<OsdConfig>> {
    principal.require(
        principal.can_manage_registry(),
        "configure camera OSD overlays",
    )?;
    let provider = provider_for(&st, &id).await?;
    provider.put_osd_config(&cfg).await?;
    let updated = provider.get_osd_config().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_put_osd",
        "camera",
        &id,
        json!({
            "datetime_enabled": updated.datetime_enabled,
            "channel_name_enabled": updated.channel_name_enabled,
        }),
    )
    .await;
    Ok(Json(updated))
}

// ========================= Reboot (DISRUPTIVE) =========================

async fn reboot(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<RebootRequest>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "reboot a camera")?;
    let cam = load_camera(&st.pool, &id).await?; // 404 if missing
    if !body.confirm {
        tracing::warn!(camera_id = %id, "camera reboot rejected: `confirm` was not true");
        return Err(AppError::BadRequest(
            "rebooting a camera is disruptive; resend with `confirm: true`".into(),
        ));
    }
    let provider = camera_config::for_camera(&cam, &st.http, st.cfg.isapi_request_timeout_ms)?;
    provider.reboot().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_reboot",
        "camera",
        &id,
        json!({ "confirm": true }),
    )
    .await;
    Ok(Json(json!({ "ok": true, "rebooting": true })))
}

// ========================= Bulk apply =========================

async fn bulk_config(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<BulkConfigRequest>,
) -> AppResult<Json<BulkConfigResponse>> {
    principal.require(
        principal.can_manage_registry(),
        "run a bulk camera configuration",
    )?;

    // Resolve the target set: an explicit list, or every enabled camera.
    let ids: Vec<String> = match &body.camera_ids {
        Some(list) => list.clone(),
        None => {
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM cameras WHERE enabled = 1 ORDER BY id ASC",
            )
            .fetch_all(&st.pool)
            .await?
        }
    };

    let per_camera = Duration::from_millis(
        st.cfg
            .isapi_request_timeout_ms
            .saturating_mul(BULK_REQUEST_FACTOR)
            .max(2000),
    );

    // SERIAL loop: one slow/unreachable camera is bounded by `per_camera` and never aborts the run.
    let mut results: Vec<BulkCameraResult> = Vec::with_capacity(ids.len());
    for cam_id in &ids {
        let outcome = run_bulk_for_camera(&st, cam_id, &body.action, per_camera).await;
        results.push(match outcome {
            Ok(()) => BulkCameraResult {
                camera_id: cam_id.clone(),
                ok: true,
                error: None,
            },
            Err(e) => BulkCameraResult {
                camera_id: cam_id.clone(),
                ok: false,
                error: Some(e.to_string()),
            },
        });
    }

    let succeeded = results.iter().filter(|r| r.ok).count();
    let failed = results.len() - succeeded;
    auth::audit(
        &st.pool,
        &principal,
        "camera_config_bulk",
        "camera",
        "*",
        json!({
            "action": action_name(&body.action),
            "targets": ids.len(),
            "succeeded": succeeded,
            "failed": failed,
        }),
    )
    .await;
    Ok(Json(BulkConfigResponse {
        results,
        succeeded,
        failed,
    }))
}

/// Build a provider for one camera and run the bulk action, bounded by `per_camera`.
async fn run_bulk_for_camera(
    st: &AppState,
    cam_id: &str,
    action: &BulkAction,
    per_camera: Duration,
) -> AppResult<()> {
    let provider = provider_for(st, cam_id).await?;
    match tokio::time::timeout(per_camera, apply_bulk_action(provider.as_ref(), action)).await {
        Ok(res) => res,
        Err(_) => Err(AppError::Other(anyhow::anyhow!(
            "camera configuration action timed out"
        ))),
    }
}

/// Apply a single [`BulkAction`] against a live provider.
async fn apply_bulk_action(
    provider: &dyn CameraConfigProvider,
    action: &BulkAction,
) -> AppResult<()> {
    match action {
        BulkAction::EnableOnvif {
            onvif_username,
            onvif_password,
        } => {
            provider
                .put_onvif_settings(&OnvifSettings {
                    onvif_enabled: true,
                    isapi_enabled: true,
                })
                .await?;
            provider
                .ensure_onvif_user(onvif_username, onvif_password, DEFAULT_ONVIF_USER_TYPE)
                .await?;
        }
        BulkAction::SyncTime { ntp_server } => {
            if let Some(server) = ntp_server {
                provider.put_ntp_config(&ntp_config_for(server)).await?;
            }
            provider.sync_time_now().await?;
        }
        BulkAction::SetNtp { ntp_server } => {
            provider.put_ntp_config(&ntp_config_for(ntp_server)).await?;
        }
        BulkAction::SetVideo { channel, patch } => {
            let ch = channel.unwrap_or(MAIN_CHANNEL) as u32;
            let merged = merge_video(provider.get_video_config(ch).await?, patch);
            provider.put_video_config(ch, &merged).await?;
        }
    }
    Ok(())
}

// ========================= helpers =========================

/// Overlay a [`VideoConfigPatch`]'s set fields onto a full [`VideoConfig`] (read-modify-write).
fn merge_video(mut cfg: VideoConfig, patch: &VideoConfigPatch) -> VideoConfig {
    if let Some(v) = &patch.codec {
        cfg.codec = v.clone();
    }
    if let Some(v) = patch.width {
        cfg.width = v;
    }
    if let Some(v) = patch.height {
        cfg.height = v;
    }
    if let Some(v) = patch.fps {
        cfg.fps = v;
    }
    if let Some(v) = &patch.quality_control {
        cfg.quality_control = v.clone();
    }
    if let Some(v) = patch.bitrate {
        cfg.bitrate = v;
    }
    if let Some(v) = patch.vbr_upper_cap {
        cfg.vbr_upper_cap = v;
    }
    if let Some(v) = patch.gop {
        cfg.gop = v;
    }
    cfg
}

/// Build an [`NtpConfig`] from a bare server string, inferring `ipaddress` vs `hostname`.
fn ntp_config_for(server: &str) -> NtpConfig {
    let addressing_format = if server.parse::<std::net::IpAddr>().is_ok() {
        "ipaddress"
    } else {
        "hostname"
    };
    NtpConfig {
        addressing_format: addressing_format.to_string(),
        host_name: server.to_string(),
        port: 123,
    }
}

/// Stable label for a bulk action (audit detail).
fn action_name(action: &BulkAction) -> &'static str {
    match action {
        BulkAction::EnableOnvif { .. } => "enable_onvif",
        BulkAction::SyncTime { .. } => "sync_time",
        BulkAction::SetNtp { .. } => "set_ntp",
        BulkAction::SetVideo { .. } => "set_video",
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Path;
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use tower::Service;

    async fn send(app: &mut Router, method: &str, uri: &str) -> (StatusCode, String) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = app.call(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The literal `cameras/config/bulk` route is a sibling of the `cameras/{id}/config/*` param
    /// routes; axum/matchit must resolve the static segment ahead of capturing `id = "config"`, so a
    /// POST reaches the bulk handler. Also assert the real kernel router (camera_config merged before
    /// cameras) builds without a route conflict.
    #[tokio::test]
    async fn bulk_route_beats_id_param() {
        // The full kernel router merges camera_config before cameras; constructing it would panic on
        // any conflicting-route overlap.
        let _ = crate::routes::api_router();

        let mut app: Router = Router::new()
            .route("/api/v1/cameras/config/bulk", post(|| async { "bulk" }))
            .route(
                "/api/v1/cameras/{id}/config/device_info",
                get(|Path(id): Path<String>| async move { format!("device_info:{id}") }),
            );

        // The static bulk path wins over the {id} param.
        let (status, body) = send(&mut app, "POST", "/api/v1/cameras/config/bulk").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "bulk");

        // A genuine id still routes to the per-camera param handler.
        let (status, body) =
            send(&mut app, "GET", "/api/v1/cameras/cam-1/config/device_info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "device_info:cam-1");
    }
}
