//! Camera device-control API: capability discovery, day/night (IR-cut), image/lighting, and
//! alarm/relay output ports.
//!
//! The dashboard's Device tab is driven entirely by `GET .../control/capabilities` (a DB read of
//! the persisted map) — it renders only surfaces the probe confirmed, so the UI stays
//! vendor-neutral. Reads are open to any authenticated principal (`can_view`); settings writes and
//! the probe are manager+ (`can_manage_registry`); a raw output pulse is manager+ too (the
//! guard-facing gate-open lives in the access-control app behind its own policy). Every mutation
//! is written to the immutable audit log.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, Cap, Principal};
use crate::error::AppResult;
use crate::services::camera_config::types::{
    DayNightConfig, DayNightPatch, ImageConfig, ImageConfigPatch, IntrusionConfig, IoOutput,
    LineCrossingConfig, MotionConfig,
};
use crate::services::camera_config::{self, CameraConfigProvider};
use crate::services::camera_control;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/cameras/{id}/control/capabilities",
            get(get_capabilities),
        )
        .route("/api/v1/cameras/{id}/control/probe", post(probe))
        .route(
            "/api/v1/cameras/{id}/control/day_night",
            get(get_day_night).put(put_day_night),
        )
        .route(
            "/api/v1/cameras/{id}/control/image",
            get(get_image).put(put_image),
        )
        .route(
            "/api/v1/cameras/{id}/control/detections/{kind}",
            axum::routing::put(put_detection),
        )
        .route(
            "/api/v1/cameras/{id}/control/line_crossing",
            get(get_line_crossing).put(put_line_crossing),
        )
        .route(
            "/api/v1/cameras/{id}/control/intrusion",
            get(get_intrusion).put(put_intrusion),
        )
        .route(
            "/api/v1/cameras/{id}/control/motion",
            get(get_motion).put(put_motion),
        )
        .route("/api/v1/cameras/{id}/control/io/outputs", get(list_outputs))
        .route(
            "/api/v1/cameras/{id}/control/io/outputs/{port}/pulse",
            post(pulse_output),
        )
}

/// Build a device-control provider for `id` (404 unknown camera; 400 not configurable).
async fn provider_for(
    st: &AppState,
    principal: &Principal,
    id: &str,
) -> AppResult<Box<dyn CameraConfigProvider>> {
    // Camera scope is asserted HERE rather than in each handler: every read and every write in
    // this file goes through this helper, so a new endpoint cannot forget the check.
    let cam = st.camera_for(principal, id).await?;
    camera_config::for_camera(&cam, &st.http, st.cfg.isapi_request_timeout_ms)
}

// ========================= Capability map =========================

/// The device-control surfaces this camera was last probed to support.
///
/// A DB read of the persisted map, never a device call — it answers instantly and answers `{}` for
/// a camera nobody has probed yet. `POST .../control/probe` is what refreshes it.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/control/capabilities", tag = "cameras",
    operation_id = "getCameraControlCapabilities",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The persisted capability map (`{}` when never probed)"),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_capabilities(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::CameraRead, "view camera device capabilities")?;
    st.camera_scope_check(&principal, &id)?;
    Ok(Json(camera_control::stored_capabilities(&st, &id).await?))
}

/// Re-probe the camera's device-control surfaces and persist the fresh capability map.
///
/// Best-effort by design: a surface that does not answer is recorded `false`, so an unreachable or
/// non-configurable camera still returns 200 with a map of falses rather than an error.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/control/probe", tag = "cameras",
    operation_id = "probeCameraControl",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The freshly probed capability map"),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn probe(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    principal.require(
        principal.can_manage_registry(),
        "probe camera device capabilities",
    )?;
    st.camera_scope_check(&principal, &id)?;
    let map = camera_control::refresh_capabilities(&st, &id).await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_control_probe",
        "camera",
        &id,
        json!({
            "day_night": map.get("day_night"),
            "image": map.get("image"),
            "native_anpr": map.get("native_anpr"),
            "ptz": map.get("ptz"),
        }),
    )
    .await;
    Ok(Json(map))
}

// ========================= Day/night (IR-cut) =========================

