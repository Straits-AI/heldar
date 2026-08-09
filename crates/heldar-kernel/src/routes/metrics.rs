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
async fn metrics_handler(State(st): State<AppState>, principal: Principal) -> AppResult<Response> {
    principal.require_cap(Cap::SystemRead, "read metrics")?;
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
