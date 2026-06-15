//! Segment-spanning HLS playback sessions.
//!
//! `POST /api/v1/cameras/{id}/playback/sessions` generates a trimmed HLS VOD playlist over a recorded
//! time range so operators can scrub/seek through footage; `DELETE /api/v1/playback/sessions/{id}`
//! tears it down (releasing the segment read-locks it held). Both are viewer+ (any authenticated
//! principal); `from`/`to` are RFC3339 and evaluated as absolute UTC instants.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::services::playback_session::{self, PlaybackSession};
use crate::state::AppState;
use crate::util;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/cameras/{id}/playback/sessions",
            post(create_session),
        )
        .route(
            "/api/v1/playback/sessions/{session_id}",
            axum::routing::delete(delete_session),
        )
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    from: String,
    to: String,
}

async fn create_session(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(req): Json<CreateSessionRequest>,
) -> AppResult<Json<PlaybackSession>> {
    // viewer+: any authenticated principal (the extractor enforces auth when it is enabled).
    let from = util::parse_rfc3339(&req.from)
        .ok_or_else(|| AppError::BadRequest("invalid `from` timestamp".into()))?;
    let to = util::parse_rfc3339(&req.to)
        .ok_or_else(|| AppError::BadRequest("invalid `to` timestamp".into()))?;
    let session = playback_session::create_session(&st, &id, from, to).await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_playback_session",
        "camera",
        &id,
        json!({ "session_id": session.id, "from": from, "to": to }),
    )
    .await;
    Ok(Json(session))
}

async fn delete_session(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    // viewer+: any authenticated principal.
    playback_session::delete_session(&st, &session_id).await?;
    auth::audit(
        &st.pool,
        &principal,
        "delete_playback_session",
        "playback_session",
        &session_id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
