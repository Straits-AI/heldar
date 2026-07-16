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

use crate::auth::{self, Principal};
use crate::error::AppResult;
use crate::routes::cameras::load_camera;
use crate::services::camera_config::types::{
    DayNightConfig, DayNightPatch, ImageConfig, ImageConfigPatch, IoOutput,
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
        .route("/api/v1/cameras/{id}/control/io/outputs", get(list_outputs))
        .route(
            "/api/v1/cameras/{id}/control/io/outputs/{port}/pulse",
            post(pulse_output),
        )
}

/// Build a device-control provider for `id` (404 unknown camera; 400 not configurable).
async fn provider_for(st: &AppState, id: &str) -> AppResult<Box<dyn CameraConfigProvider>> {
    let cam = load_camera(&st.pool, id).await?;
    camera_config::for_camera(&cam, &st.http, st.cfg.isapi_request_timeout_ms)
}

// ========================= Capability map =========================

async fn get_capabilities(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_view(), "view camera device capabilities")?;
    Ok(Json(camera_control::stored_capabilities(&st, &id).await?))
}

async fn probe(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    principal.require(
        principal.can_manage_registry(),
        "probe camera device capabilities",
    )?;
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

async fn get_day_night(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<DayNightConfig>> {
    principal.require(principal.can_view(), "view camera day/night configuration")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.get_day_night().await?))
}

async fn put_day_night(
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
    let provider = provider_for(&st, &id).await?;
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

async fn get_image(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<ImageConfig>> {
    principal.require(principal.can_view(), "view camera image configuration")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.get_image_config().await?))
}

async fn put_image(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(patch): Json<ImageConfigPatch>,
) -> AppResult<Json<ImageConfig>> {
    principal.require(
        principal.can_manage_registry(),
        "configure camera image settings",
    )?;
    let provider = provider_for(&st, &id).await?;
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

// ========================= IO outputs =========================

async fn list_outputs(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Vec<IoOutput>>> {
    principal.require(principal.can_view(), "view camera IO outputs")?;
    let provider = provider_for(&st, &id).await?;
    Ok(Json(provider.list_io_outputs().await?))
}

#[derive(Debug, Default, Deserialize)]
struct PulseRequest {
    /// Pulse width in milliseconds (0/absent = the service default; bounded by MAX_PULSE_MS).
    #[serde(default)]
    pulse_ms: u64,
}

/// Pulse a relay output (PHYSICAL-WORLD side effect — e.g. a barrier test fire). Manager+: the
/// guard-facing gate-open flows through the access-control app's policy, not this raw primitive.
async fn pulse_output(
    State(st): State<AppState>,
    Path((id, port)): Path<(String, i64)>,
    principal: Principal,
    body: Option<Json<PulseRequest>>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_manage_registry(), "pulse a camera IO output")?;
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
