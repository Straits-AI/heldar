use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::error::AppResult;
use crate::services::mediamtx::{self, LiveUrls};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/cameras/{id}/liveview",
        get(liveview).post(liveview),
    )
}

/// Ensure a MediaMTX path exists for the camera and return live playback URLs.
async fn liveview(State(st): State<AppState>, Path(id): Path<String>) -> AppResult<Json<LiveUrls>> {
    Ok(Json(mediamtx::ensure_live(&st, &id).await?))
}
