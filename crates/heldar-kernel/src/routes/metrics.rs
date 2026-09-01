use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::auth::{Cap, Principal};
use crate::error::AppResult;
use crate::services::metrics;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics_handler))
}

/// Prometheus exposition endpoint. Metrics disclose the camera inventory, disk capacity and other
/// operational data, so require an authenticated principal — when auth is enabled, a scraper must
/// present a valid API key (Bearer); when auth is disabled (LAN-appliance default) the extractor
/// yields the synthetic admin and this stays open, unchanged.
///
/// The body carries per-camera series (`heldar_camera_up{camera=…}` and friends) for the WHOLE fleet,
/// so a camera-scoped credential is REFUSED here rather than served a filtered exposition. Filtering
/// would be worse than refusing: Prometheus reads an absent series as a camera that stopped existing
/// and writes a staleness marker, so a scoped scrape would silently corrupt the fleet's history with
/// gaps indistinguishable from real outages. A scraper is a fleet-wide machine credential; scope it
/// to cameras and it has no coherent exposition to receive. (Same reasoning as the outbox cursor.)
async fn metrics_handler(State(st): State<AppState>, principal: Principal) -> AppResult<Response> {
    principal.require_cap(Cap::SystemRead, "read metrics")?;
    crate::routes::cameras::require_fleet_scope(&principal, "scrape fleet-wide metrics")?;
    let body = metrics::render(&st.pool, &st.cfg).await?;
    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response())
}
