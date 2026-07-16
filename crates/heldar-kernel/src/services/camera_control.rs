//! Camera device-control service: capability discovery + the relay-output pulse primitive.
//!
//! The dashboard renders per-camera device controls (day/night, image/lighting, IO outputs,
//! on-board ANPR, PTZ) strictly from the normalized capability map this service persists under the
//! `device_control` key of `cameras.capabilities` — capability-driven, never vendor-hardcoded. The
//! probe is best-effort per surface: an endpoint that errors simply leaves its capability absent,
//! and a probe failure never breaks camera add or streaming (callers treat it as advisory).
//!
//! [`pulse_output`] is the physical-world actuation primitive (barrier/boom relays wired to a
//! camera's alarm output): set the port high, hold, set it low — the release is always attempted,
//! even when the hold was interrupted, so a failed pulse cannot leave a gate relay latched open.

use std::time::Duration;

use chrono::Utc;
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;

use crate::error::{AppError, AppResult};
use crate::models::Camera;
use crate::services::camera_config::{self, CameraConfigProvider};
use crate::state::AppState;

/// Longest relay pulse we will hold (safety bound — a barrier pulse is O(seconds)).
pub const MAX_PULSE_MS: u64 = 30_000;
/// Default relay pulse width when a caller passes 0.
pub const DEFAULT_PULSE_MS: u64 = 1_000;

/// Load a camera row (404 when unknown).
async fn load_camera(st: &AppState, camera_id: &str) -> AppResult<Camera> {
    sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("camera {camera_id} not found")))
}

/// Probe every device-control surface of a camera and persist the normalized capability map under
/// `capabilities.device_control` (other capability keys are preserved). Returns the fresh map.
///
/// Best-effort by design: each surface that answers is recorded `true` (with detail where useful);
/// one that errors is recorded `false`. A camera whose vendor has no config provider at all (or
/// that lacks address/credentials) still gets a map — with only the ONVIF-derived PTZ flag set.
pub async fn refresh_capabilities(st: &AppState, camera_id: &str) -> AppResult<Value> {
    let cam = load_camera(st, camera_id).await?;

    let mut map = json!({
        "vendor": cam.vendor,
        "day_night": false,
        "image": false,
        "io_outputs": [],
        "native_anpr": false,
        "ptz": false,
        "probed_at": Utc::now().to_rfc3339(),
    });

    // PTZ comes from the persisted ONVIF probe (vendor-neutral), not from ISAPI.
    if let Ok(Some(ptz)) =
        sqlx::query_scalar::<_, bool>("SELECT ptz_enabled FROM camera_onvif WHERE camera_id = ?")
            .bind(camera_id)
            .fetch_optional(&st.pool)
            .await
    {
        map["ptz"] = json!(ptz);
    }

    // Vendor device-control probes (ISAPI today). No provider => the ONVIF-only map above stands.
    if let Ok(provider) = camera_config::for_camera(&cam, &st.http, st.cfg.isapi_request_timeout_ms)
    {
        probe_surfaces(provider.as_ref(), &mut map).await;
    }

    persist_device_control(st, camera_id, &map).await?;
    Ok(map)
}

/// Run the per-surface probes against a live provider, recording what answered.
async fn probe_surfaces(provider: &dyn CameraConfigProvider, map: &mut Value) {
    if provider.get_day_night().await.is_ok() {
        map["day_night"] = json!(true);
    }
    if provider.get_image_config().await.is_ok() {
        map["image"] = json!(true);
    }
    if let Ok(outputs) = provider.list_io_outputs().await {
        map["io_outputs"] = json!(outputs);
    }
    if provider.supports_native_anpr().await {
        map["native_anpr"] = json!(true);
    }
}

/// Merge the device-control map into `cameras.capabilities` (preserving unrelated keys).
async fn persist_device_control(st: &AppState, camera_id: &str, map: &Value) -> AppResult<()> {
    let cam = load_camera(st, camera_id).await?;
    let mut caps = cam.capabilities.0;
    if !caps.is_object() {
        caps = json!({});
    }
    caps["device_control"] = map.clone();
    sqlx::query("UPDATE cameras SET capabilities = ?, updated_at = ? WHERE id = ?")
        .bind(SqlxJson(caps))
        .bind(Utc::now())
        .bind(camera_id)
        .execute(&st.pool)
        .await?;
    Ok(())
}

/// The persisted capability map for a camera (`{}` when never probed) — a DB read, no device call.
pub async fn stored_capabilities(st: &AppState, camera_id: &str) -> AppResult<Value> {
    let cam = load_camera(st, camera_id).await?;
    Ok(cam
        .capabilities
        .0
        .get("device_control")
        .cloned()
        .unwrap_or_else(|| json!({})))
}

/// Pulse a camera's alarm/relay output: set `port` high, hold `pulse_ms` (bounded by
/// [`MAX_PULSE_MS`]), then set it low. The release is ALWAYS attempted — even when the raise
/// succeeded and the hold elapsed abnormally — so a gate relay is never left latched. Returns the
/// effective pulse width.
pub async fn pulse_output(
    st: &AppState,
    camera_id: &str,
    port: i64,
    pulse_ms: u64,
) -> AppResult<u64> {
    pulse_output_with(&st.pool, &st.http, &st.cfg, camera_id, port, pulse_ms).await
}

/// [`pulse_output`] for callers that hold the parts rather than an [`AppState`] (e.g. an app's
/// detection consumer, which is constructed before the state exists).
pub async fn pulse_output_with(
    pool: &sqlx::SqlitePool,
    http: &reqwest::Client,
    cfg: &crate::config::Config,
    camera_id: &str,
    port: i64,
    pulse_ms: u64,
) -> AppResult<u64> {
    let cam = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("camera {camera_id} not found")))?;
    let provider = camera_config::for_camera(&cam, http, cfg.isapi_request_timeout_ms)?;
    let hold = if pulse_ms == 0 {
        DEFAULT_PULSE_MS
    } else {
        pulse_ms.min(MAX_PULSE_MS)
    };

    provider.set_io_output(port, true).await?;
    tokio::time::sleep(Duration::from_millis(hold)).await;
    // Release, retrying once: a stuck-high relay is the failure mode that matters.
    if let Err(first) = provider.set_io_output(port, false).await {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Err(second) = provider.set_io_output(port, false).await {
            tracing::error!(
                %camera_id, port,
                first_error = %first, error = %second,
                "camera_control: relay release failed twice — output may be latched high"
            );
            return Err(second);
        }
    }
    Ok(hold)
}
