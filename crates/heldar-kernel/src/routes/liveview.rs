use axum::extract::{Path, State};
use axum::http::{header::HOST, HeaderMap};
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::{Cap, Principal};
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
async fn liveview(
    State(st): State<AppState>,
    principal: Principal,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> AppResult<Json<LiveUrls>> {
    // Operational action (viewer+); the extractor enforces auth when it is enabled.
    principal.require_cap(Cap::VideoLive, "view live streams")?;
    // Camera scope BEFORE the MediaMTX path is ensured: `ensure_live` mints a signed read token for
    // `cam_<id>` that MediaMTX's external-auth callback honours, so a capability-only check here
    // handed a camera-scoped credential a working live stream of any camera on the box. This one
    // insert covers GET and POST — the router registers both methods on this handler.
    st.camera_scope_check(&principal, &id)?;
    // The Host the client used lets us hand back stream URLs reachable over the tunnel / LAN.
    let host = headers.get(HOST).and_then(|v| v.to_str().ok());
    Ok(Json(mediamtx::ensure_live(&st, &id, host).await?))
}
