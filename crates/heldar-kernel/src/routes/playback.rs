use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::services::clip::ClipResult;
use crate::services::{clip, snapshot};
use crate::state::AppState;
use crate::util;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/cameras/{id}/clip", post(export_clip))
        .route("/api/v1/cameras/{id}/snapshot", get(snapshot_handler))
}

#[derive(Debug, Deserialize)]
struct ClipRequest {
    from: String,
    to: String,
}

async fn export_clip(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(req): Json<ClipRequest>,
) -> AppResult<Json<ClipResult>> {
    // Operational action (viewer+); the extractor enforces auth when it is enabled.
    principal.require_cap(Cap::VideoExport, "export clips")?;
    // Camera scope before any footage is read: an export is an arbitrary time range of this camera's
    // recordings, so capability alone let a scoped credential export a camera it does not hold.
    st.camera_scope_check(&principal, &id)?;
    let from = util::parse_rfc3339(&req.from)
        .ok_or_else(|| AppError::BadRequest("invalid `from` timestamp".into()))?;
    let to = util::parse_rfc3339(&req.to)
        .ok_or_else(|| AppError::BadRequest("invalid `to` timestamp".into()))?;
    let result = clip::export_clip(&st, &id, from, to).await?;
    auth::audit(
        &st.pool,
        &principal,
        "export_clip",
        "camera",
        &id,
        json!({ "from": from, "to": to }),
    )
    .await;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    /// Recorded-frame timestamp (RFC3339). If omitted, a live frame is grabbed.
    at: Option<String>,
}

async fn snapshot_handler(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<SnapshotQuery>,
) -> AppResult<Response> {
    // Operational action (viewer+): a snapshot can contain faces/plates.
    principal.require_cap(Cap::VideoPlayback, "capture snapshots")?;
    // Camera scope: these bytes are returned INLINE, so no media-plane guard can ever cover this
    // route — the check has to be here, before a live or recorded frame is grabbed.
    st.camera_scope_check(&principal, &id)?;
    let bytes = match q.at {
        Some(ref at) => {
            let ts = util::parse_rfc3339(at)
                .ok_or_else(|| AppError::BadRequest("invalid `at` timestamp".into()))?;
            snapshot::snapshot_at(&st, &id, ts).await?
        }
        None => snapshot::snapshot_live(&st, &id).await?,
    };

    Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .map_err(|e| AppError::Other(anyhow::anyhow!("building response: {e}")))
}
