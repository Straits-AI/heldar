//! ONVIF (Profile S MVP) API: network discovery, a per-camera device probe, and PTZ control.
//!
//! Discovery + probe + every PTZ command are managed by manager+ (they touch devices / change
//! state); reading a camera's stored ONVIF profile and its PTZ presets is open to any authenticated
//! principal. All mutating calls are written to the immutable audit log. Out of scope for this MVP:
//! ONVIF events, Profile G (recording/replay), Profile T, imaging, and absolute/relative PTZ moves.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, Cap, Principal};
use crate::error::AppResult;
use crate::models::{CameraOnvif, PtzPreset};
use crate::services::onvif;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/onvif/discover", post(discover))
        .route("/api/v1/cameras/{id}/onvif", get(get_onvif))
        .route("/api/v1/cameras/{id}/onvif/probe", post(probe))
        .route("/api/v1/cameras/{id}/ptz/presets", get(list_presets))
        .route(
            "/api/v1/cameras/{id}/ptz/presets/refresh",
            post(refresh_presets),
        )
        .route("/api/v1/cameras/{id}/ptz/continuous", post(continuous_move))
        .route("/api/v1/cameras/{id}/ptz/stop", post(ptz_stop))
        .route("/api/v1/cameras/{id}/ptz/goto_preset", post(goto_preset))
}

// ---- Discovery ----

/// Sweep the local network segment for ONVIF devices (WS-Discovery).
///
/// Refused outright to a camera-scoped credential: the sweep answers "what devices are on this
/// segment", which for such a credential is a list of cameras it does not hold, and there is no
/// camera id to scope the answer by.
#[utoipa::path(
    post, path = "/api/v1/onvif/discover", tag = "cameras",
    operation_id = "discoverOnvifDevices",
    responses(
        (status = 200, description = "Devices that answered within the discovery window"),
        (status = 403, description = "Missing `registry:manage`, or a camera-scoped credential", body = crate::openapi::ErrorBody),
        (status = 500, description = "The discovery socket could not be opened or the probe could not be sent", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn discover(State(st): State<AppState>, principal: Principal) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "run ONVIF discovery")?;
    // Box-level: a WS-Discovery sweep answers "what devices are on this segment", which for a
    // camera-scoped credential is a list of cameras it does not hold. There is no camera id to scope
    // it by, so containment can only be a refusal. See `cameras::require_fleet_scope`.
    crate::routes::cameras::require_fleet_scope(&principal, "run ONVIF discovery")?;
    let devices = onvif::discover(&st.cfg).await?;
    auth::audit(
        &st.pool,
        &principal,
        "onvif_discover",
        "onvif",
        "discovery",
        json!({ "found": devices.len() }),
    )
    .await;
    Ok(Json(json!({
        "found": devices.len(),
        "devices": devices,
    })))
}

// ---- Per-camera device profile ----