/// The camera's day/night (IR-cut filter) configuration, read live from the device.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/control/day_night", tag = "cameras",
    operation_id = "getCameraDayNight",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The device's current day/night configuration", body = DayNightConfig),
        (status = 400, description = "Camera is not configurable (unsupported vendor, or no address/credentials), or the device does not expose this surface", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_day_night(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<DayNightConfig>> {
    principal.require_cap(Cap::CameraRead, "view camera day/night configuration")?;
    let provider = provider_for(&st, &principal, &id).await?;
    Ok(Json(provider.get_day_night().await?))
}

/// Write the day/night (IR-cut filter) configuration to the device.
///
/// Read-modify-write: absent fields are left as the device has them. Returns the configuration
/// read back from the device afterwards, not the patch that was sent.
#[utoipa::path(
    put, path = "/api/v1/cameras/{id}/control/day_night", tag = "cameras",
    operation_id = "setCameraDayNight",
    params(("id" = String, Path, description = "Camera id")),
    request_body = DayNightPatch,
    responses(
        (status = 200, description = "The configuration read back from the device", body = DayNightConfig),
        (status = 400, description = "`mode` is not one of auto|day|night|schedule, or the camera is not configurable", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn put_day_night(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(patch): Json<DayNightPatch>,
) -> AppResult<Json<DayNightConfig>> {
    principal.require(
        principal.can_manage_registry(),
        "configure camera day/night",
    )?;
    if let Some(mode) = patch.mode.as_deref() {
        if !matches!(mode, "auto" | "day" | "night" | "schedule") {
            return Err(crate::error::AppError::BadRequest(
                "`mode` must be one of auto|day|night|schedule".into(),
            ));
        }
    }
    let provider = provider_for(&st, &principal, &id).await?;
    provider.put_day_night(&patch).await?;
    let updated = provider.get_day_night().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_control_put_day_night",
        "camera",
        &id,
        json!({ "mode": updated.mode, "sensitivity": updated.sensitivity }),
    )
    .await;
    Ok(Json(updated))
}

// ========================= Image / lighting =========================

/// The camera's image and supplement-lighting configuration, read live from the device.
///
/// Fields the device does not expose come back `null`; which lighting modes it accepts is in the
/// capability map (`supplement_light_modes`), not here.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/control/image", tag = "cameras",
    operation_id = "getCameraImage",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The device's current image/lighting configuration", body = ImageConfig),
        (status = 400, description = "Camera is not configurable (unsupported vendor, or no address/credentials), or the device does not expose this surface", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_image(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<ImageConfig>> {
    principal.require_cap(Cap::CameraRead, "view camera image configuration")?;
    let provider = provider_for(&st, &principal, &id).await?;
    Ok(Json(provider.get_image_config().await?))
}

/// Write image and supplement-lighting settings to the device.
///
/// Read-modify-write: only the fields present are written, and the response is what the device
/// reports afterwards — a value it clamped or ignored comes back as the device kept it.
#[utoipa::path(
    put, path = "/api/v1/cameras/{id}/control/image", tag = "cameras",
    operation_id = "setCameraImage",
    params(("id" = String, Path, description = "Camera id")),
    request_body = ImageConfigPatch,
    responses(
        (status = 200, description = "The configuration read back from the device", body = ImageConfig),
        (status = 400, description = "Camera is not configurable, or the device rejected a value", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn put_image(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(patch): Json<ImageConfigPatch>,
) -> AppResult<Json<ImageConfig>> {
    principal.require(
        principal.can_manage_registry(),
        "configure camera image settings",
    )?;
    let provider = provider_for(&st, &principal, &id).await?;
    provider.put_image_config(&patch).await?;
    let updated = provider.get_image_config().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_control_put_image",
        "camera",
        &id,
        json!({
            "brightness": updated.brightness,
            "contrast": updated.contrast,
            "saturation": updated.saturation,
            "wdr_mode": updated.wdr_mode,
            "blc_enabled": updated.blc_enabled,
            "supplement_light_mode": updated.supplement_light_mode,
        }),
    )
    .await;
    Ok(Json(updated))
}

// ========================= Built-in detections =========================

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DetectionUpdate {
    pub enabled: bool,
}

/// Arm/disarm one of the camera's built-in detections (motion / line_crossing / intrusion) on the
/// device, then refresh the capability map in the background so the panel's state stays truthful.
///
/// `kind` must be one the device actually exposes; anything else is a 400, not a silent no-op.
#[utoipa::path(
    put, path = "/api/v1/cameras/{id}/control/detections/{kind}", tag = "cameras",
    operation_id = "setCameraBuiltinDetection",
    params(
        ("id" = String, Path, description = "Camera id"),
        ("kind" = String, Path, description = "Detection token, e.g. `motion`, `line_crossing`, `intrusion`"),
    ),
    request_body = DetectionUpdate,
    responses(
        (status = 200, description = "The detection's new arm state"),
        (status = 400, description = "This detection cannot be armed on this device, or the camera is not configurable", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn put_detection(
    State(st): State<AppState>,
    Path((id, kind)): Path<(String, String)>,
    principal: Principal,
    Json(body): Json<DetectionUpdate>,
) -> AppResult<Json<Value>> {
    principal.require(
        principal.can_manage_registry(),
        "configure camera built-in detections",
    )?;
    let provider = provider_for(&st, &principal, &id).await?;
    provider.set_builtin_detection(&kind, body.enabled).await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_control_put_detection",
        "camera",
        &id,
        json!({ "kind": kind, "enabled": body.enabled }),
    )
    .await;
    camera_control::spawn_probe(&st, &id);
    Ok(Json(
        json!({ "ok": true, "kind": kind, "enabled": body.enabled }),
    ))
}

// ========================= Detection geometry (line / intrusion / motion) =========================

/// The camera's line-crossing rules, read live from the device.
///
/// The device exposes a FIXED set of rule slots; an unused one comes back `enabled: false` with a
/// degenerate line rather than being absent. Coordinates are normalized 0..1.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/control/line_crossing", tag = "cameras",
    operation_id = "getCameraLineCrossing",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The device's line-crossing rule slots", body = LineCrossingConfig),
        (status = 400, description = "Camera is not configurable (unsupported vendor, or no address/credentials), or the device does not expose this surface", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_line_crossing(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<LineCrossingConfig>> {
    principal.require_cap(Cap::CameraRead, "view line-crossing rules")?;
    let provider = provider_for(&st, &principal, &id).await?;
    Ok(Json(provider.get_line_crossing().await?))
}

/// Write line-crossing rules (geometry drawn in the Device panel over the camera frame).
///
/// Submitted lines are merged over the device's existing slots BY `id`, so a slot you do not send
/// is left alone. Each line needs exactly two points in 0..1 and a `direction` of
/// `any|left-right|right-left`.
#[utoipa::path(
    put, path = "/api/v1/cameras/{id}/control/line_crossing", tag = "cameras",
    operation_id = "setCameraLineCrossing",
    params(("id" = String, Path, description = "Camera id")),
    request_body = LineCrossingConfig,
    responses(
        (status = 200, description = "The rules read back from the device", body = LineCrossingConfig),
        (status = 400, description = "Bad geometry or direction, or the camera is not configurable", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn put_line_crossing(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(cfg): Json<LineCrossingConfig>,
) -> AppResult<Json<LineCrossingConfig>> {
    principal.require(
        principal.can_manage_registry(),
        "configure line-crossing rules",
    )?;
    let provider = provider_for(&st, &principal, &id).await?;
    provider.put_line_crossing(&cfg).await?;
    let updated = provider.get_line_crossing().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_control_put_line_crossing",
        "camera",
        &id,
        json!({
            "enabled": updated.enabled,
            "armed_lines": updated.lines.iter().filter(|l| l.enabled).count(),
        }),
    )
    .await;
    camera_control::spawn_probe(&st, &id);
    Ok(Json(updated))
}

/// The camera's intrusion (field-detection) regions, read live from the device.
///
/// An unconfigured region slot carries NO points — an empty `points` array means "slot unused",
/// not "region covering nothing".
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/control/intrusion", tag = "cameras",
    operation_id = "getCameraIntrusion",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The device's intrusion region slots", body = IntrusionConfig),
        (status = 400, description = "Camera is not configurable (unsupported vendor, or no address/credentials), or the device does not expose this surface", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_intrusion(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<IntrusionConfig>> {
    principal.require_cap(Cap::CameraRead, "view intrusion regions")?;
    let provider = provider_for(&st, &principal, &id).await?;
    Ok(Json(provider.get_intrusion().await?))
}

/// Write intrusion regions (geometry drawn in the Device panel over the camera frame).
///
/// Regions are merged over the device's existing slots BY `id`. A region must be a 3–10 vertex
/// polygon in 0..1, or empty to clear the slot.
#[utoipa::path(
    put, path = "/api/v1/cameras/{id}/control/intrusion", tag = "cameras",
    operation_id = "setCameraIntrusion",
    params(("id" = String, Path, description = "Camera id")),
    request_body = IntrusionConfig,
    responses(
        (status = 200, description = "The regions read back from the device", body = IntrusionConfig),
        (status = 400, description = "Bad polygon (not 3–10 points, or outside 0..1), or the camera is not configurable", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn put_intrusion(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(cfg): Json<IntrusionConfig>,
) -> AppResult<Json<IntrusionConfig>> {
    principal.require(
        principal.can_manage_registry(),
        "configure intrusion regions",
    )?;
    let provider = provider_for(&st, &principal, &id).await?;
    provider.put_intrusion(&cfg).await?;
    let updated = provider.get_intrusion().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_control_put_intrusion",
        "camera",
        &id,
        json!({
            "enabled": updated.enabled,
            "armed_regions": updated.regions.iter().filter(|r| r.enabled).count(),
        }),
    )
    .await;
    camera_control::spawn_probe(&st, &id);
    Ok(Json(updated))
}

/// The camera's on-board motion-detection arm switch and sensitivity.
///
/// The detection GRID stays on the device and is not exposed here — this is the on/off and how
/// twitchy, not where.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/control/motion", tag = "cameras",
    operation_id = "getCameraMotion",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The device's motion-detection settings", body = MotionConfig),
        (status = 400, description = "Camera is not configurable (unsupported vendor, or no address/credentials), or the device does not expose this surface", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn get_motion(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<MotionConfig>> {
    principal.require_cap(Cap::CameraRead, "view motion detection settings")?;
    let provider = provider_for(&st, &principal, &id).await?;
    Ok(Json(provider.get_motion().await?))
}

/// Write motion arm switch + sensitivity (the grid layout stays on-device).
#[utoipa::path(
    put, path = "/api/v1/cameras/{id}/control/motion", tag = "cameras",
    operation_id = "setCameraMotion",
    params(("id" = String, Path, description = "Camera id")),
    request_body = MotionConfig,
    responses(
        (status = 200, description = "The settings read back from the device", body = MotionConfig),
        (status = 400, description = "Camera is not configurable, or the device rejected the value", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn put_motion(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(cfg): Json<MotionConfig>,
) -> AppResult<Json<MotionConfig>> {
    principal.require(
        principal.can_manage_registry(),
        "configure motion detection",
    )?;
    let provider = provider_for(&st, &principal, &id).await?;
    provider.put_motion(&cfg).await?;
    let updated = provider.get_motion().await?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_control_put_motion",
        "camera",
        &id,
        json!({ "enabled": updated.enabled, "sensitivity": updated.sensitivity }),
    )
    .await;
    camera_control::spawn_probe(&st, &id);
    Ok(Json(updated))
}

// ========================= IO outputs =========================

/// The camera's alarm/relay output ports.
#[utoipa::path(
    get, path = "/api/v1/cameras/{id}/control/io/outputs", tag = "cameras",
    operation_id = "listCameraIoOutputs",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The device's output ports", body = Vec<IoOutput>),
        (status = 400, description = "Camera is not configurable (unsupported vendor, or no address/credentials), or the device has no IO surface", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `camera:read`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
        (status = 500, description = "Device unreachable or returned an unusable response", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn list_outputs(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Vec<IoOutput>>> {
    principal.require_cap(Cap::CameraRead, "view camera IO outputs")?;
    let provider = provider_for(&st, &principal, &id).await?;
    Ok(Json(provider.list_io_outputs().await?))
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct PulseRequest {
    /// Pulse width in milliseconds (0/absent = the service default of 1000; clamped to 30000).
    #[serde(default)]
    pub pulse_ms: u64,
}

/// Pulse a relay output (PHYSICAL-WORLD side effect — e.g. a barrier test fire). Manager+: the
/// guard-facing gate-open flows through the access-control app's policy, not this raw primitive.
///
/// The request body is optional. `pulse_ms` is clamped to 30000 rather than refused, and the
/// EFFECTIVE width is what comes back. A device refusal (no relay port on this model) is reported
/// as a 400 carrying the device's own reason, not as a 500.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/control/io/outputs/{port}/pulse", tag = "cameras",
    operation_id = "pulseCameraIoOutput",
    params(
        ("id" = String, Path, description = "Camera id"),
        ("port" = i64, Path, description = "Output port number (1-based)"),
    ),
    request_body(content = PulseRequest, description = "Optional pulse width"),
    responses(
        (status = 200, description = "Pulsed; reports the effective width in milliseconds"),
        (status = 400, description = "Camera is not configurable, or the device refused the pulse (e.g. no relay port)", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`, or a camera this credential does not hold", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn pulse_output(
    State(st): State<AppState>,
    Path((id, port)): Path<(String, i64)>,
    principal: Principal,
    body: Option<Json<PulseRequest>>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "pulse a camera IO output")?;
    st.camera_scope_check(&principal, &id)?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    // Map a device refusal to a 400 with the ISAPI reason — the operator needs to see e.g.
    // "Invalid Operation" (camera has no relay port), not a generic internal error.
    let held_ms = camera_control::pulse_output(&st, &id, port, req.pulse_ms)
        .await
        .map_err(|e| match e {
            crate::error::AppError::NotFound(_) | crate::error::AppError::BadRequest(_) => e,
            other => crate::error::AppError::BadRequest(format!("output pulse failed: {other}")),
        })?;
    auth::audit(
        &st.pool,
        &principal,
        "camera_control_pulse_output",
        "camera",
        &id,
        json!({ "port": port, "pulse_ms": held_ms }),
    )
    .await;
    Ok(Json(
        json!({ "ok": true, "port": port, "pulse_ms": held_ms }),
    ))
}
