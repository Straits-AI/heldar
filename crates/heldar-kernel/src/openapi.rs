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
    /// Stable machine-readable identifier. Branch on this, not on `error`.
    ///
    /// Exactly one of: `bad_request`, `unauthorized`, `forbidden`, `not_found`, `conflict`,
    /// `payload_too_large`, `rate_limited`, `unavailable`, `internal`.
    ///
    /// This list is held to `AppError::ALL_CODES` by `codes_documented_match_codes_returned`, in
    /// both directions — a code the server can emit must appear here, and a code named here must be
    /// reachable. It previously listed one the server has never returned and omitted two it returns
    /// routinely, which is why the test exists rather than a correction. (The test also refuses the
    /// obsolete identifier by name, so this text cannot quietly reintroduce it while explaining it.)
    #[schema(example = "not_found")]
    pub code: String,
    /// Whether retrying the SAME request could plausibly succeed. True only for transient
    /// saturation; a `404` or a validation failure will fail identically forever. Retryable
    /// responses also carry `Retry-After`.
    pub retryable: bool,
}

/// The API CONTRACT's version, reported by `GET /api/v1/system` and stamped into the served spec.
///
/// Deliberately not the crate version. A patch release bumps the binary without moving a single
/// field, and a client that pinned to it would churn for nothing; a breaking API change can land in
/// any release, and a client that ignored it would break silently. This number moves only when the
/// contract does — minor for additive changes, major for anything a generated client must be
/// regenerated for.
///
/// While the contract is partial (most routes are still undocumented, see the module docs) this
/// stays `0.x`: a `1.0` would promise a completeness the document does not yet have.
pub const API_VERSION: &str = "0.1.0";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Heldar Core API",
        version = API_VERSION,
        description = "Media kernel, perception ingest and access-control surface for a Heldar box.",
        license(name = "Apache-2.0"),
    ),
    servers((url = "/", description = "The box itself")),
    paths(
        crate::routes::cameras::list_cameras,
        crate::routes::cameras::get_camera,
        crate::routes::cameras::delete_camera,
        crate::routes::evidence::create,
        crate::routes::evidence::list,
        crate::routes::evidence::get_one,
        crate::routes::evidence::signing_key,
        crate::routes::system::get_timezone,
        crate::routes::system::put_timezone,
        crate::routes::sites::list,
        crate::routes::sites::get_one,
        crate::routes::sites::create,
        crate::routes::sites::update,
        crate::routes::sites::delete_site,
    ),
    components(schemas(ErrorBody, crate::models::CameraView)),
    tags(
        (name = "cameras", description = "Camera registry. Every route here is camera-scoped."),
        (name = "system", description = "Box-wide operational settings. Fleet-wide by nature: a \
         camera-scoped credential cannot change them."),
        (name = "sites", description = "Sites and their timezones. A site's zone is what its \
         cameras' schedules and searches are read in, so changing it moves recording windows."),
        (name = "evidence", description = "Signed, offline-verifiable evidence bundles (#118). \
         Every route is camera-scoped, and gated on `video:export` except the public signing key."),
    )
)]
pub struct ApiDoc;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/openapi.json", get(spec))
}

async fn spec() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