/// The camera's stored ONVIF device profile, as recorded by the last probe.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/onvif", tag = "cameras",
    operation_id = "getCameraOnvif",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The stored ONVIF profile"),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera, or one never probed", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_onvif(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<CameraOnvif>> {
    principal.require_cap(Cap::CameraRead, "view ONVIF profile")?;
    let _ = st.camera_for(&principal, &id).await?;
    Ok(Json(onvif::load_onvif(&st.pool, &id).await?))
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct ProbeRequest {
    /// Optional explicit ONVIF device service URL (e.g. `http://host/onvif/device_service`). When
    /// omitted, the URL is taken from a prior probe or derived from the camera's address.
    pub device_url: Option<String>,
}

/// Probe the camera's ONVIF interface and persist what it reports.
///
/// An explicit `device_url` is checked against the LAN egress guard before it is called, so a URL
/// pointing at the cloud-metadata or link-local ranges is a 400 rather than a request.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/onvif/probe", tag = "cameras",
    operation_id = "probeCameraOnvif",
    params(("id" = String, Path, description = "Camera id")),
    request_body = Option<ProbeRequest>,
    responses(
        (status = 200, description = "The probed ONVIF profile, now stored"),
        (status = 400, description = "A `device_url` the egress guard rejects, or no `device_url` and no camera address to derive one from", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "The device did not answer, or answered with an ONVIF fault", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn probe(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    body: Option<Json<ProbeRequest>>,
) -> AppResult<Json<CameraOnvif>> {
    principal.require(principal.can_manage_registry(), "probe ONVIF devices")?;
    let _ = st.camera_for(&principal, &id).await?;
    let device_url = body.and_then(|Json(b)| b.device_url);
    let onvif = onvif::probe(&st, &id, device_url).await?;
    auth::audit(
        &st.pool,
        &principal,
        "onvif_probe",
        "camera",
        &id,
        json!({
            "manufacturer": onvif.manufacturer,
            "model": onvif.model,
            "ptz_enabled": onvif.ptz_enabled,
        }),
    )
    .await;
    Ok(Json(onvif))
}

// ---- PTZ presets ----

/// The camera's stored PTZ presets, ordered by device token.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/ptz/presets", tag = "cameras",
    operation_id = "listPtzPresets",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "Stored presets, by device token"),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_presets(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Vec<PtzPreset>>> {
    principal.require_cap(Cap::CameraRead, "view PTZ presets")?;
    let _ = st.camera_for(&principal, &id).await?;
    let rows = sqlx::query_as::<_, PtzPreset>(
        "SELECT * FROM camera_ptz_presets WHERE camera_id = ? ORDER BY token ASC",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

/// Re-fetch the PTZ presets from the camera and replace the stored set.
///
/// Presets the device no longer reports are deleted, so this is a replace and not a merge.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/ptz/presets/refresh", tag = "cameras",
    operation_id = "refreshPtzPresets",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The presets the device now reports"),
        (status = 400, description = "The camera exposes no ONVIF PTZ service or has no media profile token", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera, or one never probed", body = crate::openapi::ErrorBody),
        (status = 500, description = "The device did not answer, or answered with an ONVIF fault", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn refresh_presets(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Vec<PtzPreset>>> {
    principal.require(principal.can_manage_registry(), "refresh PTZ presets")?;
    let _ = st.camera_for(&principal, &id).await?;
    let presets = onvif::get_presets(&st, &id).await?;
    auth::audit(
        &st.pool,
        &principal,
        "ptz_refresh_presets",
        "camera",
        &id,
        json!({ "count": presets.len() }),
    )
    .await;
    Ok(Json(presets))
}

// ---- PTZ movement ----

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContinuousMoveRequest {
    /// Pan velocity, clamped to -1.0..=1.0.
    #[serde(default)]
    pub pan: f64,
    /// Tilt velocity, clamped to -1.0..=1.0.
    #[serde(default)]
    pub tilt: f64,
    /// Zoom velocity, clamped to -1.0..=1.0.
    #[serde(default)]
    pub zoom: f64,
}

/// Start a continuous pan/tilt/zoom at the given normalized velocities.
///
/// The motion runs until `/ptz/stop` or the device's own timeout — a 200 here means the camera is
/// still moving, not that a move finished.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/ptz/continuous", tag = "cameras",
    operation_id = "ptzContinuousMove",
    params(("id" = String, Path, description = "Camera id")),
    request_body = ContinuousMoveRequest,
    responses(
        (status = 200, description = "The move was accepted by the device"),
        (status = 400, description = "The camera exposes no ONVIF PTZ service or has no media profile token", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera, or one never probed", body = crate::openapi::ErrorBody),
        (status = 500, description = "The device did not answer, or answered with an ONVIF fault", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn continuous_move(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<ContinuousMoveRequest>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "control PTZ")?;
    let _ = st.camera_for(&principal, &id).await?;
    onvif::continuous_move(&st, &id, body.pan, body.tilt, body.zoom).await?;
    auth::audit(
        &st.pool,
        &principal,
        "ptz_continuous_move",
        "camera",
        &id,
        json!({ "pan": body.pan, "tilt": body.tilt, "zoom": body.zoom }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// Stop any PTZ motion on the camera.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/ptz/stop", tag = "cameras",
    operation_id = "ptzStop",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The stop was accepted by the device"),
        (status = 400, description = "The camera exposes no ONVIF PTZ service or has no media profile token", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera, or one never probed", body = crate::openapi::ErrorBody),
        (status = 500, description = "The device did not answer, or answered with an ONVIF fault", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn ptz_stop(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "control PTZ")?;
    let _ = st.camera_for(&principal, &id).await?;
    onvif::stop(&st, &id).await?;
    auth::audit(&st.pool, &principal, "ptz_stop", "camera", &id, json!({})).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct GotoPresetRequest {
    /// The preset's DEVICE token, as reported by `/ptz/presets` — not the stored row id.
    pub token: String,
}

/// Move the camera to a stored PTZ preset.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/ptz/goto_preset", tag = "cameras",
    operation_id = "ptzGotoPreset",
    params(("id" = String, Path, description = "Camera id")),
    request_body = GotoPresetRequest,
    responses(
        (status = 200, description = "The move was accepted by the device"),
        (status = 400, description = "Empty `token`, or the camera exposes no ONVIF PTZ service or media profile token", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera, or one never probed", body = crate::openapi::ErrorBody),
        (status = 500, description = "The device did not answer, or answered with an ONVIF fault", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn goto_preset(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<GotoPresetRequest>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "control PTZ")?;
    let _ = st.camera_for(&principal, &id).await?;
    onvif::goto_preset(&st, &id, &body.token).await?;
    auth::audit(
        &st.pool,
        &principal,
        "ptz_goto_preset",
        "camera",
        &id,
        json!({ "token": body.token }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Scope;
    use crate::error::AppError;
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
            started_at: chrono::Utc::now(),
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

    /// WS-Discovery sweeps the whole segment and answers with the devices on it — for a
    /// camera-scoped credential, a list of cameras it does not hold. It takes no camera id, so there
    /// is nothing to scope it by and the refusal happens before the probe leaves the box.
    #[tokio::test]
    async fn onvif_discovery_is_refused_to_a_camera_scoped_credential() {
        let st = test_state().await;
        let err = discover(State(st), scoped(&["cam_a"])).await.unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)), "{err:?}");
    }
}
