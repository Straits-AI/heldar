//! Manual event-recording control.
//!
//! `POST /api/v1/cameras/{id}/record-trigger` fires an event recording trigger for a camera, exactly
//! like a zone/breach event would: it extends the camera's post-roll recording window to
//! `now + post_roll_seconds`. Only meaningful for `event` / `scheduled_event` cameras; managed by
//! manager+. Triggers are evaluated against the SERVER's wall clock.

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/cameras/{id}/record-trigger", post(record_trigger))
}

/// Result of a manual event-recording trigger.
#[derive(Debug, Serialize)]
pub struct TriggerResult {
    pub camera_id: String,
    pub triggered: bool,
    /// When the post-roll recording window currently ends (server time, UTC). Repeated triggers
    /// extend it.
    pub window_end: DateTime<Utc>,
    pub pre_roll_seconds: i64,
    pub post_roll_seconds: i64,
}

/// Manually trigger an event recording window on a camera.
///
/// Only meaningful for a camera in `event` or `scheduled_event` mode; anything else is a 400 rather
/// than a silent no-op, because "I pressed record and nothing happened" is the worst outcome here.
#[utoipa::path(
    post, path = "/api/v1/cameras/{id}/record-trigger", tag = "recordings",
    operation_id = "triggerRecording",
    params(("id" = String, Path, description = "Camera id")),
    responses(
        (status = 200, description = "The trigger window that is now open"),
        (status = 400, description = "Camera is not in an event recording mode, or recording is disabled", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `registry:manage`", body = crate::openapi::ErrorBody),
        (status = 404, description = "Unknown camera, or one this credential does not hold", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn record_trigger(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<TriggerResult>> {
    principal.require(principal.can_manage_registry(), "trigger event recording")?;
    let cam = st.camera_for(&principal, &id).await?;
    let window_end = st.recorder.trigger(&id, "manual").await.ok_or_else(|| {
        AppError::BadRequest(
            "camera is not in an event recording mode (`event` or `scheduled_event`), or recording is disabled".into(),
        )
    })?;
    auth::audit(
        &st.pool,
        &principal,
        "record_trigger",
        "camera",
        &id,
        json!({ "window_end": window_end, "post_roll_seconds": cam.post_roll_seconds }),
    )
    .await;
    Ok(Json(TriggerResult {
        camera_id: id,
        triggered: true,
        window_end,
        pre_roll_seconds: cam.pre_roll_seconds,
        post_roll_seconds: cam.post_roll_seconds,
    }))
}
