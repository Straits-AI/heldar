use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::error::AppResult;
use crate::services::metrics;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics_handler))
}

/// Prometheus exposition endpoint.
async fn metrics_handler(State(st): State<AppState>) -> AppResult<Response> {
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
