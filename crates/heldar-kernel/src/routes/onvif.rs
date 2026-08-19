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

async fn discover(State(st): State<AppState>, principal: Principal) -> AppResult<Json<Value>> {
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

async fn get_onvif(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<CameraOnvif>> {
    principal.require_cap(Cap::CameraRead, "view ONVIF profile")?;
    let _ = st.camera_for(&principal, &id).await?;
    Ok(Json(onvif::load_onvif(&st.pool, &id).await?))
}

#[derive(Debug, Default, Deserialize)]
struct ProbeRequest {
    /// Optional explicit ONVIF device service URL (e.g. `http://host/onvif/device_service`). When
    /// omitted, the URL is taken from a prior probe or derived from the camera's address.
    device_url: Option<String>,
}

async fn probe(
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

async fn list_presets(
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

async fn refresh_presets(
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

#[derive(Debug, Deserialize)]
struct ContinuousMoveRequest {
    #[serde(default)]
    pan: f64,
    #[serde(default)]
    tilt: f64,
    #[serde(default)]
    zoom: f64,
}

async fn continuous_move(
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

async fn ptz_stop(
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

#[derive(Debug, Deserialize)]
struct GotoPresetRequest {
    token: String,
}

async fn goto_preset(
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
