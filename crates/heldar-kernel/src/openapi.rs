//! The machine-readable API contract, generated FROM the route handlers.
//!
//! The contract already existed — spread across route definitions, request/response structs,
//! dashboard types and Markdown — which is precisely the arrangement that drifts. Anything
//! hand-maintained alongside the code eventually disagrees with it; this session already found a
//! README claiming `HELDAR_INGEST_PROVENANCE` was auto-promoted when `config.rs` says it never is.
//! So the spec is generated from `#[utoipa::path]` annotations on the handlers themselves, and the
//! drift is caught by a test rather than by an integrator.
//!
//! # This is deliberately partial, and the test says so
//!
//! 151 routes are served. Annotating all of them in one change would be unreviewable, so the routes
//! documented so far are listed in [`ApiDoc`] and the rest are named in the
//! `openapi_covers_every_route` test's allowlist. That list only shrinks: adding a route without
//! documenting it fails CI, exactly as the route census made an unguarded route fail CI.
//!
//! Served at `GET /api/v1/openapi.json`, inside the `/api/v1` auth floor — the surface map is not
//! something to hand to an unauthenticated caller. Publishing it for codegen is the release
//! artifact's job, not this endpoint's.

use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::state::AppState;

/// The error body EVERY endpoint can return.
///
/// One shape, documented once and referenced everywhere, so a client writes one error path instead
/// of guessing per route. This mirrors what `AppError::into_response` actually emits today
/// (`{"error": "...", "code": "...", "retryable": false}`) rather than an aspirational envelope — a spec that describes a body the
/// server does not send is worse than no spec.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    /// Human-readable message. Not a stable identifier — do not match on it.
    #[schema(example = "camera cam_x not found")]
    pub error: String,
    /// Stable machine-readable identifier: `not_found`, `bad_request`, `conflict`, `unauthorized`,
    /// `forbidden`, `unavailable`, `busy`, `internal`. Branch on this, not on `error`.
    #[schema(example = "not_found")]
    pub code: String,
    /// Whether retrying the SAME request could plausibly succeed. True only for transient
    /// saturation; a `404` or a validation failure will fail identically forever. Retryable
    /// responses also carry `Retry-After`.
    pub retryable: bool,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Heldar Core API",
        description = "Media kernel, perception ingest and access-control surface for a Heldar box.",
        license(name = "Apache-2.0"),
    ),
    servers((url = "/", description = "The box itself")),
    paths(
        crate::routes::cameras::list_cameras,
        crate::routes::cameras::get_camera,
        crate::routes::cameras::delete_camera,
    ),
    components(schemas(ErrorBody, crate::models::CameraView)),
    tags((name = "cameras", description = "Camera registry. Every route here is camera-scoped."))
)]
pub struct ApiDoc;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/openapi.json", get(spec))
}

async fn spec() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
